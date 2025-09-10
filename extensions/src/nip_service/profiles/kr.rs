//! NIP-KR (Rotation) profile router stub for NIP-SERVICE.
//!
//! Maps NIP-SERVICE service-request (40910) with service="rotation", profile="nip-kr/0.1.0"
//! into a structured context. This file currently provides a stub handler and a
//! local/dev "prepare" flow that demonstrates canonical input construction and
//! HMAC-SHA-256 MACSign using a dev key from env for deterministic tests.
//!
//! NOTE: This stub avoids logging plaintext secrets. It only logs non-sensitive fields.

use serde_json::Value as JsonValue;
use tracing::{info, warn};

use hmac::{Hmac, Mac};
use rand::rngs::OsRng;
use rand::RngCore;
use sha2::Sha256;
use uuid::Uuid;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;

/// Structured context extracted from tags and content.
#[derive(Debug, Clone)]
pub struct RotationRequestContext {
    pub client_id: Option<String>,
    pub rotation_id: Option<String>,
    pub mls_group: Option<String>,
    pub rotation_reason: Option<String>,
    pub not_before_ms: Option<i64>,
    pub grace_duration_ms: Option<i64>,
    pub jwt_proof_present: bool,
    pub params_keys: Vec<String>,
}

/// Result of local/dev prepare flow (non-sensitive subset).
#[derive(Debug, Clone)]
pub struct PreparedRotation {
    pub version_id: String,
    pub secret_hash: String,
    pub mac_key_ref: String,
}

/// Extract rotation-specific fields from a service-request JSON content.
pub fn extract_rotation_params(
    content: &JsonValue,
) -> (Option<String>, Option<i64>, Option<i64>, bool, Vec<String>) {
    let rotation_reason = content
        .get("params")
        .and_then(|p| p.get("rotation_reason"))
        .and_then(|x| x.as_str())
        .map(|s| s.to_owned());

    let not_before_ms = content
        .get("params")
        .and_then(|p| p.get("not_before"))
        .and_then(|x| x.as_i64());

    let grace_duration_ms = content
        .get("params")
        .and_then(|p| p.get("grace_duration_ms"))
        .and_then(|x| x.as_i64());

    let jwt_proof_present = content.get("jwt_proof").and_then(|x| x.as_str()).is_some();

    let params_keys = content
        .get("params")
        .and_then(|p| p.as_object())
        .map(|m| m.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();

    (
        rotation_reason,
        not_before_ms,
        grace_duration_ms,
        jwt_proof_present,
        params_keys,
    )
}

/// Handle a rotation service-request (stub).
///
/// This currently logs a structured summary. Next step: hand off to the KR flow:
/// - Validate jwt_proof (JWKS)
/// - AuthZ MLS membership
/// - KMS MACSign (compute secret_hash)
/// - Firestore prepare/promote transactions
/// - MLS rotate-notify to admin group(s)
/// - Track acks/quorum and finalize
pub fn handle_rotation_request(ctx: RotationRequestContext) {
    info!(
        target: "nip_service",
        "NIP-KR rotation request mapped: client_id={:?} rotation_id={:?} mls_group={:?} reason={:?} not_before_ms={:?} grace_ms={:?} jwt_proof_present={} params={:?}",
        ctx.client_id,
        ctx.rotation_id,
        ctx.mls_group,
        ctx.rotation_reason,
        ctx.not_before_ms,
        ctx.grace_duration_ms,
        ctx.jwt_proof_present,
        ctx.params_keys
    );
}

/// DEV/Local prepare flow (no KMS, no DB, no MLS).
///
/// - Generates a 32-byte secret (base64url, no padding)
/// - Generates a version_id (UUID v4)
/// - Computes HMAC-SHA-256 over canonical input using a dev key from env:
///   NIP_KR_TEST_HMAC_KEY_BASE64URL
///
/// Returns PreparedRotation with non-sensitive fields (no plaintext).
pub fn prepare_rotation_local(ctx: &RotationRequestContext) -> Option<PreparedRotation> {
    let client_id = match &ctx.client_id {
        Some(v) if !v.is_empty() => v,
        _ => {
            warn!("prepare_rotation_local: missing client_id");
            return None;
        }
    };

    // rotation_id is primarily for idempotency/audit; not required to compute MAC
    let _rotation_id = ctx.rotation_id.as_deref();

    // Generate secret (32 bytes) and base64url encode without padding
    let secret_b64 = generate_secret_base64url(32);

    // Generate version_id (UUID v4 for now; ULID can be substituted later)
    let version_id = Uuid::new_v4().to_string();

    // Build canonical input
    let canonical = canonical_input(client_id, &version_id, &secret_b64);

    // Load dev HMAC key from env
    let dev_key_b64 = match std::env::var("NIP_KR_TEST_HMAC_KEY_BASE64URL") {
        Ok(v) => v,
        Err(_) => {
            warn!("prepare_rotation_local: env NIP_KR_TEST_HMAC_KEY_BASE64URL not set; skip local MACSign");
            return None;
        }
    };

    let dev_key = match URL_SAFE_NO_PAD.decode(dev_key_b64.as_bytes()) {
        Ok(v) => v,
        Err(e) => {
            warn!("prepare_rotation_local: base64url decode dev key failed: {}", e);
            return None;
        }
    };

    // HMAC-SHA-256 MACSign
    let secret_hash = hmac_sign_base64url(&dev_key, &canonical);

    // Do NOT log plaintext secret. Only non-sensitive fields.
    let mac_key_ref = "local-test-key-v1".to_string();
    Some(PreparedRotation {
        version_id,
        secret_hash,
        mac_key_ref,
    })
}

/// Helper: generate a random secret and return base64url (no padding).
fn generate_secret_base64url(len: usize) -> String {
    let mut buf = vec![0u8; len];
    OsRng.fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(&buf)
}

/// Helper: canonical input builder (length-prefixed BE, UTF-8, no normalization).
pub fn canonical_input(client_id: &str, version_id: &str, secret: &str) -> Vec<u8> {
    fn be32(n: usize) -> [u8; 4] {
        (n as u32).to_be_bytes()
    }
    let c = client_id.as_bytes();
    let v = version_id.as_bytes();
    let s = secret.as_bytes();
    [
        &be32(c.len())[..],
        c,
        &be32(v.len())[..],
        v,
        &be32(s.len())[..],
        s,
    ]
    .concat()
}

/// Helper: HMAC-SHA-256 sign and return base64url (no padding).
fn hmac_sign_base64url(key: &[u8], data: &[u8]) -> String {
    let mut mac = <Hmac<Sha256>>::new_from_slice(key).expect("HMAC key init");
    mac.update(data);
    let tag = mac.finalize().into_bytes();
    URL_SAFE_NO_PAD.encode(tag)
}
