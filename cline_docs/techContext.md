# Technical Context — rust-nostr-relay

Languages, Frameworks, and Libraries
- Rust 2021 workspace
  - Core relay: rnostr (nostr-relay + nostr-db crates)
  - Actix ecosystem for web and actor-style extension hooks
- Extensions crate (nostr-extensions):
  - MLS Gateway (extensions/src/mls_gateway): event handling for kinds 443/444/445/446/447/450, registry, optional archival
  - NIP-SERVICE (extensions/src/nip_service): service-request/notify/ack plumbing and profile routing
- MLS layer (planned integration):
  - loxation_mls_mls_rust::api::MlsClient (OpenMLS-based) providing:
    - has_group(user_id, group_id) (fast in-memory membership)
    - decrypt/encrypt application messages
  - SQLCipher-backed SQLite storage for server-side MLS state (on Cloud Run with GCS Fuse)
- Cloud Services:
  - Firestore (GCP) for group registry metadata, audit, and KR store (planned)
  - Cloud KMS for HMAC-SHA-256 MACSign/Verify; mac_key_ref identifies cryptoKeyVersion

Key Specifications Implemented/Aligned
- NIP-SERVICE (internal spec):
  - MLS-first transport: kind 445 with ["h", group_id]; strictly non-sensitive outer tags
  - Profiles (first: NIP-KR) define JSON payload semantics and canonical encodings
  - Authorization: MLS membership + jwt_proof (attested JWS) (implementation pending)
  - Idempotency by action_id; audit lifecycle
- NIP-KR (rotation profile):
  - Canonical input (length-prefixed BE, UTF-8): be32(len(client_id)) || client_id || be32(len(version_id)) || version_id || be32(len(secret)) || secret
  - HMAC-SHA-256 MACSign with non-exportable key; base64url_no_padding encodings
  - Firestore pointers (current/previous), pending/promoted/grace/retired lifecycle
  - MLS rotate-notify distribution with plaintext only inside MLS E2EE
- Nostr kinds in use:
  - 443 KeyPackage, 444 Welcome (embedded), 445 MLS group message, 446 Noise DM
  - 447 KeyPackage Request, 450 Roster/Policy (admin-signed metadata)
  - 40910/40911 (NIP-SERVICE) reserved for dev/non-sensitive fallback only

Configuration and Feature Flags
- Cargo features:
  - mls_gateway (default): enables MLS Gateway with Firestore backend
  - nip_service (default): enables NIP-SERVICE extension
  - nip_service_mls: gates MLS-first decrypt path and service-member adapter
- MLS Gateway runtime configuration (MlsGatewayConfig):
  - enable_in_process_decrypt: bool (default true)
  - preferred_service_handler: "in-process" | "external" (default "in-process")
  - gating_use_registry_hint: bool (default false) — ops/UX hint only
  - mls_service_user_id: Option<String> — required to evaluate has_group(...) in-process
  - storage_backend: Firestore (default) | CloudSql (optional feature)
  - project_id, database_url, keypackage_ttl, welcome_ttl, admin_pubkeys, etc.
- Environment and TOML:
  - rnostr.toml [extensions.mls_gateway] should declare the above keys for production
  - Existing config parsing exists for other settings; MLS Gateway setting ingestion is pending wiring

Security Controls and Constraints
- No plaintext secrets persisted in DB/logs; only inside MLS payloads
- KMS keys are non-exportable; least privileged (Sign/Verify only)
- JWT verification (planned):
  - JWKS fetch/cache
  - Claims: aud/exp/iat/(nbf), amr includes app_attest+totp, PoP binding to npub
- Membership-first gating:
  - Authoritative gate that prevents decrypt attempts for groups where the service member has no in-memory state
  - Registry hints are non-authoritative and optional prefilters only

Data Storage Models (Firestore)
- mls_groups/{groupId}
  - owner_pubkey, admin_pubkeys, last_epoch
  - service_member (bool, advisory)
  - (Optional) service_handler override per group (in-process | external)
- oauth2_clients/{clientId}; oauth2_clients/{clientId}/secrets/{versionId}
  - MACed secret metadata, algo, mac_key_ref, not_before/not_after, state
- oauth2_rotations/{rotationId}
  - client_id, new_version, old_version, not_before, grace_until, quorum, outcome, audit fields

Build, Run, and Test
- Build: cargo build
- Example dev run: src/relay.rs uses App::create with config path (config/rnostr.toml) and registers extensions
- Testing (planned and partial):
  - Unit tests for db/kv exist; add tests for canonical encodings, store idempotency, and gating
  - Integration path: MLS-first service-request → NIP-SERVICE dispatcher (dev HMAC prepare) → (future) MLS notify → ack → promote

Performance and Reliability
- Fast in-memory membership gate ensures O(1) checks before any decrypt attempt
- Event processing in spawned tasks; counters and histograms added for observability
- In-process vs external handler selection allows operational trade-offs (latency vs isolation)

Known Gaps and Future Work
- RN MLS integration (has_group + decrypt/encrypt + storage)
- Config parsing for MlsGatewayConfig from rnostr.toml in Extension::setting
- Firestore-backed KR store and KMS client
- JWT/JWKS validator and MLS membership checks for requester authorization
- Roster/policy operations for advisory flags and group-level handler overrides
- Outbound notifier (MLS) for rotate-notify
- End-to-end tests and dashboards; SRE runbooks

Developer Notes
- All logging must be redacted; rely on action_id/client_id/version_id correlation only
- Always enforce idempotency by action_id in rotation flows
- Prefer base64url without padding for all relevant encodings
- Treat registry flags as hints, never as authorization or security gates
