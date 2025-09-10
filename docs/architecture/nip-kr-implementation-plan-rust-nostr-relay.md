# NIP-KR Implementation Plan for rust-nostr-relay
Status: Draft for review
Owners: Relay Engineering
Last updated: 2025-09-09

Purpose
Implement NIP-KR (Key Rotation over Nostr + MLS) in rust-nostr-relay to:
- Accept admin-initiated rotate-requests (Nostr)
- Authorize requests via MLS membership + jwt_proof
- Generate a new secret and MAC it via KMS (MACSign) using canonical length-prefixed input
- Persist secret metadata (hash-only) + version pointers to Firestore atomically
- Distribute plaintext secret to authorized admins via MLS (rotate-notify)
- Track ack quorum and promote the version; enable rollback within grace

Alignment
- Spec: nip-kr.md (version 0.1.0)
- High-level design: high-level-architecture-automatic-api-key-rotation.md
- Server verification: kms-mac-verification-implementation.md (loxation-server)
- Implementation plan overview: oauth2-mls-key-rotation-implementation-plan.md

Scope
- New Extension: NIP-KR Rotation Service (extensions/src/nip_kr)
- Integration with existing MLS Gateway for MLS transport
- Firestore writes for oauth2_clients/* and oauth2_rotations/*
- KMS MACSign for secret_hash
- JWKS-based jwt_proof validation
- Nostr event handling for kinds 40901 (rotate-request) and 40902 (rotate-ack)
- No plaintext secret storage/logging; plaintext only in MLS payloads

Non-Goals
- Changing OAuth2 protocol for external clients
- Rotating end-user/mobile TOTP secrets (covered elsewhere)
- Implementing loxation-server validation (separate plan)

Key Decisions (Proposed)
- Canonical encoding: length-prefixed fields per spec (32-bit BE, UTF-8, no normalization)
- base64url_no_padding for all MAC encodings
- jwt_proof REQUIRED in production; MLS membership also REQUIRED
- mac_key_ref stored as exact KMS cryptoKeyVersion (preferred), or resolvable label
- Default policy values:
  - not_before minimum Δ ≥ 10 minutes
  - grace default 7 days; max 30 days
  - quorum default 1 ack; per-client overrides allowed
  - ack deadline default 30 minutes; auto-cancel if unmet

Architecture Overview

Nostr Kinds and Routes
- 40901 rotate-request (Incoming): Control-plane
  - tags: ["client", client_id], ["mls", mls_group], ["rotation", rotation_id], ["reason", rotation_reason], ["nip-kr", "0.1.0"]
  - content JSON: client_id, rotation_id, rotation_reason, not_before (ms), grace_duration_ms, mls_group, jwt_proof (compact JWS)
- 40902 rotate-ack (Incoming or Outgoing): Ack signal
  - tags: ["rotation", rotation_id], ["client", client_id], ["version", version_id], ["nip-kr", "0.1.0"]
  - body: rotation_id, client_id, version_id, ack_by, ack_at

MLS Payload
- rotate-notify (Outgoing via MLS): plaintext secret to authorized admins for client_id
  - JSON: client_id, version_id, secret, secret_hash, mac_key_ref, not_before, grace_until, rotation_id, issued_at, relay_msg_id

Data Model (Firestore)
- oauth2_clients/{clientId}
  - current_version: string
  - previous_version: string|null
  - updated_at: timestamp
  - status: "active" | "suspended" | "revoked"
  - admin_groups: string[] (authorized MLS groups)
- oauth2_clients/{clientId}/secrets/{versionId}
  - secret_hash: base64url_no_padding(HMAC_SHA256(mac_key_ref, canonical_input))
  - algo: "HMAC-SHA-256"
  - mac_key_ref: string (KMS version ref)
  - created_at: timestamp
  - not_before: timestamp
  - not_after: timestamp|null
  - state: "pending" | "current" | "grace" | "retired"
  - rotated_by: userId
  - rotation_reason: string
- oauth2_rotations/{rotationId}
  - client_id, requested_by, mls_group, new_version, old_version, not_before, grace_until
  - distribution_message_id, completed_at
  - quorum: { required: number, acks: number }
  - outcome: "promoted" | "canceled" | "expired" | "rolled_back"

Integration Points

1) MLS/Nostr Rust module integration (../react-native-mls/rust)
- Goal: Use the existing Rust MLS/Nostr module to compose and send MLS group payloads to the admin group(s) for a given client_id.
- Approach:
  - Add as a path dependency in the relay workspace (exact crate name TBD after audit). If not suitable as direct dependency, vendor a minimal sender API into a sub-crate.
  - Define an abstraction trait MlsNotifier with method:
    - send_to_group(group_id: &str, payload: Vec<u8>) -> Result<RelayMsgId>
  - Provide a concrete implementation backed by the RN MLS module.
  - Map client_id -> MLS admin group(s) via oauth2_clients/{clientId}.admin_groups.
  - For multi-group admins, broadcast rotate-notify to all authorized groups scoped for that client_id.

2) KMS MACSign (Production) and Local HMAC (Dev/Test)
- Abstraction:
  - trait MacSigner { async fn mac_sign(&self, data: &[u8]) -> Result<String>; }
  - trait MacVerifier { async fn mac_verify(&self, data: &[u8], mac: &str) -> Result<bool>; } (for future relay-side verification or tests)
- Production impl:
  - GCP: Call Cloud KMS MACSign REST API using service account; map mac_key_ref to cryptoKeyVersion resource.
  - Retry with backoff; fail closed on persistent errors; emit metrics.
- Dev impl:
  - Use hmac + sha2 crates with fixed test key (env: NIP_KR_TEST_HMAC_KEY_BASE64URL) to generate deterministic secret_hash for local testing.
- Canonical input builder:
  - fn canonical_input(client_id, version_id, secret) -> Vec<u8> using 32-bit BE length prefixes.

3) Firestore Access and Transactions
- Reuse firestore crate (already used by MLS Gateway).
- Define repo module nip_kr::store with functions:
  - prepare_rotation(rotation: RotationPrepare) -> Result<()>:
    - Create oauth2_clients/{clientId}/secrets/{versionId} with state=pending, windows, metadata
    - Write oauth2_rotations/{rotationId} entry (quorum required)
  - promote_rotation(rotation_id, client_id, version_id, quorum_state) -> Result<()> (Transaction):
    - Set current_version=new
    - Move old current to grace with not_after = not_before + grace
    - Update completed_at, outcome="promoted"
  - cancel_rotation(rotation_id, reason) -> Result<()>:
    - outcome="canceled"
- Use Firestore transactions with preconditions to avoid races:
  - Verify no concurrent rotation for the same client_id
  - Verify rotation_id idempotency (create-or-no-op)

4) Authorization: jwt_proof + MLS membership
- jwt_proof:
  - Fetch JWKS from loxation-server (config.nip_kr.jwks_url); cache keys (TTL 5–10 min)
  - Verify JWS signature (RS256/ES256 as provided)
  - Validate claims: aud, exp/iat (and nbf), amr includes app_attest + totp
  - Enforce PoP binding:
    - Recommended: Require Nostr event signature by npub in jwt_proof; cross-check event.pubkey == jwt_proof.npub (converted format) OR
    - Accept cnf.jkt (JWK thumbprint) if npub is encoded as a JWK (TBD feasibility with secp256k1)
  - Nonce verification: accept server-issued nonce bound in jwt_proof; prefer server-generated nonce (relay trusts server)
- MLS membership (belt-and-suspenders):
  - Verify the rotate-request sender belongs to an authorized admin MLS group for client_id
  - Source of truth: oauth2_clients/{clientId}.admin_groups + MLS Gateway registry (if present), or configured allowlist for MVP
- Denylist and rate limits:
  - Per-client and per-user throttling; emit policy_violation on excess

5) Secret Generation and Metadata
- Secret: 32-byte random; base64url_no_padding encode for payload
- version_id: ULID/UUID (ULID preferred)
- mac_key_ref: exact cryptoKeyVersion where possible
- secret_hash: MACSign over canonical_input
- Windows:
  - not_before: now + Δ (Δ >= policy minimum)
  - grace_until: not_before + grace_duration_ms
  - State: pending (prepare) → current (promote) → grace (old) → retired

6) MLS rotate-notify Distribution
- Payload JSON (as in spec)
- Group scoping: Only admin groups listed for client_id
- Delivery:
  - Use MlsNotifier::send_to_group for each group; capture relay_msg_id and record in oauth2_rotations
- Plaintext handling:
  - Zero logging of plaintext; scrub from memory after send

7) Ack Quorum and Promotion
- Quorum policy:
  - Default 1 ack; per-client override allowed (sensitive clients may require more)
- Ack ingestion:
  - Accept rotate-ack as MLS message into admin group or Nostr event kind 40902
  - Track acks in oauth2_rotations quorum.acks; when >= required, schedule promote
- Promote
  - Transactionally update pointers; update rotation outcome
- Deadline
  - If quorum not met by ack_deadline, cancel rotation and mark outcome="expired"

8) Configuration (config/rnostr.toml) [Proposed]
[extensions.nip_kr]
enabled = true
jwks_url = "https://<loxation-server>/jwks.json"
kms_mac_key = "projects/.../cryptoKeys/kr-mac"         # or cryptoKeyVersion
mac_key_ref = "projects/.../cryptoKeyVersions/1"
default_grace_days = 7
max_grace_days = 30
min_not_before_minutes = 10
ack_quorum_default = 1
ack_deadline_minutes = 30
nip_kr_kinds = { rotate_request = 40901, rotate_ack = 40902 }
mls_admin_group_default = "admin"
dev_local_hmac = false
dev_test_hmac_key_base64url = ""

9) Observability
- Metrics (Prometheus):
  - nip_kr_rotate_requests_total{result=...}
  - nip_kr_kms_macsign_duration_seconds (histogram)
  - nip_kr_firestore_tx_duration_seconds (histogram)
  - nip_kr_promotions_total, nip_kr_cancellations_total
  - nip_kr_acks_total
  - nip_kr_errors_total{class=jwt/kms/firestore/mls/...)
- Logs:
  - rotation_id, client_id, version_id, timestamps, result codes
  - Never log plaintext secret or MACs
- Alerts:
  - KMS error spikes, promotions delayed beyond SLO, expired (unpromoted) rotations
  - Previous-secret usage near not_after (from server metrics)

10) Security and Failure Modes
- Fail closed on:
  - jwt_proof verification failure
  - KMS MACSign failure
  - Firestore transaction conflict after retries
- Secret handling:
  - Scrub buffers; ensure no debug prints; redact payloads
- IAM/Permissions:
  - Relay SA: Firestore write on oauth2_clients/* and oauth2_rotations/*; Cloud KMS use (Sign) on MAC key; no key export
  - Server SA: Firestore read-only; Cloud KMS use (Verify)
- Disaster recovery (policy-gated):
  - Optional encrypted_secret escrow disabled by default

11) Module/Code Structure (Proposed)

extensions/src/nip_kr/
- mod.rs: Extension entrypoint (implements nostr_relay::Extension)
- config.rs: NipKrConfig (load from Setting)
- types.rs: DTOs (RotateRequest, RotateNotify, RotateAck, RotationState)
- auth.rs: jwt_proof verifier (JWKS cache) + MLS membership checks
- kms.rs: MacSigner trait + GcpKmsSigner + LocalHmacSigner
- store.rs: Firestore interactions + transactions
- notifier.rs: MlsNotifier trait + RN MLS-backed implementation
- service.rs: Orchestrates prepare → notify → ack → promote/cancel
- handlers.rs: Nostr event handlers (40901, 40902)

Dispatch
- Add .add_extension(nostr_extensions::NipKr::new(config)) to src/relay.rs after review/merge
- Kinds handled inside NipKr::message:
  - 40901 → service.prepare_and_notify()
  - 40902 → service.record_ack()

12) Work Breakdown & Milestones

M1: Foundations (2–3 days)
- Add NIP-KR extension module scaffolding
- Config parsing and defaults
- JWKS fetch+cache (reqwest + jose crate like jsonwebtoken/josekit)
- Canonical input builder + LocalHmacSigner for dev

M2: Firestore + KMS integration (3–5 days)
- Firestore store.rs with prepare/promote/cancel (transactions)
- GcpKmsSigner with retries/backoff and metrics
- Unit tests with LocalHmacSigner

M3: Nostr event handling + Policy (3–4 days)
- Implement handlers for 40901/40902
- jwt_proof verification; MLS membership stubs (until notifier integrated)
- Policy enforcement (Δ min, grace bounds, rate limits, idempotency)

M4: MLS rotate-notify integration (3–5 days)
- Integrate MlsNotifier with ../react-native-mls/rust
- Send rotate-notify payload to admin group(s) for client_id
- Capture relay_msg_id and audit in Firestore

M5: Acks and Promotion (2–3 days)
- Track acks; implement quorum; schedule promote; apply transaction
- Handle deadlines and cancelation path

M6: E2E and Hardening (3–5 days)
- E2E test in staging with one test client_id
- Metrics/alerts wiring
- Performance tuning; secure logging; buffer scrubbing

Estimated Timeline: 2–3 weeks

13) Testing Strategy

Unit
- Canonical input builder property tests (strings of various lengths, Unicode)
- base64url_no_padding compliance
- JWT verification (valid/expired/wrong aud/nbf)
- LocalHmacSigner MAC sign/verify

Integration
- Firestore prepare/promote with transaction conflicts + retries
- KMS MACSign mock/integration (guarded by env for real KMS)
- Nostr 40901/40902 flow with idempotency and duplicates

E2E
- Full rotation: prepare → MLS notify → ack → promote
- Rollback within grace (re-promote previous)
- Immediate revoke (grace=0) path coordination with loxation-server

14) Risks and Mitigations
- KMS client/library complexity in Rust: Implement REST client explicitly; fall back to LocalHmacSigner for dev only
- RN MLS module API drift: Abstract via MlsNotifier; vendor minimal API if needed
- Policy misconfiguration: Enforce hard bounds; reject on violation; log metrics
- Clock skew: Apply ±2s tolerance on windows; require NTP
- Concurrent rotations: Reject with conflict; rely on rotation_id idempotency

15) Open Items for Team Decision
- Exact crate/module name and API surface for ../react-native-mls/rust integration
- mac_key_ref storage: full cryptoKeyVersion vs logical label + mapping
- Default quorum for sensitive clients (2+ or majority?)
- Whether relay issues nonce vs trusting loxation-server-issued nonce
- JWKS cache TTL and failure behavior (fallback? fail closed recommended)

Appendix A — Example rotate-request content
{
  "client_id": "ext-totp-svc",
  "rotation_id": "01JM8VEXA8C5Q2DG0E5B1N0K4W",
  "rotation_reason": "Routine quarterly rotation",
  "not_before": 1767312000000,
  "grace_duration_ms": 604800000,
  "mls_group": "admin",
  "jwt_proof": "eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9..."
}

Appendix B — Canonical input builder (Rust)
fn canonical_input(client_id: &str, version_id: &str, secret: &str) -> Vec<u8> {
    fn le32(x: usize) -> [u8; 4] {
        let len = x as u32;
        len.to_be_bytes()
    }
    let c = client_id.as_bytes();
    let v = version_id.as_bytes();
    let s = secret.as_bytes();
    [le32(c.len()).as_slice(), c, le32(v.len()).as_slice(), v, le32(s.len()).as_slice(), s].concat()
}

Appendix C — Metrics names
- nip_kr_rotate_requests_total{result}
- nip_kr_kms_macsign_duration_seconds
- nip_kr_firestore_tx_duration_seconds
- nip_kr_promotions_total
- nip_kr_cancellations_total
- nip_kr_acks_total
- nip_kr_errors_total{class}

Suitability Assessment: ../react-native-mls/rust

Summary
- Crate: react_native_mls_rust v0.1.4 (edition 2021), library outputs: rlib, cdylib, staticlib. Suitable for direct Rust integration (no FFI required) inside rust-nostr-relay.
- Public API: src/api.rs exposes a Rust-native MlsClient with:
  - create_application_message/encrypt_message/decrypt_message
  - group create/join/add/remove/commit/self_update
  - storage path/config, sqlite-backed (sqlcipher bundled) state
  - export_secret for exporter secrets if needed
- Nostr integration: src/nostr.rs provides scaffolding for a dual-layer (MLS inner, NIP-44 outer). Nip44Crypto::encrypt/decrypt is intentionally unimplemented; not required for rotate-notify since we only need MLS application messages to admins.
- OpenMLS deps: Tracks openmls upstream (git main). Includes openmls_sqlite_storage and a local openmls_sqlite_storage crate; mature enough for server-side state management.

Integration posture for rotate-notify
- Required capability: Relay must be able to produce MLS application messages for the admin group carrying the JSON rotate-notify payload (plaintext secret, metadata).
- Option A (recommended): Service-member model
  - Add a service identity (e.g., "relay") as a member of each client’s admin MLS group.
  - Store group state under a configurable storage path (e.g., /data/mls-relay) using the crate’s sqlite provider (bundled-sqlcipher).
  - Use MlsClient::create_application_message(group_id, "relay", payload_bytes) to generate ciphertext and inject as a kind 445 event into the relay DB (writer), letting admin clients retrieve via normal subscriptions.
- Option B (future): Delivery-service pattern using DS constructs
  - Not required for NIP-KR. Current requirement is only to encrypt to existing admin members; Option A suffices.

Outbound event emission
- The current MLS Gateway mostly processes inbound kinds (443/444/445/446/447/450).
- For rotate-notify, we will add a small writer path to persist generated kind 445 MLS application messages:
  - Compose MLS ciphertext via MlsClient
  - Create an Event struct (kind 445, tags #h group_id, #k epoch if available)
  - Persist with nostr_db writer and allow subscribers to receive it (and archive if enabled)

Build/runtime considerations
- rusqlite with "bundled-sqlcipher" pulls native code; increase build time and image size. Works on Rust 2021.
- Cloud build images must include a suitable toolchain (cc, pkg-config); alternatively:
  - Evaluate using in-memory storage (openmls memory_storage) for the relay service identity if persistence is not required across restarts; or
  - Keep sqlite storage and mount a volume for persistence in staging/prod.
- License: "All rights reserved" — acceptable for internal integration within same organization.

Gaps and mitigations
- NIP-44 encryption helpers are unimplemented; not needed for rotate-notify. We will use pure MLS application messages to the admin group.
- Group membership: Relay must be explicitly added to admin groups, or a designated “service” member exists already. Coordinate via admin UX/runbook.
- Outbound publish API: The relay already has write paths; we will add a minimal adapter to create and commit events from the extension.
- Upstream API drift: We abstract through MlsNotifier so we can adapt if the module API changes.

Conclusion
- Suitability: YES, with the service-member model. The crate exposes all required primitives to generate MLS application messages and manage local state. No immediate need to implement NIP-44 for NIP-KR delivery.
- Action: Implement MlsNotifier using react_native_mls_rust::api::MlsClient and connect to the relay writer to emit kind 445 messages scoped to the admin group(s).

Review Checklist
- Validate the module structure and trait boundaries (MacSigner, MlsNotifier)
- Confirm KMS approach (REST) and environment config
- Approve Nostr kinds 40901/40902 for this relay
- Approve default policy values (Δ, grace, quorum, deadlines)
- Approve config keys for rnostr.toml
- Approve E2E validation milestones and metrics
