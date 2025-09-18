#![allow(dead_code)]
//! Service Member adapter (stub) for MLS-first NIP-SERVICE path.
//!
//! This module is compiled only when the `nip_service_mls` feature is enabled.
//! It will eventually own the RN MLS client (loxation_mls_mls_rust::api::MlsClient) and
//! provide decrypt/encrypt helpers for MLS application messages.
//!
//! For now, we provide a safe stub that never leaks plaintext. It only attempts to parse
//! event.content() as JSON in dev scenarios where clients may send JSON directly as 445 content.
//! In real deployments, MLS ciphertext will not be JSON, and this stub will no-op (return None).

use nostr_relay::db::Event;
use serde_json::Value as JsonValue;
use tracing::{info, warn};

/// Fast in-memory membership gate (stub).
/// In production, this should call into the RN MLS client:
/// has_group(client: &MlsClient, user_id: &str, group_id: &str) -> bool
/// For now, return false to avoid any decrypt attempts until MLS is wired.
pub fn has_group(_user_id: &str, _group_id: &str) -> bool {
    false
}

/// Attempt to decrypt an incoming MLS group message (kind 445) into a NIP-SERVICE JSON payload.
///
/// Production: this will use the service-member MLS client to decrypt the MLS ciphertext.
/// Stub (current): if content looks like JSON, parse and validate expected NIP-SERVICE fields.
/// Returns Some(JsonValue) if the payload appears to be a NIP-SERVICE service-request; otherwise None.
///
/// NEVER logs plaintext payloads; only logs non-sensitive summary for observability.
pub async fn try_decrypt_service_request(event: &Event) -> Option<JsonValue> {
    if event.kind() != 445 {
        return None;
    }

    // DEV-only heuristic: If content looks like JSON, try to parse it.
    let raw = event.content().trim();
    if !(raw.starts_with('{') && raw.ends_with('}')) {
        // Likely MLS ciphertext; real decrypt to be implemented with RN MLS client.
        return None;
    }

    match serde_json::from_str::<JsonValue>(raw) {
        Ok(v) => {
            let action_type = v.get("action_type").and_then(|x| x.as_str());
            let action_id = v.get("action_id").and_then(|x| x.as_str());
            let client_id = v.get("client_id").and_then(|x| x.as_str());
            let profile = v.get("profile").and_then(|x| x.as_str());

            // Minimal shape check for NIP-SERVICE service-request
            let looks_like_service =
                action_type.is_some() && action_id.is_some() && client_id.is_some() && profile.is_some();

            if looks_like_service {
                info!(
                    target: "nip_service",
                    "try_decrypt_service_request: dev JSON path detected (profile={:?}, action_type={:?})",
                    profile, action_type
                );
                Some(v)
            } else {
                // Not a NIP-SERVICE payload
                None
            }
        }
        Err(e) => {
            // Opaque or malformed content; safe to skip
            warn!(
                target: "nip_service",
                "try_decrypt_service_request: content not JSON (expected for MLS ciphertext): {}",
                e
            );
            None
        }
    }
}
