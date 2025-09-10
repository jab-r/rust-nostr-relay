# NIP-KR MACSign/MACVerify and MLS Service Member Diagrams

This document visualizes:
- How the rust-nostr-relay control plane performs KMS-backed HMAC MACSign, writes hash-only metadata, and distributes plaintext via MLS to MLS admin group members (service-member model).
- How loxation-server validates presented client secrets using KMS MACVerify with grace-window semantics.

Conventions
- Canonical input: length-prefixed fields: len(client_id)||client_id||len(version_id)||version_id||len(secret)||secret (32-bit big-endian, UTF-8; no normalization).
- Encoding: base64url_no_padding for stored MACs and transported fields.
- mac_key_ref: exact KMS cryptoKeyVersion (preferred) or resolvable logical key label.

---

## Diagram 1 — Rotation flow: MACSign + DB + MLS Distribution (Service Member Model)

```mermaid
sequenceDiagram
  autonumber
  participant Admin as Admin App (MLS Member)
  participant Relay as rust-nostr-relay (Control Plane)
  participant KMS as KMS MAC Key (Sign)
  participant FS as Firestore (Hashes + Metadata)
  participant MLS as MLS Admin Group
  participant Svc as Relay MLS Service Member

  note over Admin,Relay: 1) Admin initiates rotation via rotate-request (Nostr 40901 + jwt_proof)
  Admin->>Relay: rotate-request {client_id, rotation_id, not_before, grace, jwt_proof}

  note over Relay: 2) Authorize: verify MLS membership + validate jwt_proof via JWKS
  Relay->>Relay: AuthZ OK (MLS group & jwt_proof)

  note over Relay,KMS: 3) Generate secret + version_id; build canonical input
  Relay->>Relay: secret=32B random (base64url_no_padding); version_id=ULID/UUID
  Relay->>Relay: data=len(client_id)||client_id||len(version_id)||version_id||len(secret)||secret
  Relay->>KMS: MACSign(mac_key_ref, data)
  KMS-->>Relay: secret_hash (base64url_no_padding)

  note over Relay,FS: 4) Prepare in Firestore (hash-only; no plaintext)
  Relay->>FS: Create oauth2_clients/{clientId}/secrets/{versionId} (state=pending, not_before, mac_key_ref, algo)
  Relay->>FS: Upsert oauth2_rotations/{rotationId} (quorum, not_before, grace_until)

  note over Relay,MLS: 5) Distribute via MLS to authorized admins (scoped by client_id)
  Relay->>Svc: Use service-member state (MlsClient) to compose MLS app msg
  Svc->>MLS: Post kind 445 (rotate-notify payload with plaintext secret + metadata)
  MLS-->>Admin: Encrypted delivery to MLS admin group members

  note over Admin,Relay: 6) Optional acks (MLS or Nostr 40902); quorum triggers promotion
  Admin-->>Relay: rotate-ack(s) (quorum>=required) [until ack_deadline]

  note over Relay,FS: 7) Promote (transaction): flip pointers atomically
  Relay->>FS: TX: set current_version=new; move old current->grace (not_after)
  Relay->>FS: Update oauth2_rotations outcome="promoted", completed_at

  note over MLS: Plaintext exists only in (encrypted) MLS payload; never in DB or logs
```

Key notes
- The Relay acts as an MLS service member (Svc), composing MLS application messages for the admin group. This enables secure, E2EE distribution of the plaintext secret to authorized admins only.
- Firestore persists only secret_hash and metadata (algo, mac_key_ref, windows, state); no plaintext ever persisted.
- Idempotency (rotation_id) and transactions (pointer flips) prevent races and ensure atomic promotion.

---

## Diagram 2 — Validation flow: loxation-server MACVerify with Grace Window

```mermaid
sequenceDiagram
  autonumber
  participant Ext as External Client
  participant API as loxation-server (Validation Plane)
  participant FS as DB (Read-only)
  participant KMS as KMS MAC Key (Verify)

  note over Ext,API: 1) External client presents client_id + client_secret (OAuth2 client_credentials or X-API)
  Ext->>API: {client_id, presented_secret}

  note over API,FS: 2) Resolve pointers and version docs (cache with listeners)
  API->>FS: Read oauth2_clients/{clientId} (current_version, previous_version)
  API->>FS: Read secrets/{versionId} docs (algo, mac_key_ref, secret_hash, not_before, not_after, state)

  note over API,KMS: 3) Compute canonical input and MACVerify against current; fallback to previous in grace
  API->>API: data=len(client_id)||client_id||len(version_id)||version_id||len(presented_secret)||presented_secret
  API->>KMS: MACVerify(mac_key_ref_current, data, secret_hash_current)
  KMS-->>API: success?/failure
  alt current failed AND previous within grace window
    API->>KMS: MACVerify(mac_key_ref_prev, data_prev, secret_hash_prev)
    KMS-->>API: success?/failure
  end

  note over API: 4) Enforce windows and policies
  API->>API: Check not_before/not_after (± skew), state ∈ {current,grace}
  API-->>Ext: Accept OR Reject (structured reason)

  note over API: Token stamping (if OAuth2): client_version_id in token; grace=0 enables immediate revoke of prior tokens
```

Key notes
- loxation-server only performs MACVerify; it never computes secret_hash for production flows.
- mac_key_ref is stored per-version to allow cryptographic agility and KMS key rotation.
- Grace semantics: If current fails and previous is within grace, verify previous; otherwise reject. Apply small clock skew tolerance.
- Observability: No plaintext logging. Log correlation fields (rotation_id if available), version used, and result codes.

---
