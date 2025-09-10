//! NIP-SERVICE configuration scaffolding.
//!
//! This module provides a basic config structure for the NIP-SERVICE extension.
//! It is intentionally minimal and uses defaults; parsing from the relay Setting
//! can be added when wiring real KMS/Firestore/MLS notifier implementations.

#[derive(Debug, Clone)]
pub struct NipServiceConfig {
    // JWKS endpoint for jwt_proof verification (loxation-server)
    pub jwks_url: Option<String>,
    // KMS MAC key resource (e.g., projects/.../cryptoKeys/kr-mac)
    pub kms_mac_key: Option<String>,
    // Optional pinned KMS key version ref
    pub mac_key_ref: Option<String>,
    // Policy defaults
    pub default_grace_days: u32,
    pub max_grace_days: u32,
    pub min_not_before_minutes: u32,
    pub ack_quorum_default: u32,
    pub ack_deadline_minutes: u32,
    // Dev/local HMAC toggle and key
    pub dev_local_hmac: bool,
    pub dev_test_hmac_key_base64url: Option<String>,
    // MLS service-member storage path (for RN MLS state)
    pub mls_service_storage_path: Option<String>,
}

impl Default for NipServiceConfig {
    fn default() -> Self {
        Self {
            jwks_url: std::env::var("NIP_SERVICE_JWKS_URL").ok(),
            kms_mac_key: std::env::var("NIP_SERVICE_KMS_MAC_KEY").ok(),
            mac_key_ref: std::env::var("NIP_SERVICE_MAC_KEY_REF").ok(),
            default_grace_days: std::env::var("NIP_SERVICE_DEFAULT_GRACE_DAYS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(7),
            max_grace_days: std::env::var("NIP_SERVICE_MAX_GRACE_DAYS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(30),
            min_not_before_minutes: std::env::var("NIP_SERVICE_MIN_NOT_BEFORE_MINUTES")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(10),
            ack_quorum_default: std::env::var("NIP_SERVICE_ACK_QUORUM_DEFAULT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1),
            ack_deadline_minutes: std::env::var("NIP_SERVICE_ACK_DEADLINE_MINUTES")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(30),
            dev_local_hmac: std::env::var("NIP_SERVICE_DEV_LOCAL_HMAC")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(true),
            dev_test_hmac_key_base64url: std::env::var("NIP_KR_TEST_HMAC_KEY_BASE64URL").ok(),
            mls_service_storage_path: std::env::var("NIP_SERVICE_MLS_STORAGE_PATH").ok(),
        }
    }
}
