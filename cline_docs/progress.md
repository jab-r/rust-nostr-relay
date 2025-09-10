# Progress — rust-nostr-relay

Status Summary
- Protocol docs (nip-service.md, nip-kr.md) updated to codify MLS-first transport, membership-first gating, and handler selection with config keys.
- Code updated to introduce configuration and gating:
  - MlsGatewayConfig now includes:
    - enable_in_process_decrypt (default: true)
    - preferred_service_handler ("in-process" | "external", default: "in-process")
    - gating_use_registry_hint (default: false)
    - mls_service_user_id (optional)
  - 445 handler applies:
    - Handler check → policy hint (optional) → membership-first gate (has_group) → decrypt+dispatch (stub)
    - Metrics emitted for disabled/policy-skip/not-member/missing-user-id/decrypted/decrypt-skip
  - Service member adapter exposes has_group() (stub) and a safe dev JSON decrypt path (no plaintext logs).
- Build passes; changes are safe-by-default (no in-process decrypt unless configured with a user id).

What Works
- Membership-first gating scaffolding with config.
- Spec alignment: NIP-SERVICE/NIP-KR emphasize MLS-first, non-sensitive envelopes, and authorization requirements.
- Dev path for NIP-KR local prepare (HMAC) with in-memory store and idempotent action_id.

What’s Left to Build
- Config parsing: Wire MlsGatewayConfig from rnostr.toml in Extension::setting; apply project_id and flags; call initialize.
- RN MLS integration (feature-gated):
  - Use MlsClient to implement has_group(user_id, group_id)
  - Decrypt MLS ciphertext into JSON payload (service-request)
  - Encrypt MLS application messages for service-notify
- Outbound notifier:
  - Implement MlsNotifier to emit kind 445 MLS messages with a dedicated service signing key
- Authorization:
  - Implement jwt_proof verification via JWKS cache; validate aud/exp/iat/nbf; amr includes app_attest + totp; PoP binding
  - Verify requester’s MLS membership for the target client_id
- KR store + KMS:
  - Firestore-backed store for prepare/promote/ack with transactions and idempotency
  - GcpKmsSigner for MACSign; maintain LocalHmacSigner for dev
- Roster/policy enhancements:
  - Admin-signed ops to set/unset advisory `service_member` and optional `service_handler` per group
- Tests/Observability:
  - Unit tests for canonical encodings, store idempotency, gating logic
  - Integration E2E for MLS-first request → notify → ack → promote
  - Dashboards and runbooks

Next Steps (Actionable)
1) Ingest config keys from rnostr.toml into MlsGateway::setting; initialize Firestore backend using project_id.
2) Feature-gate RN MLS integration and implement has_group() + decrypt_application().
3) Add MlsNotifier; emit MLS service-notify to admin groups after dev prepare (to validate outbound path).
4) Implement JWKS validator and tie jwt_proof verification into NIP-SERVICE authorization checks.
5) Implement Firestore-based KR store with idempotent transactions and connect to KR profile flow.

Risks and Mitigations
- Double handling in hybrid: use preferred_service_handler + idempotency guard.
- MLS library integration complexity: proceed incrementally behind nip_service_mls; keep safe defaults.
- Config drift: centralize parsing; emit metrics and logs for missing/invalid keys; support dynamic reload if available.

Milestones
- M1 (done): Docs updated; config keys added; membership-first gating scaffolded; build passing.
- M2: Config ingestion + RN MLS has_group/decrypt
- M3: Outbound notifier; end-to-end dev validation with MLS
- M4: JWKS + KMS + Firestore store integration
- M5: Tests, dashboards, and runbook hardening
