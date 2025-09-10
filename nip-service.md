# NIP-SERVICE: Service Account Action Protocol over Nostr + MLS

Status
- Draft (internal)
- NIP-SERVICE version: 0.1.0
- Initial Profile(s): Rotation (NIP-KR 0.1.0)
- Target projects:
  - rust-nostr-relay (service control-plane + MLS service member)
  - loxation-server (validation/execution plane for applicable actions)
  - react-native-mls (admin/operator client; MLS membership)

Abstract
NIP-SERVICE defines a general protocol for enabling a server-side service account to participate in MLS-backed, operator-initiated workflows over a Nostr control plane. It standardizes how “service actions” are requested, authorized, executed, and audited, while ensuring sensitive payloads are distributed only via MLS to authorized group members. The Rotation profile (NIP-KR) is the first concrete action; additional action profiles (e.g., policy updates, escrow, notifications) can be added without changing the core.

Key properties:
- Control-plane via Nostr for non-sensitive parameters and audit tags
- Data-plane via MLS for any sensitive payload distribution to authorized MLS admin groups
- Service account participates as an MLS “service member” for action delivery and/or orchestration
- Strict authZ (MLS membership + jwt_proof) and complete audit trails
- Pluggable “profiles” defining action-specific schemas and semantics

Terminology
- Service action: A named operation initiated by an admin and executed/assisted by a service account (e.g., rotation, policy_update).
- Profile: Action-specific schema/behavior specification (e.g., Rotation profile = NIP-KR).
- Service member: The relay’s MLS identity participating in target admin groups to deliver MLS messages.
- Admin group(s): Per-client MLS group(s) of authorized operators receiving sensitive action payloads.
- jwt_proof: Short-lived attested admin token (server-signed JWS) binding device/app integrity, TOTP, and PoP.
- Action ID: Unique identifier for an action invocation (ULID/UUID).

Scope
- Defines generic envelopes for service-request, service-notify, and service-ack.
- Defines authorization, idempotency, audit fields, and transport constraints.
- Leaves action-specific payload schemas to profiles (e.g., Rotation = NIP-KR).
- Non-goals: Redefine MLS/Nostr cryptographic details, or replace profile specs.

Protocol Overview

Message types (MLS-first)
- service-request (MLS preferred; carried inside a Nostr envelope):
  - Admin → Relay (Service Member) using an MLS application message addressed to the target admin group.
  - The outer Nostr event carries only routing metadata (e.g., ["h", group_id]) and MUST NOT contain sensitive fields. The inner MLS payload carries action_type, profile, params, jwt_proof, etc., encrypted end-to-end to the group (including the service member).
  - Production deployments SHOULD use MLS for service-request to avoid leaking control operations.
  - A fallback Nostr JSON envelope (kind 40910) is defined for dev/test or non-sensitive flows; discouraged in production.
- service-notify (MLS preferred; Nostr optional for non-sensitive):
  - Relay → Admin group(s) with MLS payload containing sensitive or result data.
  - If notify is strictly non-sensitive, a Nostr event (kind 40912) MAY be used.
- service-ack:
  - Admin → Relay; MAY be MLS (preferred) or Nostr (40911) depending on sensitivity. Acks can be kept non-sensitive (e.g., only action_id), but MLS is recommended for uniformity.

Role of Service Member (MLS)
- The relay runs as an MLS “service member” within each client’s admin group(s).
- For sensitive actions, the relay uses MLS to E2EE-deliver result/payloads (e.g., secrets in Rotation).
- For non-sensitive actions, notify can be a Nostr event; MLS still recommended for uniformity/authorization.

Kinds and Tags (Enterprise/private experimental)
- MLS carriage (preferred):
  - service-request and service-notify are MLS ciphertexts transported via standard MLS group message events (e.g., kind 445). The Nostr envelope includes only routing tags such as:
    - ["h", group_id] — target MLS group
    - Optional minimal scoping tags (non-sensitive); avoid leaking action types
- Nostr control kinds (fallback/dev/test):
  - 40910: service-request (JSON; discouraged in production)
  - 40911: service-ack (JSON; acceptable if non-sensitive)
  - 40912: service-notify (JSON; only for non-sensitive notify)
- Tags (common across kinds; profiles may add):
  - ["service", action_type] — e.g., "rotation", "policy_update"
  - ["action", action_id]
  - ["client", client_id]
  - ["mls", mls_group] — primary admin group scope
  - ["nip-service", "0.1.0"]
  - ["profile", profile_id] — e.g., "nip-kr/0.1.0"

Decrypt Gating (Implementation Guidance)
- Implementations SHOULD NOT attempt to decrypt every MLS group message (kind 445).
- Membership-first gating: Attempt decrypt ONLY if the service-member MLS client has the group loaded in memory (e.g., has_group(client, user_id, group_id)). This is the primary condition. Registry flags are advisory for ops/UX and MUST NOT be treated as authorization.
- Gate decryption using non-sensitive signals:
  - Group registry flag (preferred): mark group metadata service_member=true (or capabilities includes "nip-service"). The relay attempts MLS-first decrypt/dispatch ONLY for such groups.
  - Optional event hint (deployment-specific): a generic, non-sensitive hint tag MAY be used; action types and profiles MUST NOT be exposed in 445 tags.
- Roster/Policy integration: An admin-signed roster/policy stream (e.g., kind 450) MAY set/unset the service_member flag. The group registry SHOULD honor it immediately for gating.
- Observability: Track decrypt_attempted/ok/err/skipped metrics by group. Never log plaintext; logs MUST remain redacted.

Authorization (Normative)
- The relay MUST verify:
  - The sender is a member of an authorized MLS admin group for the target client_id (group scope).
  - jwt_proof (JWS) is valid and fresh:
    - Verify signature via loxation-server JWKS
    - Check aud, exp/iat (and nbf if present)
    - Ensure amr includes app_attest and totp
    - Bind npub via PoP (either cnf.jkt or require that the Nostr event is signed by the attested npub within jwt_proof)
  - Rate limits and denylist policies (per user and per client) MUST be enforced.
- The relay MUST scope MLS distribution of any service-notify payload to only the authorized admin group(s) for that client_id.

Idempotency, Atomicity, and Concurrency
- service-request MUST include action_id (ULID/UUID).
- Relay MUST treat action_id as an idempotency key:
  - Re-requests with the same action_id MUST NOT create duplicate executions.
- Per-client concurrency:
  - By default, reject conflicting concurrent service actions of the same profile for the same client_id unless the profile explicitly allows it (policy).

Data Model (Audit) — Example (Firestore/DB)
- service_actions/{actionId}
  - action_type: string (e.g., "rotation", "policy_update")
  - profile: string (e.g., "nip-kr/0.1.0")
  - client_id: string
  - requested_by: userId
  - mls_group: string
  - state: "requested" | "prepared" | "notified" | "completed" | "canceled" | "expired" | "failed"
  - not_before: timestamp|null (profile-dependent)
  - deadline_at: timestamp|null (ack quorum or execute-by)
  - quorum: { required: number, acks: number }
  - notify_message_id: string|null (MLS or Nostr id)
  - outcome: string|null (profile-dependent result code)
  - created_at / updated_at: timestamp

Transport Requirements (Normative)
- Control-plane confidentiality:
  - service-request MUST use MLS (carried in a Nostr envelope) in production. Fallback 40910 is for dev/test or non-sensitive cases only.
  - service-ack SHOULD use MLS for uniform confidentiality; a Nostr ack (40911) MAY be used if strictly non-sensitive.
- Data-plane (service-notify) for sensitive results MUST be MLS to authorized admin group(s).
- Outer transport is Nostr WebSocket; inner sensitive content is MLS-encrypted. TLS MUST be used for all network paths.

Canonical Encoding and Encoding Rules
- For profile-specific MAC or cryptographic operations (e.g., NIP-KR Rotation), canonical encodings are defined by the profile (see nip-kr.md).
- All base64url encodings in this spec and profiles MUST be base64url without padding. Non-canonical encodings MUST be rejected.

Message Formats

MLS service-request (preferred; carried via Nostr kind 445)
- Envelope (Nostr):
  - kind: 445 (MLS group message)
  - tags: MUST include ["h", group_id]; SHOULD avoid revealing action types; additional non-sensitive routing tags MAY be added by deployment policy
  - content: MLS ciphertext (opaque to the relay except via its service-member MLS identity)
- MLS payload (JSON inside MLS, encrypted E2EE to group members including service member):
  {
    "action_type": "rotation",                 // profile-defined
    "action_id": "ULID/UUID",
    "client_id": "string",
    "profile": "nip-kr/0.1.0",
    "params": { /* profile-specific */ },
    "jwt_proof": "compact JWS"
  }

Note: The relay processes the decrypted payload only through its MLS service-member identity; no plaintext is exposed in Nostr or logs.


service-request (Nostr) — kind 40910 (fallback/dev/test)
- tags (MUST include):
  - ["service", action_type]
  - ["profile", profile_id]              // e.g., "nip-kr/0.1.0"
  - ["client", client_id]
  - ["mls", mls_group]
  - ["action", action_id]
  - ["nip-service", "0.1.0"]
- content (JSON):
  - action_type: string
  - action_id: string (ULID/UUID)
  - client_id: string
  - profile: string ("nip-kr/0.1.0", etc.)
  - params: object (profile-specific non-sensitive parameters)
  - jwt_proof: string (compact JWS)

Example (Rotation):
{
  "action_type": "rotation",
  "action_id": "01JM8W5YJ4GSD4N7T6X9QZP3R0",
  "client_id": "ext-totp-svc",
  "profile": "nip-kr/0.1.0",
  "params": {
    "rotation_reason": "Routine quarterly rotation",
    "not_before": 1767312000000,
    "grace_duration_ms": 604800000
  },
  "jwt_proof": "eyJhbGciOiJSUzI1NiIsInR..."
}

service-notify (MLS preferred; Nostr 40912 allowed for non-sensitive)
- MLS body JSON (profile-specific), delivered only to authorized admin group(s).
- MUST include action identifiers for audit correlation:
  - action_type, action_id, client_id, profile, issued_at, relay_msg_id
- Rotation example (see nip-kr.md rotate-notify).

service-ack (MLS or Nostr 40911)
- tags:
  - ["service", action_type]
  - ["action", action_id]
  - ["client", client_id]
  - ["profile", profile_id]
  - ["nip-service", "0.1.0"]
- content (JSON):
  - action_type, action_id, client_id, profile
  - ack_by: string (userId or MLS member id)
  - ack_at: number (unix ms)
  - result: optional object (profile-specific; e.g., "received": true)

Service Member Responsibilities (Relay)
- Maintain a durable MLS state for the service identity (“service member”) using a secure provider (e.g., SQLite + SQLCipher persisted via GCS Fuse in Cloud Run).
- For sensitive actions, compose MLS application messages for the admin group(s) with the service member.
- Securely handle any plaintext in memory only; never persist plaintext to DB or logs.

Implementation Placement (Informative)
- In-process service member (inside the relay):
  - Pros: simpler wiring and lower latency; direct access to inbound 445 events; unified observability.
  - Cons: larger blast radius; relay and MLS logic are coupled; careful resource isolation needed.
- Out-of-process service member (separate service):
  - Pros: strong fault/isolation boundaries; independent scaling and deployment cadence; portability across relays.
  - Cons: higher complexity (ingest/publish paths, retries, idempotency), additional credentials, and registry synchronization.

This specification is transport/profile-centric and placement-agnostic. Deployments MUST preserve the same JSON schemas, authorization rules (jwt_proof + MLS membership), and confidentiality guarantees regardless of placement.

Implementation Configuration (Deployment)
- enable_in_process_decrypt: boolean (default: true)
  - When false, the relay never attempts in-process decrypt/dispatch; external service-members can handle MLS messages as normal clients.
- preferred_service_handler: "in-process" | "external" (default: "in-process")
  - Selects a single active handler at deployment scope. The non-selected path SHOULD be passive to reduce churn. Idempotency by action_id MUST remain enforced to neutralize duplicates in hybrid scenarios.
- gating_use_registry_hint: boolean (default: false)
  - Optional prefilter to skip attempts when registry does not mark a group as service-enabled. Security note: this is an ops hint only; MLS membership remains the authoritative gate.
- mls_service_user_id: string (optional)
  - Service-member user identifier for the relay’s MLS client, used by the membership-first gate (has_group(client, user_id, group_id)) when in-process decrypt is enabled.

Profiles

Rotation (Profile: NIP-KR 0.1.0)
- See nip-kr.md for full details.
- Sensitive payload: plaintext client_secret → MUST use MLS service-notify.
- Verifier plane: loxation-server performs KMS MACVerify (current→previous grace).
- Control-plane kinds reuse/align: NIP-KR uses 40901/40902. For NIP-SERVICE, 40910/40911 can alias or forward to NIP-KR handlers (deployments may harmonize kind handling to avoid duplication).

Rotation profile binding details
- Tags mapping (service-request → NIP-KR semantics)
  - ["service","rotation"] → action type = rotation
  - ["profile","nip-kr/0.1.0"] → NIP-KR profile/version
  - ["client",client_id] → client_id
  - ["mls",mls_group] → target admin MLS group
  - ["action",action_id] → rotation_id
  - ["nip-service","0.1.0"] → NIP-SERVICE version tag

- Content mapping (service-request.content → rotate-request.content):
  - action_type (must be "rotation")
  - action_id → rotation_id
  - client_id → client_id
  - profile (must be "nip-kr/0.1.0")
  - params.rotation_reason → rotation_reason
  - params.not_before → not_before (unix ms)
  - params.grace_duration_ms → grace_duration_ms
  - jwt_proof → jwt_proof (compact JWS)

JSON Schema (service-request content for rotation)
```json
{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "NIP-SERVICE service-request content (Rotation profile: nip-kr/0.1.0)",
  "type": "object",
  "required": ["action_type", "action_id", "client_id", "profile", "params", "jwt_proof"],
  "properties": {
    "action_type": { "type": "string", "const": "rotation" },
    "action_id": { "type": "string", "description": "ULID/UUID" },
    "client_id": { "type": "string" },
    "profile": { "type": "string", "const": "nip-kr/0.1.0" },
    "params": {
      "type": "object",
      "required": ["rotation_reason", "not_before", "grace_duration_ms"],
      "properties": {
        "rotation_reason": { "type": "string" },
        "not_before": { "type": "integer", "minimum": 0, "description": "unix ms" },
        "grace_duration_ms": { "type": "integer", "minimum": 0 }
      },
      "additionalProperties": true
    },
    "jwt_proof": { "type": "string", "description": "compact JWS" }
  },
  "additionalProperties": true
}
```

Example future profiles
- Policy Update (Roster/Admin) — may leverage existing kind 450 for roster/policy; NIP-SERVICE can coordinate approvals and audit while the data-plane remains MLS or extant Nostr kinds.
- Escrow/Break-Glass (DR) — sealed secrets with quorum approvals; notify via MLS.
- Broadcast Notice — non-sensitive operational notices; notify via Nostr 40912 to admins, or MLS if scoping is required.

State Machine (Generic)
requested → prepared → notified → completed
           ↘ canceled | expired
           ↘ failed (on errors)

- requested: service-request accepted and authorized; audit entry created
- prepared: any pre-exec steps (e.g., generate artifacts, compute MACs)
- notified: notify delivered (MLS/Nostr); awaiting acks or completion criteria
- completed: success (profile-specific success conditions)
- canceled/expired: acks not received by deadline or manual cancel
- failed: unrecoverable error with reason code

Policies and Quorum
- Default quorum: 1 ack; configurable per client/classification.
- Deadlines: Default 30 minutes; auto-cancel on expiry.
- Rate limits: Per client and per requester; denylists enforced.

Security Considerations
- jwt_proof is REQUIRED for production (device/app integrity + TOTP + PoP).
- MLS membership checks provide defense in depth.
- Sensitive payloads must never be present in control-plane Nostr events or server logs.
- Key usage: Only “use” permissions (Sign/Verify) on KMS keys; no export.
- Observability: Only non-sensitive fields in logs/metrics; correlate with action_id/client_id.

Interoperability and Versioning
- NIP-SERVICE includes version tag ["nip-service", "0.1.0"] in tags.
- Profiles include their own versioning tags and should specify backwards compatibility behavior.
- Relay implementations SHOULD namespace kinds to avoid collisions until a public registry is finalized.

Kind Registry Guidance
- Suggested enterprise kinds: 40910 (service-request), 40911 (service-ack), 40912 (service-notify non-sensitive).
- Deployments MAY remap to their internal ranges; document mappings.

Test Guidance
- Unit: authZ (jwt_proof validation), MLS membership checks, idempotency by action_id.
- Integration: end-to-end request→notify→ack→completion for each profile.
- Security: ensure no plaintext in non-MLS paths; redaction checks in logs.

References
- NIP-KR: Rotation profile (nip-kr.md)
- MLS Protocol: RFC 9420
- Nostr Protocol: https://github.com/nostr-protocol/nostr
- JSON Web Token (JWT): RFC 7519
