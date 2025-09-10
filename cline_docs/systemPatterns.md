# System Patterns — rust-nostr-relay

Architecture Overview
- Core: A high-performance Nostr relay (rnostr) extended with an Extensions framework (actix-based) for custom logic.
- Key Extensions:
  - MLS Gateway (extensions/src/mls_gateway): Handles MLS-related event kinds (443/444/445/446/447/450), group registry, mailbox, and optional message archival.
  - NIP-SERVICE (extensions/src/nip_service): Generic service action plumbing with profile routing (first profile: NIP-KR).
- Transport Pattern: MLS-first for control-plane confidentiality.
  - Sensitive control payloads are MLS application messages carried inside Nostr envelopes (kind 445). Outer tags are minimal/non-sensitive (["h", group_id]).
  - Fallback JSON control kinds (40910 service-request, 40911 service-ack) are reserved for dev/testing or non-sensitive flows.
- Service Member Model:
  - The relay can act as an MLS “service member” (in-process) or delegate to an out-of-process service member that connects like a normal MLS client.
  - Only groups for which the service member has MLS state are considered; plaintext never leaves the MLS E2EE boundary.

Critical Decisions
1) MLS-first by default
- Enforce control-plane confidentiality; never leak sensitive control operations via plaintext in Nostr envelopes or server logs.
- Kinds 40910/40911 are dev-only; NIP-17 optional.

2) Membership-first gating (authoritative)
- Primary decrypt gate: Attempt decrypt ONLY if the MLS client has the group loaded (fast in-memory has_group check).
- Avoid scanning/decrypt attempts on arbitrary 445s.
- Registry flag service_member=true is strictly advisory (ops/UX), not authorization.

3) Handler Selection (single active handler)
- Deployments choose a single active handler for service actions:
  - “in-process”: relay decrypts and dispatches when membership gate passes.
  - “external”: out-of-process service member subscribes to 445 and handles decrypt downstream; relay does not attempt decrypt.
- Idempotency by action_id is mandatory to neutralize duplicates in hybrid/failover cases.

4) Hash-only persistence; no plaintext at rest
- Relay computes secret_hash with KMS MACSign (HMAC-SHA-256, non-exportable key) using a canonical input (length-prefixed UTF-8 triples: client_id, version_id, secret).
- Firestore stores only secret_hash, algo, mac_key_ref, and metadata. No plaintext or MAC keys ever persist server-side.

5) Firestore Transactions and Idempotency
- Pointer flips (current/previous) and state transitions must be atomic (transactions) with idempotency keyed by action_id/rotation_id.
- Per-client concurrency rules prevent overlapping conflicting operations.

6) Profile Routing via NIP-SERVICE
- NIP-SERVICE defines generic envelopes for service-request/notify/ack; profiles specify semantics (NIP-KR first).
- The dispatcher maps decrypted JSON to profile handlers; logs are redacted and minimal.

7) Configurability and Safety
- Config keys govern in-process handling and ops policy (defaults safe):
  - enable_in_process_decrypt (bool, default true)
  - preferred_service_handler ("in-process" | "external", default "in-process")
  - gating_use_registry_hint (bool, default false; ops hint only)
  - mls_service_user_id (string, required for in-process membership gate)
- When unset/misconfigured, in-process path should fail-safe (no decrypt attempts) and emit metrics.

End-to-End Flow Pattern (MLS-first)
1) Inbound service-request: MLS application JSON → carried via Nostr kind 445 (["h", group_id]).
2) MlsGateway sees 445 → membership-first gate (has_group) → decrypt (service member) → NipService dispatcher.
3) Profile (NIP-KR) prepare:
   - KMS MACSign secret_hash (canonical input).
   - Firestore transaction: create version state=pending, not_before; audit entry; write pointers.
4) Outbound notify: MLS service-notify to admin group(s) with sensitive payload (e.g., plaintext secret + metadata).
5) Acks accumulated → quorum met → promotion (transactional pointers) → audit update.
6) Verifier (loxation-server) accepts current → previous during grace via MACVerify, with strict time windows.

Security Patterns
- E2EE: MLs application payloads are the only carriers of plaintext; outer transport remains opaque.
- Logging: Redacted by default; correlate via action_id, client_id, version_id only.
- KMS/IAM: MAC keys are non-exportable and constrained to Sign/Verify; mac_key_ref identifies exact key version.
- Authorization: MLS membership + attested jwt_proof (JWS with aud/exp/iat/nbf, amr includes app_attest + totp, PoP binding).

Observability & SRE Patterns
- Metrics:
  - MLS Gateway: events_processed{kind}, groups_updated, decrypt_{attempted|ok|err|skipped}{reason}
  - NIP-SERVICE: requests_total, acks_total, errors_total
- Policy and Ops Flags:
  - Optional registry “service_member” flag to drive admin UX and discoverability (not auth).
  - Handler selection toggles to switch between in-process and external service member.

Data & Schema Patterns
- Canonical Input for MAC:
  - be32(len(client_id)) || client_id || be32(len(version_id)) || version_id || be32(len(secret)) || secret
- Base64url (no padding) for MAC and random secrets.
- Firestore Collections:
  - oauth2_clients/{clientId}, oauth2_clients/{clientId}/secrets/{versionId}
  - oauth2_rotations/{rotationId}
  - mls_groups/{groupId} (advisory metadata like service_member flag)

Error & Robustness
- Idempotency enforced everywhere (action_id as key).
- Replay resistance via jwt_proof (short TTL + nonce check).
- Graceful disablement: Flip preferred_service_handler to “external” or disable in-process decrypt to offload to an external service-member without code changes.

Key Files and Roles
- extensions/src/mls_gateway/mod.rs: Core MLS Gateway; gating; registry; events.
- extensions/src/mls_gateway/service_member.rs: Service-member adapter (has_group, decrypt/encrypt hooks).
- extensions/src/nip_service/dispatcher.rs: Decrypted JSON → profile routing → KR flow.
- nip-service.md, nip-kr.md: Protocol and profile specifications; now reflect MLS-first and membership-first gating.
- docs/architecture/mls-service-member-plan.md: Design plan, storage, and ops for service-member state.

Compatibility & Extensibility
- NIP-SERVICE profiles can be added without changing core transport assumptions.
- Out-of-process service member lets operators deploy polyglot MLS stacks and scale independently.
- Native KR kinds (40901/40902) remain deployment-optional.

Design Trade-offs (Captured)
- In-process simplicity vs isolation/portability with an external service-member.
- Membership-first (authoritative) vs optional registry hints (ops-only).
- Feature gating and config toggles for safe progressive rollout.
