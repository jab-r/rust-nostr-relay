# MLS Service Member Plan: Relay as a Full MLS Client for NIP-SERVICE (MLS-first)

Status
- Proposal/Plan (ready to implement incrementally)
- Scope: rust-nostr-relay only (this repo)
- Profiles: NIP-SERVICE (0.1.0) with NIP-KR (Rotation) as first concrete profile
- Transport: MLS-first (MLS ciphertext carried in Nostr kind 445 envelopes). NIP-17 optional.

Objectives
- Make the relay’s service account a first-class MLS client (“service member”) that fully participates in MLS group flows, including its own key lifecycle (KeyPackages, Welcome/joins, Updates/commits).
- Enforce control-plane confidentiality: service-request arrives as MLS ciphertext (kind 445), decrypted inside the server via the service member, dispatched to NipService without plaintext exposure in Nostr/logs/DB.
- Enable outbound MLS messages (e.g., NIP-KR rotate-notify) from the service member to the appropriate admin group(s) as kind 445 events.
- Keep plaintext sensitive data strictly inside MLS payloads; never persist plaintext in DB or logs.

Constraints & Security Posture
- No sensitive plaintext in Nostr envelopes, logs, or DB. Only encrypted MLS payloads may carry sensitive info.
- jwt_proof validation and MLS membership checks required for production (dev scaffolding allowed).
- Non-exportable KMS MAC key for hashing (relay uses MACSign; verifier uses MACVerify). Hash-only persisted.
- Durable, encrypted MLS state for the service member (SQLite + SQLCipher); backing storage on Cloud Run via GCS Fuse.

High-Level Design
- Inbound MLS path (preferred):
  1) Client/admin sends an MLS application message to the admin group containing a NIP-SERVICE service-request JSON payload, carried in a Nostr kind 445 event with minimal routing tags (["h", group_id]).
  2) MLS Gateway intercepts kind 445. Behind feature `nip_service_mls`, it attempts to decrypt the ciphertext via the Service Member agent.
  3) If decrypted payload matches NIP-SERVICE service-request (action_type, action_id, client_id, profile, params, jwt_proof), dispatch to NipService JSON dispatcher (no Event dependency).
  4) NipService routes to profile handlers (e.g., NIP-KR), authorizes, prepares, and (later) performs KMS MACSign + Firestore writes. Sensitive notifications go back to admin groups via MLS.

- Outbound MLS path:
  - Service member composes MLS application messages (e.g., rotate-notify for NIP-KR) and emits as Nostr kind 445 events with non-sensitive tags (["h", group_id], optionally ["k", epoch]).
  - The MLS ciphertext is the entire event content; archive can store it opaquely.

Key Components

1) ServiceMemberAgent (new: `extensions/src/mls_gateway/service_member.rs`)
- Owns a singleton instance of `react_native_mls_rust::api::MlsClient` for the service identity.
- Responsibilities:
  - init(storage_path, sqlcipher_key) → set up durable MLS state (SQLite + SQLCipher); lazy-initialized (once_cell).
  - publish_keypackages(n) → generate and output kind 443 KeyPackages for onboarding or refresh.
  - accept_welcome_from_giftwrap(giftwrap_1059) → process Welcome (444) and join the target group; update registry.
  - decrypt_application(event_445) → try to decrypt MLS content; return plaintext bytes + metadata (group_id, sender, epoch).
  - encrypt_application(group_id, plaintext) → create MLS application message bytes for that group.
  - rotate_leaf_update(group_id) → perform an Update commit to rotate the service member’s leaf key periodically or on policy triggers.
- Observability:
  - Metrics: decrypt_ok/decrypt_err, encrypt_ok/encrypt_err, keypackages_published, welcomes_accepted, updates_committed.
  - Logging: only non-sensitive metadata (group_id, action_id); never plaintext payloads.

2) NipService JSON Dispatcher (extend existing `extensions/src/nip_service/mod.rs`)
- Add `handle_service_request_payload(json: &serde_json::Value, group_hint: Option<&str>)`:
  - Validates basic shape, extracts action_type/action_id/client_id/profile/params/jwt_proof.
  - Routes to profiles (first: NIP-KR), invoking `extract_rotation_params()` and existing dev `prepare_rotation_local()` flow for now.
  - In future: enforce jwt_proof validation (JWKS) and MLS membership authorization.
- Keep existing 40910/40911 handlers as fallback/dev paths.

3) MlsNotifier (new trait: `extensions/src/nip_service/notifier.rs`)
- Trait: `send_to_group(group_id: &str, payload_json: &serde_json::Value) -> anyhow::Result<String>` returning `relay_msg_id`.
- RN MLS-backed implementation:
  - Uses `ServiceMemberAgent::encrypt_application` to produce ciphertext.
  - Creates a Nostr kind 445 Event with tags: ["h", group_id], optional ["k", epoch]; signs with `NOSTR_SERVICE_PRIVKEY_HEX`; persists via relay writer (so subscribers receive and archive picks up).
  - Future: consider signing with KMS-backed secp256k1 or abstaining from local secrets.

MLSGateway Integration (`extensions/src/mls_gateway/mod.rs`)
- Current state:
  - Kind 445 handler updates group registry and archives messages (no decryption).
- Plan:
  - Behind feature `nip_service_mls`, attempt to decrypt MLS content via `ServiceMemberAgent::decrypt_application`:
    - If successful and JSON looks like NIP-SERVICE service-request (fields present), call `NipService::handle_service_request_payload`.
    - Continue existing paths (registry update, archival) regardless of decrypt result; decryption is additive.
  - Keep logs strictly non-sensitive (group_id, provenance, action_id only if present).

Decrypt Gating Strategy
- Do not attempt to decrypt every kind 445. Gate decryption using configurable, non-sensitive signals:
  1) Group registry flag (preferred): mark group metadata `service_member=true` or `service_capabilities` contains `"nip-service"`.
     - Storage/API: extend `MlsStorage` with group metadata helpers (e.g., `group_set_flag`, `group_get_flag`) or a generic `group_set_metadata(key,value)` / `group_get_metadata(key)`.
     - Persistence (Firestore): `groups/{groupId}` doc includes `{ service_member: true }` or `{ capabilities: ["nip-service"] }`.
     - Persistence (SQL): add column(s) or JSONB metadata to store boolean flag/capability set.

- Runtime behavior:
  - Extract `group_id` from `#h` tag. If:
    - group registry indicates `service_member=true`, OR
    - event has the optional service hint tag, OR
    - group is in ALLOWLIST and not in DENYLIST, OR
    - DEV override enabled,
    then attempt `ServiceMemberAgent::decrypt_application(event_445)`. Otherwise skip and continue normal processing.
  - Rate-limit decrypt attempts per group to bound CPU usage and guard against abuse.

- Metrics/Observability:
  - `mls_service_member_decrypt_attempted_total{group}`, `mls_service_member_decrypt_ok_total{group}`, `mls_service_member_decrypt_err_total{group}`, `mls_service_member_decrypt_skipped_total{group,reason}`

- Roster/Policy (kind 450) integration:
  - Add admin-signed ops to set/unset the `service_member` flag (or capabilities) for a group.
  - Store in group registry and honor immediately in gating logic.
  - Example: `["op","set_flag"]`, `["flag","service_member"]`, `["value","1"]` (final schema TBD).

Service Member Lifecycle
- Identity:
  - MLS credential (OpenMLS/Ed25519) stored in encrypted SQLite.
  - Nostr signing key (secp256k1) for envelope events (`NOSTR_SERVICE_PRIVKEY_HEX`); future: KMS signer.
- KeyPackages (443):
  - Maintain a pool of fresh KeyPackages to allow new joins; publish when below threshold (configurable).
  - Tags: ["p", service_nostr_pubkey], ["cs", ciphersuite], ["exp", expiry_ts].
- Group Joins (1059/444 via Giftwrap):
  - On Giftwrap for the service member, accept Welcome, join group, and persist state. Update MLS group registry.
- Updates/Commits:
  - Periodically perform leaf Update commits (time-based, e.g., every 7 days) or on policy triggers; emit as kind 445.
- Rekey/ReInit:
  - Be able to process inbound commits, proposals, and reinitializations per OpenMLS via the RN MLS crate.

Configuration
- New/used environment variables:
  - NIP_SERVICE_MLS_STORAGE_PATH: path to MLS SQLite state (mount with GCS Fuse on Cloud Run).
  - NIP_SERVICE_MLS_SQLCIPHER_KEY: SQLCipher passphrase (application-layer encryption); rotate via rekey procedures.
  - NOSTR_SERVICE_PRIVKEY_HEX: secp256k1 private key to sign kind 445/443 envelopes (future: KMS).
  - NIP_SERVICE_JWKS_URL: JWKS endpoint for jwt_proof verification.
  - NIP_SERVICE_KMS_MAC_KEY / NIP_SERVICE_MAC_KEY_REF: KMS key identifiers for MACSign.
  - POLICY knobs: KeyPackage pool size/TTL, Update rotation interval, rate limits.
- Features:
  - In extensions/Cargo.toml: `nip_service_mls` feature (already added).
  - In root Cargo.toml: add mapping `nip_service_mls = ["nostr-extensions/nip_service_mls"]`.

Data Privacy & Authorization
- Decrypt only inside service member; plaintext never written to logs/DB.
- Authorization (production):
  - Verify jwt_proof via JWKS (signature + claims: aud, exp/iat/nbf, amr includes app_attest and totp).
  - Verify MLS membership for the requesting admin against the authorized admin group(s) for the client_id.
  - Enforce policy: idempotency by action_id; denylist; per-client/user rate limits.
- Note: 40910/40911 remain for dev or strictly non-sensitive flows but are discouraged.

Observability & SRE
- Metrics (Prometheus style):
  - mls_gateway_events_processed{kind}
  - service_member_decrypt_ok/err, encrypt_ok/err, keypackages_published_total, welcomes_accepted_total, updates_committed_total
  - nip_service_requests_total, acks_total, errors_total
- Logs:
  - Redacted; include group_id, action_id, client_id where possible; never include jwt_proof or sensitive params.
- Alerts:
  - Low KeyPackage pool, decrypt errors spike, update failure rates, storage health, MLS join errors.

Implementation Plan (Milestones)
- M1: Feature + Skeleton (1–2 days)
  - Add `nip_service_mls` feature (done in extensions/Cargo.toml).
  - Root Cargo feature mapping (add).
  - Implement `ServiceMemberAgent::init()`, storage setup, and decrypt stub.
  - Add `NipService::handle_service_request_payload()` that reuses NIP-KR router and dev prepare flow.
  - Wire kind 445 handler to attempt decrypt and dispatch behind the feature.
- M2: Bootstrap Service Member (2–3 days)
  - Implement KeyPackage publication (443) and basic admin CLI/logs to publish.
  - Process Welcome (1059/444) to join groups; verify registry update; ensure durable storage.
- M3: Outbound Notifier (2–3 days)
  - Implement `MlsNotifier` via ServiceMemberAgent and persist outbound 445 events with a service signer.
  - Hook NIP-KR prepare to send MLS rotate-notify payload to admin groups.
- M4: MLS Lifecycle (3–4 days)
  - Implement scheduled leaf Update commits and handle inbound commits/proposals.
  - Add policies, retries, and error handling.
- M5: AuthN/AuthZ + Store (3–5 days)
  - Implement JWKS verifier cache and hook into NipService authorization.
  - Implement Firestore-backed KR store with atomic prepare/promote and idempotency by action_id.
  - Integrate KMS MACSign client (`GcpKmsSigner`).
- M6: Testing & Hardening (ongoing; 3–5 days)
  - Unit tests for canonical encoding, HMAC, profile routing.
  - Integration tests: end-to-end MLS service-request → notify → ack → promote.
  - Redaction/safety checks; runbook updates; SLOs; backup/restore for MLS state.

Interfaces (initial)
- ServiceMemberAgent
  - `fn init(config: &NipServiceConfig) -> anyhow::Result<()>`
  - `async fn publish_keypackages(count: u32) -> anyhow::Result<Vec<nostr_relay::db::Event>>`
  - `async fn accept_welcome_giftwrap(event_1059: &nostr_relay::db::Event) -> anyhow::Result<()>`
  - `async fn decrypt_application(event_445: &nostr_relay::db::Event) -> anyhow::Result<(serde_json::Value, DecryptMeta)>`
  - `async fn encrypt_application(group_id: &str, payload: &serde_json::Value) -> anyhow::Result<Vec<u8>>`
  - `async fn rotate_leaf_update(group_id: &str) -> anyhow::Result<()>`
- NipService
  - `fn handle_service_request_payload(json: &serde_json::Value, group_hint: Option<&str>)`
- MlsNotifier
  - `async fn send_to_group(group_id: &str, payload_json: &serde_json::Value) -> anyhow::Result<String>`

Acceptance Criteria
- Inbound MLS-first:
  - A valid MLS application message carrying NIP-SERVICE service-request JSON to a group the service member belongs to is decrypted and dispatched. Dev flow records via in-memory store. No plaintext logging.
- Outbound rotate-notify:
  - Service member emits a kind 445 MLS message to the admin group with the rotate-notify payload. Event is persisted and archived. No plaintext leakage.
- Lifecycle:
  - Service member can publish KeyPackages, accept a Welcome (join group), and perform a scheduled leaf Update commit, all observable via metrics/logs.

Risks & Mitigations
- RN MLS crate API drift → Wrap via ServiceMemberAgent; gate with `nip_service_mls` feature; vendor minimal interface if necessary.
- SQLCipher configuration issues → Provide strong defaults; runbooks; add health checks and migration steps.
- Signing key management → Use Secret Manager; consider KMS signer in future.
- Authorization gaps during dev → Start with logging-only; progressively enforce JWKS + membership; guard with feature flags and config.

Operational Notes
- Mount `NIP_SERVICE_MLS_STORAGE_PATH` via GCS Fuse on Cloud Run; keep SQLCipher key in Secret Manager.
- Maintain KeyPackage pool and rotation schedule; alert on low pool.
- Ensure service member is added to required admin groups (roster/policy kind 450, bootstrap flows).
- Backups: snapshot MLS storage and Firestore (audit/state), with no plaintext.

Next Implementation Steps (short term)
1) Add root Cargo feature mapping for `nip_service_mls`.
2) Implement `NipService::handle_service_request_payload`.
3) Create `ServiceMemberAgent` skeleton with `init()` and `decrypt_application()` stub, behind `nip_service_mls`.
4) Wire MLS Gateway kind 445 to call decrypt-and-dispatch if feature enabled.
5) Build and smoke test with synthetic 445 carrying a minimal JSON payload.

References
- nip-service.md (MLS-first requirement, message schemas)
- nip-kr.md (Rotation profile canonical encoding, mac_key_ref, quorum)
- docs/architecture/mls-service-member-storage.md (SQLCipher + GCS Fuse guidance)
- docs/architecture/nip-kr-implementation-plan-rust-nostr-relay.md (integration with RN MLS, notifier design)
