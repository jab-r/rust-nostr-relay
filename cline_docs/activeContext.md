# Active Context — rust-nostr-relay

Current Work (Latest)
- Shifted NIP-SERVICE and NIP-KR to MLS-first as the default for control-plane confidentiality with non-sensitive Nostr envelopes (kind 445 + ["h", group_id]).
- Adopted “membership-first gating” for decrypt attempts:
  - Attempt decrypt ONLY if the service-member MLS client has the group loaded in memory (fast in-memory has_group check).
  - Registry flag `service_member=true` is advisory for ops/UX only; never an authorization source.
- Added handler selection policy and config:
  - Deployments select a single active handler (“in-process” or “external” service-member).
  - Keep idempotency by action_id as a backstop to neutralize duplicates in hybrid or failover scenarios.

Recent Code Changes
- MLS Gateway (extensions/src/mls_gateway/mod.rs)
  - MlsGatewayConfig extended with:
    - enable_in_process_decrypt: bool (default true)
    - preferred_service_handler: String (“in-process” | “external”, default “in-process”)
    - gating_use_registry_hint: bool (default false)
    - mls_service_user_id: Option<String>
  - 445 handler now:
    - Exits early if handler disabled or not “in-process”.
    - Optionally applies registry-hint prefilter (policy/ops only).
    - Membership-first gate using service_member::has_group(user_id, group_id) before attempting decrypt+dispatch.
    - Metrics for disabled/policy-skip/not-member/missing-user-id/decrypted/decrypt-skip; strictly redacted logs.
- Service member adapter (extensions/src/mls_gateway/service_member.rs)
  - has_group(user_id, group_id) stub added (returns false until MLS client integration).
  - try_decrypt_service_request retains safe dev-only JSON path (never logs plaintext).
- NIP-SERVICE doc (nip-service.md)
  - Documented membership-first gating and handler selection; added deployment config keys.
- NIP-KR doc (nip-kr.md)
  - Emphasized MLS-first delivery with non-sensitive envelopes and referenced gating.

What Works
- Build passes with new config fields and gating path in place (stubbed has_group prevents accidental decrypt).
- NIP-SERVICE dispatcher consumes decrypted JSON payloads and routes to KR profile stub, demonstrating local HMAC-based prepare flow for dev.
- Firestore group registry supports `service_member` flag (advisory) and helper in Firestore backend.

Open Gaps / Next Steps (Implementation)
1) Wire config parsing from rnostr.toml into MlsGateway:
   - Read keys under [extensions.mls_gateway]:
     - enable_in_process_decrypt (bool), preferred_service_handler (string), gating_use_registry_hint (bool), mls_service_user_id (string).
   - Apply values in Extension::setting and call initialize() as needed.
2) RN MLS integration (feature: nip_service_mls):
   - Add react_native_mls_rust dependency (optional) and guarded code.
   - Implement service_member::has_group using MlsClient’s in-memory registry (constant-time check).
   - Implement decrypt_application(event_445) to decrypt MLS ciphertext and return JSON payload.
   - Implement encrypt_application(group_id, payload) for outbound MLS service-notify.
3) Outbound notifier:
   - Add MlsNotifier backed by the MLS client to emit 445 events with MLS ciphertext to the admin group.
   - Use a dedicated Nostr signing key (NOSTR_SERVICE_PRIVKEY_HEX) to persist envelopes; consider KMS-backed signing later.
4) Authorization:
   - Implement jwt_proof verification (JWKS cache using reqwest; validate aud/exp/iat/nbf; amr includes app_attest + totp; PoP binding).
   - Verify MLS membership of the requester for the client_id (belt-and-suspenders).
5) Store & KMS:
   - Implement Firestore-backed KR store: idempotent prepare/promote/ack with transactions and rotation_id as the idempotency key.
   - Implement GcpKmsSigner for MACSign; keep LocalHmacSigner for dev.
6) Roster/policy operations:
   - Extend kind 450 handling with admin-signed ops to set/unset `service_member` advisory flag and (optionally) `service_handler` override per group.
   - Add corresponding storage helpers (metadata upsert) and enforcement in the gateway if policy hint is enabled.
7) Tests & Observability:
   - Unit tests: canonical input, HMAC, profile routing, membership gate.
   - Integration tests: MLS-first request → dev prepare → (future) MLS notify → ack → promote.
   - Metrics dashboards and alerts; confirm no plaintext logging; redaction checks.
8) Runbook & Config docs:
   - Document config keys, hybrid operation, and handler selection with examples.
   - Provide procedures for switching handlers safely, rotating MLS/SQLCipher keys, and monitoring KP pools.

Decisions (Key Technical)
- MLS-first by default; NIP-17 optional; 40910/40911 reserved for dev/non-sensitive flows.
- Membership-first gating as the primary control to avoid scanning all 445.
- Registry flags are advisory; never authorization.
- Single active handler model (in-process vs external) + idempotency guard.
- Hash-only persistence; no plaintext at rest or in logs.

Risks / Mitigations
- Double handling in hybrid: Use handler selection + idempotency to neutralize duplicates.
- MLS client integration complexity: Feature-gate; start with minimal has_group and decrypt; add encrypt next.
- Config drift: Centralize config parsing; expose readiness checks and metrics.
- Performance: has_group is constant-time in-memory; prevents decrypt attempts for irrelevant groups.

Rollback Strategy
- If in-process decrypt causes instability, flip preferred_service_handler="external" or set enable_in_process_decrypt=false to delegate entirely to an external service-member.
- Idempotency and safe logging prevent state corruption; revert code is low risk.

Pointers (Files Touched)
- Code: extensions/src/mls_gateway/mod.rs, extensions/src/mls_gateway/service_member.rs, extensions/src/nip_service/dispatcher.rs, src/relay.rs
- Docs: nip-service.md, nip-kr.md, docs/architecture/mls-service-member-plan.md

Immediate Next Steps (Actionable)
- Parse and apply config keys in MlsGateway::setting; pass mls_service_user_id.
- Implement RN MLS has_group + real decrypt for in-process path (behind feature).
- Add basic MlsNotifier for outbound 445 after dev prepare to validate end-to-end flow with MLS.
