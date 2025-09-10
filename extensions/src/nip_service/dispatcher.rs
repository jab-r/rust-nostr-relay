use serde_json::Value as JsonValue;
use tracing::{info, warn};
use crate::nip_service::store::NipKrStore;

/// Handle a decrypted MLS-first NIP-SERVICE service-request payload (JSON).
/// This path avoids any dependency on Nostr events/tags and takes an optional group hint.
///
/// Expected JSON shape (nip-service.md):
/// {
///   "action_type": "rotation",
///   "action_id": "ULID/UUID",
///   "client_id": "string",
///   "profile": "nip-kr/0.1.0",
///   "params": { ... },
///   "jwt_proof": "compact JWS"
/// }
pub fn handle_service_request_payload(json: &JsonValue, group_hint: Option<&str>) {
    let action_type = json.get("action_type").and_then(|v| v.as_str()).map(|s| s.to_string());
    let action_id = json.get("action_id").and_then(|v| v.as_str()).map(|s| s.to_string());
    let client_id = json.get("client_id").and_then(|v| v.as_str()).map(|s| s.to_string());
    let profile = json.get("profile").and_then(|v| v.as_str()).map(|s| s.to_string());

    // Basic shape validation (non-sensitive fields only)
    if action_type.is_none() || action_id.is_none() || client_id.is_none() || profile.is_none() {
        warn!(
            target: "nip_service",
            "MLS-first service-request missing required fields: action_type={:?} action_id={:?} client_id={:?} profile={:?}",
            action_type, action_id, client_id, profile
        );
        return;
    }

    // Route profiles. First supported: rotation (NIP-KR 0.1.0)
    if action_type.as_deref() == Some("rotation") && profile.as_deref() == Some("nip-kr/0.1.0") {
        // Extract rotation-specific fields using existing helper.
        let (rotation_reason, not_before_ms, grace_duration_ms, jwt_present, params_keys) =
            crate::nip_service::profiles::kr::extract_rotation_params(json);

        let ctx = crate::nip_service::profiles::kr::RotationRequestContext {
            client_id: client_id.clone(),
            rotation_id: action_id.clone(),
            mls_group: group_hint.map(|s| s.to_owned()),
            rotation_reason: rotation_reason.clone(),
            not_before_ms,
            grace_duration_ms,
            jwt_proof_present: jwt_present,
            params_keys,
        };

        // Log a redacted summary (no plaintext).
        info!(
            target: "nip_service",
            "MLS-first service-request mapped: profile=nip-kr/0.1.0 client_id={:?} action_id={:?} group_hint={:?} jwt_proof_present={} params={:?}",
            client_id, action_id, group_hint, jwt_present, ctx.params_keys
        );

        // Stub handler (authorization, KMS, Firestore to be wired later)
        crate::nip_service::profiles::kr::handle_rotation_request(ctx.clone());

        // DEV/local: demonstrate prepare (no KMS/DB/MLS), using env NIP_KR_TEST_HMAC_KEY_BASE64URL
        if let Some(prep) = crate::nip_service::profiles::kr::prepare_rotation_local(&ctx) {
            info!(
                target: "nip_service",
                "NIP-KR local prepare (MLS-first): version_id={} mac_key_ref={} secret_hash_len={}",
                prep.version_id, prep.mac_key_ref, prep.secret_hash.len()
            );

            // Persist a dev record in the in-memory store to exercise the flow.
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
                        warn!("NIP-KR dev store prepare (MLS-first) failed: {}", e);
                    } else {
                        info!(
                            target: "nip_service",
                            "NIP-KR dev store prepared (MLS-first): client_id={} version_id={} rotation_id={}",
                            cid, ver, rid
                        );
                    }
                } else {
                    warn!("NIP-KR dev store prepare (MLS-first) skipped: missing client_id/action_id");
                }
            });
        } else {
            warn!("NIP-KR local prepare (MLS-first) skipped (missing/invalid NIP_KR_TEST_HMAC_KEY_BASE64URL)");
        }
        return;
    }

    // Unknown or unsupported profile
    warn!(
        target: "nip_service",
        "MLS-first service-request unsupported: action_type={:?} profile={:?} (ignored)",
        action_type, profile
    );
}
