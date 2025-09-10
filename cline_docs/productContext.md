# Product Context — rust-nostr-relay

Purpose
- Provide a high-security, MLS-enabled Nostr relay that orchestrates operator-initiated service actions (NIP-SERVICE), with Key Rotation (NIP-KR) as the first concrete profile.
- Eliminate plaintext exposure of secrets in control-plane infrastructure while supporting auditable, policy-enforced actions across organizations.

Why This Project Exists
- Static OAuth2 client_secrets and X-API keys are a persistent breach risk. Operators need fast, safe rotation across third-party ecosystems where dynamic secrets or STS are not applicable.
- Traditional distribution paths (UIs, APIs, tickets, emails) expose plaintext in logs/storage. We need end-to-end encrypted group distribution to authorized humans, with verifiable operator identity.
- A standardized protocol and implementation are needed so rotation flows are portable, auditable, and enforce strong identity and policy.

Core Problems Solved
- Secure human distribution: Plaintext secrets are only delivered via MLS to authorized admin group members, never persisted in relay storage/logs.
- Deterministic verification: Only HMAC hashes and metadata are stored server-side; verifiers (loxation-server) use MACVerify at runtime with “current → previous” grace windows.
- Strong operator identity: AuthZ combines MLS membership with a short-lived attested jwt_proof (App Attest + TOTP + PoP).
- Zero/low downtime cutover: Version pointers allow overlapping “current + previous” usage during controlled grace windows.
- Auditability: Firestore or DB tracks action lifecycle, pointers, and acknowledgments with idempotency and transactions.

How It Should Work (Happy Path)
1) Admin initiates a service action (e.g., rotation) from a mobile app that is an MLS group member and presents a jwt_proof (JWS) from loxation-server.
2) The request is sent as an MLS application message (MLS-first) and carried via a Nostr kind 445 envelope (non-sensitive tags only).
3) The relay’s service account (service-member) decrypts the MLS payload only if it is a group member and processes the action:
   - Authorizes (MLS membership + jwt_proof),
   - Prepares artifacts (e.g., HMAC secret_hash via KMS MACSign),
   - Writes metadata and version pointers transactionally,
   - Distributes any sensitive results via MLS to authorized admins (service-notify).
4) Acks accumulate to a quorum; the relay promotes the new version and records audit state. Verifier (loxation-server) validates presented secrets with MACVerify and honors grace.

Protocol Scope and Boundaries
- NIP-SERVICE: Defines generic service-request/notify/ack envelopes and profiles; transport defaults to MLS-first (Nostr carries only routing metadata).
- NIP-KR: Rotation profile binding; canonical encodings (length-prefixed BE; UTF-8), HMAC-SHA-256 via KMS, Firestore versioned pointers, quorum/ack, and grace.
- Transport: MLS-first required in production, NIP-17 optional, and JSON control kinds (40910/40911) reserved for dev/non-sensitive flows.
- No plaintext in Nostr envelopes, server logs, or DB. Only MLS payloads may contain plaintext, decrypted solely via service-member identity.

Key Decisions (Validated in Implementation)
- MLS-first for control-plane confidentiality; 445 envelopes include only non-sensitive tags such as ["h", group_id].
- Membership-first gating: Attempt decrypt only when the service-member MLS client has the target group loaded in memory (fast in-memory has_group check). This is the primary gating condition.
- Registry flag (service_member=true) is advisory for ops/UX; not an authorization source. It can serve as an optional prefilter.
- Handler selection: Deployments select a single active handler for service actions (“in-process” or “external” service-member). Idempotency by action_id remains mandatory in all modes.
- KMS MACSign/Verify: Relay computes and persists hash-only outputs; verifier validates presented secrets at runtime; mac_key_ref references exact key version for agility.
- Firestore transactions around pointer flips and action state for correctness and idempotency.

Stakeholders and Components
- rust-nostr-relay: Control-plane relay with MLS group routing and the service-member role (in-process optional).
- loxation-server: Validation plane for MACVerify and version-aware acceptance (current → previous grace).
- react-native-mls: Admin operator client; MLS member; initiates actions and receives sensitive MLS notifications.
- Firestore/DB: Persistent audit and state for actions, versioned secrets metadata, and pointers.

Security & Compliance Posture
- No plaintext secret storage. No plaintext in logs. Encrypted MLS payloads only.
- KMS keys are non-exportable and constrained to “use” (Sign/Verify) only.
- Strong operator identity: MLS membership + attested jwt_proof; PoP binding to npub.
- Policy enforcement: not_before minimums, grace bounds, deny lists, rate limits, and idempotency.
- Observability: Redacted logs; metrics for attempts/ok/err/skips; audit trails with rotation IDs.

Acceptance Criteria (Product-Level)
- Rotation can be initiated, prepared, distributed (MLS), acknowledged, and promoted without plaintext leak.
- Verifier accepts “current → previous” per grace; cutover is controlled and auditable.
- Idempotency and transactions prevent duplicate or conflicting rotations.
- Operator actions are auditable; policies are enforceable; metrics reflect the flow.

Out-of-Scope
- Defining general MLS or Nostr cryptographic primitives beyond what’s needed here.
- Replacing dynamic secrets or STS solutions where they fit better.

References
- nip-service.md (protocol overview and transport guidance)
- nip-kr.md (rotation profile details)
- docs/architecture/* (design plans, storage, diagrams)
