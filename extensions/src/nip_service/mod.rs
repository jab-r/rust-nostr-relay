#![allow(dead_code)]
//! NIP-SERVICE Extension (generic service account action plumbing)
//!
//! Handles control-plane events for service-request (40910) and service-ack (40911).
//! Validates basic tags/shape and logs/metrics for routing to profile-specific handlers (e.g., NIP-KR).
//!
//! This is an initial scaffold. Profile execution (e.g., rotation) will be wired in a follow-up.

use actix_web::web::ServiceConfig;
use metrics::{counter, describe_counter};
use nostr_relay::{Extension, ExtensionMessageResult, Session};
use nostr_relay::db::Event;
use serde_json::Value as JsonValue;
use tracing::{info, warn};
use crate::nip_service::store::NipKrStore;

pub mod profiles;
pub mod config;
pub mod store;
pub mod dispatcher;

const SERVICE_REQUEST_KIND: u16 = 40910; // NIP-SERVICE: service-request
const SERVICE_ACK_KIND: u16 = 40911;     // NIP-SERVICE: service-ack
const SERVICE_NOTIFY_KIND: u16 = 40912;  // Optional: service-notify (non-sensitive via Nostr; MLS preferred)

#[derive(Debug, Clone, Default)]
pub struct NipService;

impl NipService {
    pub fn new() -> Self {
        // Metrics descriptors (idempotent)
        describe_counter!("nip_service_events_processed", "Number of NIP-SERVICE events processed by kind");
        describe_counter!("nip_service_requests_total", "Count of service-request (40910) processed");
        describe_counter!("nip_service_acks_total", "Count of service-ack (40911) processed");
        describe_counter!("nip_service_errors_total", "Count of errors while processing NIP-SERVICE events");
        Self
    }

    fn handle_service_request(&self, event: &Event) {
        counter!("nip_service_events_processed", "kind" => "40910").increment(1);
        counter!("nip_service_requests_total").increment(1);

        // Extract important tags per spec
        let service = get_tag(event, "service");
        let profile = get_tag(event, "profile");
        let client_id = get_tag(event, "client");
        let mls_group = get_tag(event, "mls");
        let action_id = get_tag(event, "action");
        let nip_service = get_tag(event, "nip-service");

        // Basic shape validation/logging (full auth: jwt_proof + MLS membership handled downstream)
        if service.is_none() || profile.is_none() || client_id.is_none() || action_id.is_none() {
            warn!("NIP-SERVICE 40910 missing required tags. service={:?}, profile={:?}, client={:?}, action={:?}",
                service, profile, client_id, action_id);
        }

        // Parse content JSON for params and jwt_proof (non-sensitive)
        let mut action_type = None::<String>;
        let mut jwt_present = false;
        let mut params_keys: Vec<String> = Vec::new();

        let ct = event.content();
        match serde_json::from_str::<JsonValue>(ct.as_str()) {
            Ok(v) => {
                action_type = v.get("action_type").and_then(|x| x.as_str()).map(|s| s.to_owned());
                jwt_present = v.get("jwt_proof").and_then(|x| x.as_str()).is_some();
                if let Some(p) = v.get("params").and_then(|x| x.as_object()) {
                    params_keys = p.keys().cloned().collect();
                }
            }
            Err(e) => {
                warn!("NIP-SERVICE 40910 content not valid JSON: {}", e);
            }
        }

        info!(
            target: "nip_service",
            "service-request 40910 received: service={:?} profile={:?} action_id={:?} client_id={:?} mls_group={:?} nip_service_tag={:?} action_type={:?} jwt_proof_present={} params={:?}",
            service, profile, action_id, client_id, mls_group, nip_service, action_type, jwt_present, params_keys
        );

        // Route to NIP-KR (Rotation) profile stub if applicable
        if service.as_deref() == Some("rotation") && profile.as_deref() == Some("nip-kr/0.1.0") {
            let ct2 = event.content();
            if let Ok(json) = serde_json::from_str::<JsonValue>(ct2.as_str()) {
                let (rotation_reason, not_before_ms, grace_duration_ms, jwt_present2, params_keys2) =
                    crate::nip_service::profiles::kr::extract_rotation_params(&json);

                let ctx = crate::nip_service::profiles::kr::RotationRequestContext {
                    client_id: client_id.clone(),
                    rotation_id: action_id.clone(),
                    mls_group: mls_group.clone(),
                    rotation_reason: rotation_reason.clone(),
                    not_before_ms,
                    grace_duration_ms,
                    jwt_proof_present: jwt_present2,
                    params_keys: params_keys2,
                };
                crate::nip_service::profiles::kr::handle_rotation_request(ctx.clone());
                // DEV/local: demonstrate prepare (no KMS/DB/MLS), using env NIP_KR_TEST_HMAC_KEY_BASE64URL
                if let Some(prep) = crate::nip_service::profiles::kr::prepare_rotation_local(&ctx) {
                    info!(
                        target: "nip_service",
                        "NIP-KR local prepare: version_id={} mac_key_ref={} secret_hash_len={}",
                        prep.version_id, prep.mac_key_ref, prep.secret_hash.len()
                    );

                    // Also persist a dev record in the in-memory store to exercise the flow.
                    let cid = client_id.clone();
                    let rid = action_id.clone();
                    let ver = prep.version_id.clone();
                    let hash = prep.secret_hash.clone();
                    let mkr = prep.mac_key_ref.clone();
                    let reason = rotation_reason.clone();
                    // not_before default: now + 10 minutes if not provided
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as i64;
                    let effective_not_before = not_before_ms.unwrap_or(now_ms + 10 * 60 * 1000);
                    let grace_ms = grace_duration_ms;

                    tokio::spawn(async move {
                        if let (Some(cid), Some(rid)) = (cid, rid) {
                            let store = crate::nip_service::store::get_global_store();
                            if let Err(e) = store
                                .prepare_rotation(
                                    &cid,
                                    &ver,
                                    &hash,
                                    &mkr,
                                    effective_not_before,
                                    grace_ms,
                                    &rid,
                                    reason.as_deref(),
                                    1, // quorum_required (dev default)
                                )
                                .await
                            {
                                warn!("NIP-KR dev store prepare failed: {}", e);
                            } else {
                                info!(
                                    target: "nip_service",
                                    "NIP-KR dev store prepared: client_id={} version_id={} rotation_id={}",
                                    cid, ver, rid
                                );
                            }
                        } else {
                            warn!("NIP-KR dev store prepare skipped: missing client_id/action_id");
                        }
                    });
                } else {
                    warn!("NIP-KR local prepare skipped (missing/invalid NIP_KR_TEST_HMAC_KEY_BASE64URL)");
                }
            } else {
                warn!("NIP-KR route: content JSON parse failed");
            }
        }

        // TODO: Dispatch to profile router when available.
        // For example, if service == Some(\"rotation\") && profile == Some(\"nip-kr/0.1.0\"):
        // map params to NIP-KR rotate-request semantics and forward to KR handler.
    }

    fn handle_service_ack(&self, event: &Event) {
        counter!("nip_service_events_processed", "kind" => "40911").increment(1);
        counter!("nip_service_acks_total").increment(1);

        let service = get_tag(event, "service");
        let profile = get_tag(event, "profile");
        let client_id = get_tag(event, "client");
        let action_id = get_tag(event, "action");

        info!(
            target: "nip_service",
            "service-ack 40911 received: service={:?} profile={:?} action_id={:?} client_id={:?}",
            service, profile, action_id, client_id
        );

        // DEV/local: For rotation profile, record ack and promote immediately (quorum=1 default).
        if service.as_deref() == Some("rotation") && profile.as_deref() == Some("nip-kr/0.1.0") {
            let rid = action_id.clone();
            let cid = client_id.clone();
            tokio::spawn(async move {
                if let (Some(rid), Some(cid)) = (rid, cid) {
                    let store = crate::nip_service::store::get_global_store();
                    if let Err(e) = store.record_ack(&rid).await {
                        warn!("NIP-KR dev store ack failed: {}", e);
                    }
                    if let Err(e) = store.promote_rotation(&cid, &rid).await {
                        warn!("NIP-KR dev store promote failed: {}", e);
                    } else {
                        info!(
                            target: "nip_service",
                            "NIP-KR dev store promoted: client_id={} rotation_id={}",
                            cid, rid
                        );
                    }
                } else {
                    warn!("NIP-KR dev store ack/promote skipped: missing client_id/action_id");
                }
            });
        }
    }
}

fn get_tag(event: &Event, key: &str) -> Option<String> {
    event
        .tags()
        .iter()
        .find(|tag| tag.len() >= 2 && tag[0] == key)
        .map(|tag| tag[1].clone())
}

impl Extension for NipService {
    fn name(&self) -> &'static str {
        "nip-service"
    }

    fn setting(&mut self, _setting: &nostr_relay::setting::SettingWrapper) {
        // No settings yet; keep for parity with other extensions
        info!("NIP-SERVICE settings applied");
    }

    fn config_web(&mut self, _cfg: &mut ServiceConfig) {
        // No HTTP endpoints for now
    }

    fn connected(&self, session: &mut Session, _ctx: &mut <Session as actix::Actor>::Context) {
        info!("Client connected to NIP-SERVICE: {}", session.id());
    }

    fn disconnected(&self, session: &mut Session, _ctx: &mut <Session as actix::Actor>::Context) {
        info!("Client disconnected from NIP-SERVICE: {}", session.id());
    }

    fn message(
        &self,
        msg: nostr_relay::message::ClientMessage,
        _session: &mut Session,
        _ctx: &mut <Session as actix::Actor>::Context,
    ) -> ExtensionMessageResult {
        if let nostr_relay::message::IncomingMessage::Event(event) = &msg.msg {
            match event.kind() {
                SERVICE_REQUEST_KIND => {
                    let ev = event.clone();
                    tokio::spawn({
                        let this = self.clone();
                        async move {
                            this.handle_service_request(&ev);
                        }
                    });
                }
                SERVICE_ACK_KIND => {
                    let ev = event.clone();
                    tokio::spawn({
                        let this = self.clone();
                        async move {
                            this.handle_service_ack(&ev);
                        }
                    });
                }
                SERVICE_NOTIFY_KIND => {
                    // Typically MLS is used for notify; if 40912 is seen, just log for now.
                    counter!("nip_service_events_processed", "kind" => "40912").increment(1);
                    info!("service-notify 40912 observed (non-sensitive path)");
                }
                _ => {}
            }
        }

        ExtensionMessageResult::Continue(msg)
    }
}
