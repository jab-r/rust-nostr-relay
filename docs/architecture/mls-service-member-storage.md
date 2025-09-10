# MLS Service Member Storage for rust-nostr-relay

Status: Draft for review
Context: NIP-KR rotate-notify requires the relay to act as an MLS “service member” to publish MLS application messages to the admin group. The service member must persist MLS state across restarts.

## TL;DR

- Do NOT build a new “SQL provider for GCloud” right now.
- Persist the existing SQLite (SQLCipher-enabled) service-member database on a Cloud Run persistent volume mounted via GCS Fuse.
- Manage the SQLCipher key via Cloud KMS/Secret Manager at startup.
- This keeps effort low, is portable, and provides encryption-at-rest at the application layer. Cloud SQL is not equivalent to SQLCipher—they solve different problems.

## Why not Cloud SQL?

- The react_native_mls_rust crate’s storage is based on `openmls_sqlite_storage` (SQLite). Cloud SQL is a networked Postgres/MySQL; you cannot point SQLite at Cloud SQL.
- A Cloud SQL-based OpenMLS storage provider would require a new implementation similar in scope to `openmls_sqlite_storage`—significant engineering work.
- That work yields little ROI for the “service member” use case; we just need durable, local file-backed state.

## Why not ephemeral container FS?

- Cloud Run’s container filesystem is ephemeral. Without persistent storage, the relay would lose MLS state (keys, epochs, roster) across redeploys/scale-in.
- Stateless rejoin flows (e.g., reinvites) add operational friction and can delay rotations.

## Recommended: GCS Fuse Volume + SQLCipher

- Use Cloud Run’s native GCS volume mount (gcsfuse) to mount a bucket at a path the MLS service member will use as its storage directory (e.g., `/mnt/mls-service`).
- Continue using the crate’s SQLite provider (with `rusqlite` + `bundled-sqlcipher`):
  - Application-layer encryption-at-rest via SQLCipher.
  - Bucket also has provider-side encryption (defense in depth).
- Manage the SQLCipher key via KMS:
  - Store an encrypted key in Secret Manager, encrypted by KMS.
  - On startup, retrieve + decrypt into memory, and call `MlsClient::set_storage_key("relay", key)`.

### Security posture

- SQLCipher (application-layer) + Cloud Storage (provider-layer) encryption.
- Key material never stored in bucket; only loaded in memory on boot.
- Rotate SQLCipher key with a planned runbook (generate new key, migrate DB or rekey via app support when available).

## Implementation Steps

1) Cloud Run Volume
- Create a dedicated GCS bucket (e.g., `mls-service-state-<env>`).
- Configure Cloud Run Volume:
  - Type: GCS Fuse
  - Mount path: `/mnt/mls-service`
  - Read/Write

2) Service Account Permissions
- Allow the service to read the Secret Manager secret that contains the encrypted SQLCipher key.
- Allow decrypt via KMS on the key used to wrap the SQLCipher key.
- Allow read/write to the GCS bucket (objectAdmin scoped to this bucket).

3) Startup Flow (relay)
- Resolve MLS storage path:
  - `MlsClient::set_storage_path("/mnt/mls-service")`
- Load/decrypt SQLCipher key:
  - Read Secret Manager value (base64), call KMS Decrypt, zeroize buffers after use.
  - `MlsClient::set_storage_key("relay", <key>)`
- Proceed with normal initialization; the service member is now durable.

4) Config knobs (proposed `rnostr.toml`)
```toml
[extensions.nip_kr]
mls_service_storage_path = "/mnt/mls-service"
mls_service_sqlcipher_secret = "projects/…/secrets/mls-service-sqlcipher/versions/latest"
mls_service_sqlcipher_kms_key = "projects/…/locations/global/keyRings/…/cryptoKeys/…"
```

5) Backups & Retention
- Enable bucket versioning or periodic backups (optional).
- Guard access via IAM and org policies.

## FAQ

- Do we need a “SQL provider for gcloud”?
  - No. The crate uses SQLite (via `openmls_sqlite_storage`). Pointing this at Cloud SQL is not possible. Implementing a Postgres storage provider is feasible but unnecessary right now.

- Is Cloud SQL equivalent to SQLCipher?
  - No. Cloud SQL is a managed relational DB service (Postgres/MySQL). SQLCipher is application-layer encryption for SQLite. They serve different use cases and are not interchangeable.

- Can we use in-memory storage?
  - Only if we accept losing state on restart and implement auto-rejoin flows. Not recommended for a production rotation service.

- Performance considerations of GCS Fuse?
  - Adequate for small, infrequent MLS state updates. The service member writes are modest (group state mutations on rotation, sparse messaging). We can evaluate local SSD + snapshot if needed later.

- What about Firestore?
  - Firestore is already used for rotation metadata and pointers. Adapting OpenMLS storage to Firestore is a larger effort and unnecessary for this targeted need.

## Future Options

- Implement an OpenMLS storage provider backed by Postgres (Cloud SQL) if we ever need higher write throughput or strong transactional semantics beyond local state. This requires upstream coordination or a new crate.
- Use memory storage if/when the RN MLS module supports a deterministic stateless sender model (not typical for MLS application messaging).

## Minimal Code Sketch (where used)

```rust
use react_native_mls_rust::api::MlsClient;

fn init_service_member(client: &MlsClient, storage_path: &str, sqlcipher_key: &str) -> anyhow::Result<()> {
    client.set_storage_path(storage_path)?;
    client.set_storage_key("relay", sqlcipher_key)?;
    Ok(())
}

// Later, compose rotate-notify MLS message for admin group
let payload = serde_json::to_vec(&rotate_notify_json)?;
let ciphertext = client.create_application_message(admin_group_id, "relay", &payload)?;
```

## Decision

- Adopt GCS Fuse + SQLCipher approach for persistent, encrypted MLS service-member state in Cloud Run.
- Revisit a database-backed provider only if the above shows material operational issues.
