# High-Security MLS-over-Nostr Relay
Technical Specification for vendored rnostr (rust-nostr-relay) deployment on Google Cloud Run, with MLS gateway features and loxation-messenger replacement.

Author: react-native-mls
Status: Draft
Target: Relay/Platform teams

--------------------------------------------------------------------------------

## 1) Executive Summary

This document specifies a high-security relay solution based on a vendored copy of rnostr (rust-nostr-relay) to run in isolated environments. It replaces the legacy ../loxation-messenger websocket server. The solution:

- Uses rnostr as the core Nostr relay (LMDB storage, high performance, NIPs support).
- Adds an MLS Gateway Extension for MLS group messaging (kind 445) and Noise DMs (kind 446) features:
  - Group registry and metadata
  - Key package mailbox (onboarding)
  - Welcome mailbox (offline delivery)
  - Admin/rate limit/auth controls
  - Observability and compliance posture
- Is designed for deployment to Google Cloud Run, with security hardening, restricted ingress, and no federation with public relays unless explicitly configured.

Primary goal: Provide a self-contained relay service to route end-to-end encrypted MLS/Noise payloads, while surfacing non-secret metadata and ensuring strict access control without relaying to public Internet relays by default.

--------------------------------------------------------------------------------

## 2) Scope, Goals, and Non-Goals

Goals
- Replace ../loxation-messenger with a standards-compliant, high-performance relay.
- Support MLS group messaging (kind 445) and Noise DM (kind 446) routing semantics.
- Provide mailbox services (key packages, welcomes) and group registry (non-authoritative).
- Deploy on Cloud Run with IP allowlisting, auth, metrics, and resource autoscaling tuned for WS.
- Operate in high-security environments with no default federation to public relays.

Non-Goals
- The server does not decrypt any customer content (payloads remain opaque).
- The server does not provide full MLS control-plane; it only routes E2E-encrypted artifacts.
- The server does not enable public Nostr federation by default.

--------------------------------------------------------------------------------

## 3) Architecture Overview

Components
- rnostr core relay (vendored at ../rust-nostr-relay)
  - LMDB event storage
  - Nostr protocol and WebSocket handling
  - Extension mechanism for custom logic
- MLS Gateway Extension (new crate under rnostr/extensions/)
  - Intercepts events for kinds 445/446 to add routing hints
  - Exposes REST endpoints for mailboxes and group registry
  - Uses auxiliary SQLite for MLS-specific metadata
- Auxiliary SQLite DB (mls_auxiliary.db)
  - tables: groups, mail_keypackages, mail_welcomes
  - short TTL and cleanup processes for mailboxes
- Observability
  - Prometheus metrics (with auth key)
  - Structured logging (tracing)

Cloud Run Fit
- Containerized binary rnostr with config
- WSS supported; scale with min instances > 0 to reduce cold starts
- For LMDB durability and multi-instance, see deployment patterns below

--------------------------------------------------------------------------------

## 4) Deployment Model on Google Cloud Run

Recommended Patterns
- Single-instance (maxScale=1) for LMDB consistency without external dependencies.
  - Pros: Simpler; no additional infra
  - Cons: Availability limited to instance health; data durability on instance restart must be considered
- Multi-instance requires shared event store and pub/sub; rnostr currently uses local LMDB.
  - If multi-instance/high durability is required on Cloud Run:
    1) Prefer rethinking storage (e.g., a central DB), OR
    2) Run rnostr on GCE/GKE with persistent disks, OR
    3) Accept ephemeral storage with periodic export/import flows
- Data durability on Cloud Run:
  - Cloud Run’s filesystem is ephemeral across deployments and restarts.
  - If you must run on Cloud Run with LMDB:
    - Set autoscaling min/max to 1 (pin to single instance)
    - Keep instance warm with minScale
    - Schedule periodic exports (rnostr export) to GCS
    - Accept potential data loss across restarts; or pivot to GCE/GKE for persistence

Ingress/Network

- TLS via Cloud Run; clients use WSS

--------------------------------------------------------------------------------

## 5) Base Configuration (rnostr)

Create config/rnostr.toml (adjust per environment):

```toml
[information]
name = "MLS Secure Relay"
description = "High-security MLS-over-Nostr relay for isolated environments"
software = "https://github.com/rnostr/rnostr"

[data]
# LMDB path; on Cloud Run this is ephemeral unless on GCE/GKE
path = "./data"
db_query_timeout = "100ms"

[network]
# Cloud Run expects listening on 0.0.0.0
host = "0.0.0.0"
port = 8080

[limitation]
max_message_length = 1048576      # 1MB for MLS artifacts if needed
max_subscriptions = 50
max_filters = 20
max_limit = 1000
max_subid_length = 100
min_prefix = 10
max_event_tags = 5000
max_event_time_older_than_now = 94608000
max_event_time_newer_than_now = 900

[metrics]
enabled = true
auth = "replace_with_secure_metrics_key"

[auth]
enabled = true

[auth.req]
# Example: internal CIDRs only (manage via Cloud Armor as well)
# ip_whitelist = ["10.0.0.0/8", "172.16.0.0/12"]

[auth.event]
# High-security: only allow listed event authors and/or NIP-42-verified pubkeys
# pubkey_whitelist = ["npub1..."]
# event_pubkey_whitelist = ["npub1..."]

[rate_limiter]
enabled = true

[[rate_limiter.event]]
name = "mls_group_messages"
description = "MLS messages (kind 445)"
period = "1m"
limit = 100
kinds = [445]

[[rate_limiter.event]]
name = "noise_dms"
description = "Noise DMs (kind 446)"
period = "1m"
limit = 50
kinds = [446]

[count]
enabled = false

[search]
enabled = false
```

Operational notes
- Auth: Prefer NIP-42 authentication for request gating + pubkey allowlists for event authors
- Limitations: Tune limit/max values for your load profile
- Metrics: Protect /metrics with an auth key and keep it internal

--------------------------------------------------------------------------------

## 6) MLS Gateway Extension

Purpose
- Layer MLS/Noise routing on top of rnostr without breaking Nostr compliance
- Provide REST endpoints for auxiliary flows (mailboxes, registry)
- Keep payloads opaque; only index minimal tags for routing

Directory
```
../rust-nostr-relay/
  extensions/
    mls-gateway/
      Cargo.toml
      src/
        lib.rs
        endpoints.rs
        groups.rs
        mailbox.rs
        storage.rs
```

Example Cargo.toml (extension)
```toml
[package]
name = "mls-gateway"
version = "0.1.0"
edition = "2021"

[dependencies]
nostr-relay = { path = "../../relay" }
actix-web = "4"
sqlx = { version = "0.7", features = ["runtime-tokio-rustls", "sqlite", "macros", "chrono"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
chrono = { version = "0.4", features = ["serde"] }
tokio = { version = "1", features = ["full"] }
anyhow = "1.0"
tracing = "0.1"
```

Extension skeleton (illustrative; refer to nostr-relay extension APIs for exact traits):

```rust
// src/lib.rs
use actix_web::web::{self, ServiceConfig};
use nostr_relay::{Extension, Event, EventResult}; // Check types in relay/src/extension.rs

mod endpoints;
mod groups;
mod mailbox;
mod storage;

pub struct MLSGatewayExtension {
    aux: storage::AuxStore,
}

impl MLSGatewayExtension {
    pub async fn new(db_url: &str) -> anyhow::Result<Self> {
        let aux = storage::AuxStore::connect(db_url).await?;
        Ok(Self { aux })
    }

    // Mount extension-specific HTTP routes (scoped under /api/v1)
    pub fn configure_routes(cfg: &mut ServiceConfig) {
        cfg.service(
            web::scope("/api/v1")
                .route("/groups", web::get().to(endpoints::list_groups))
                .route("/groups/{id}", web::get().to(endpoints::get_group))
                .route("/keypackages", web::post().to(endpoints::post_keypackage))
                .route("/keypackages", web::get().to(endpoints::list_keypackages))
                .route("/welcome", web::post().to(endpoints::post_welcome))
                .route("/welcome", web::get().to(endpoints::list_welcomes)),
        );
    }
}

impl Extension for MLSGatewayExtension {
    fn name(&self) -> &str { "mls-gateway" }

    // Called on EVENTs; exact signature may differ; adapt to actual trait
    fn on_event(&self, ev: &Event) -> EventResult {
        match ev.kind {
            445 => {
                // Extract 'h' (group id), 'e' (epoch) tags if present
                // self.aux.upsert_group(...)
                EventResult::Continue
            }
            446 => {
                // Extract 'p' (recipient) tag if present
                EventResult::Continue
            }
            _ => EventResult::Continue,
        }
    }
}
```

Auxiliary storage (SQLite)
```rust
// src/storage.rs
use sqlx::{Pool, Sqlite, SqlitePool};

pub struct AuxStore {
    pub pool: Pool<Sqlite>,
}

impl AuxStore {
    pub async fn connect(url: &str) -> anyhow::Result<Self> {
        let pool = SqlitePool::connect(url).await?;
        sqlx::query("PRAGMA journal_mode=WAL;").execute(&pool).await.ok();
        Ok(Self { pool })
    }
}
```

Schema (initialize on startup)
```sql
CREATE TABLE IF NOT EXISTS groups (
  group_id TEXT PRIMARY KEY,
  display_name TEXT,
  avatar_url TEXT,
  owner_pubkey TEXT,
  last_epoch INTEGER,
  last_event_ts INTEGER,
  relays TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS mail_keypackages (
  id TEXT PRIMARY KEY,
  recipient_pubkey TEXT NOT NULL,
  sender_pubkey TEXT NOT NULL,
  content_b64 TEXT NOT NULL,
  tags TEXT,
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,
  picked_up_at INTEGER
);

CREATE TABLE IF NOT EXISTS mail_welcomes (
  id TEXT PRIMARY KEY,
  recipient_pubkey TEXT NOT NULL,
  sender_pubkey TEXT NOT NULL,
  group_id TEXT NOT NULL,
  welcome_b64 TEXT NOT NULL,
  ratchet_tree_b64 TEXT,
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,
  picked_up_at INTEGER
);

CREATE INDEX IF NOT EXISTS idx_groups_updated ON groups(updated_at);
CREATE INDEX IF NOT EXISTS idx_keypackages_recipient ON mail_keypackages(recipient_pubkey);
CREATE INDEX IF NOT EXISTS idx_welcomes_recipient ON mail_welcomes(recipient_pubkey);
```

REST endpoints (illustrative)
- GET /api/v1/groups
- GET /api/v1/groups/{id}
- POST /api/v1/groups (admin; optional)
- POST /api/v1/keypackages
- GET /api/v1/keypackages?recipient=npub1...&limit=50
- POST /api/v1/keypackages/{id}/ack
- POST /api/v1/welcome
- GET /api/v1/welcome?recipient=npub1...&limit=20
- POST /api/v1/welcome/{id}/ack

Notes
- Protect POST/ACK endpoints with HTTP auth (e.g., NIP-98-like signature) and rate limit
- TTL for mailboxes (e.g., 7–30 days), delete on pickup or expire
- All payloads remain opaque; the server never decrypts

--------------------------------------------------------------------------------

## 7) Build and Containerization

Dockerfile (multi-stage)
```dockerfile
# Builder
FROM rust:1.75 as builder
WORKDIR /app
# Copy vendored rnostr project into the build context (adjust as needed)
COPY . /app
# Build release binary
RUN cargo build --release

# Runtime
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /app/target/release/rnostr /app/
COPY config/ ./config/
# Optional: include entrypoint for templating environment into config
COPY docker-entrypoint.sh ./
EXPOSE 8080
ENTRYPOINT ["./docker-entrypoint.sh"]
```

Example docker-entrypoint.sh
```bash
#!/usr/bin/env bash
set -euo pipefail

# Optionally template config from env here (envsubst, yq/jq, etc.)
exec /app/rnostr relay -c /app/config/rnostr.toml --watch
```

--------------------------------------------------------------------------------

## 8) Cloud Run Service

Knative Service YAML (single-instance)
```yaml
apiVersion: serving.knative.dev/v1
kind: Service
metadata:
  name: mls-secure-relay
spec:
  template:
    metadata:
      annotations:
        autoscaling.knative.dev/minScale: "1"
        autoscaling.knative.dev/maxScale: "1"  # single instance for LMDB
        run.googleapis.com/execution-environment: gen2
        run.googleapis.com/cpu: "1000m"
        run.googleapis.com/memory: "1Gi"
    spec:
      containerConcurrency: 100
      containers:
      - image: gcr.io/PROJECT_ID/mls-secure-relay:latest
        ports:
        - containerPort: 8080
        env:
        - name: RUST_LOG
          value: "info"
```

CLI Deploy (example)
```bash
gcloud builds submit --tag gcr.io/$PROJECT_ID/mls-secure-relay
gcloud run deploy mls-secure-relay \
  --image gcr.io/$PROJECT_ID/mls-secure-relay \
  --region YOUR_REGION \
  --allow-unauthenticated=false \
  --min-instances 1 --max-instances 1 \
  --memory 1Gi --cpu 1 \
  --port 8080 \
  --ingress internal
```

Durability Advisory
- With LMDB on Cloud Run: storage is ephemeral. Either:
  - Accept ephemeral retention and periodically export to GCS
  - Or move to GCE/GKE with persistent disk for production-grade durability and/or multi-instance

--------------------------------------------------------------------------------

## 9) Security Hardening

- Ingress: internal only or behind Cloud Armor with strict allowlists
- TLS: Cloud Run-managed; clients use WSS
- NIP-42 auth for client sessions; enforce pubkey allowlists for publish
- Rate limiting per IP and per kind; stricter for 446 DMs
- Logging: exclude ciphertext; sample errors only; scrub PII
- RBAC/admin endpoints protected by service-to-service tokens and IP allowlists
- Egress: restrict outbound traffic; no federation unless explicitly enabled

--------------------------------------------------------------------------------

## 10) Migration from ../loxation-messenger

Phase 1: Parallel Run
- Deploy rnostr with MLS extension
- Configure a subset of clients to connect to the new gateway
- Validate feature parity and performance under representative load

Phase 2: Data Transition
- If the old server had persistent state, export required metadata (groups and mailboxes)
- Import into auxiliary SQLite via admin scripts or direct SQLx

Phase 3: Cutover and Decommission
- Update client endpoints to point to the new gateway
- Monitor errors, latency, rate limit hits
- Decommission loxation-messenger after stability window

--------------------------------------------------------------------------------

## 11) Observability and Operations

Metrics (Prometheus)
- Protect /metrics via auth= key
- Track:
  - Core: connections, events ingested, dedup, query times
  - MLS extension: mailbox deposits/pickups, group upserts
  - Rate limits and auth failures

Logging
- Structured logs with tracing
- Application-level spans for publish, fanout, backfill, and mailbox ops

Runbook
- Deploy/restart procedures via Cloud Run
- Export/import procedures (rnostr import/export)
- Rotating the metrics auth key and admin tokens
- Incident: high 429s (tune rate limits), auth failures (investigate key rotation), high latency (scale vCPU/memory or move to GCE)

--------------------------------------------------------------------------------

## 12) API Summary (MLS Gateway)

Mailbox: Key Packages
- POST /api/v1/keypackages
  - Body: { recipient, sender, content_b64, tags? }
  - Auth: Required (HTTP signature or service token)
  - Response: { ok: true, id }
- GET /api/v1/keypackages?recipient=npub1...&limit=50
  - Auth as recipient
  - Response: { ok: true, items: [ ... ] }
- POST /api/v1/keypackages/{id}/ack
  - Body: { recipient, sig }
  - Response: { ok: true }

Mailbox: Welcomes
- POST /api/v1/welcome
- GET /api/v1/welcome?recipient=npub1...&limit=20
- POST /api/v1/welcome/{id}/ack

Group Registry (non-authoritative)
- GET /api/v1/groups
- GET /api/v1/groups/{id}
- POST /api/v1/groups (admin; optional)

Backfill/Ordering
- Clients should use Nostr subscriptions/filters; MLS extension can optionally add
  cursor-based endpoints if needed in the future.

--------------------------------------------------------------------------------

## 13) Acceptance Criteria

- rnostr relay runs in Cloud Run with WSS, appropriate limits, and auth.
- MLS gateway endpoints function with appropriate TTLs and pickup semantics.
- Group registry surfaces non-secret metadata and epoch hints without conflicting with MLS authority.
- Observability is in place; metrics and logs usable for SRE.
- Security posture validated: IP allowlists, NIP-42, no default federation.

--------------------------------------------------------------------------------

## 14) Appendix

A) Example Env Vars
- RUST_LOG=info
- METRICS_AUTH_KEY=...
- AUX_DB_URL=sqlite:///app/data/mls_auxiliary.db

B) Performance Notes
- LMDB is very fast and memory-efficient; however ensure the container has sufficient memory
- For heavy backfill use-cases, consider increasing max_limit with caution

C) Future Enhancements
- Optional federation/outbox to selected private relays
- Redis/NATS PubSub if moving to multi-instance architectures (GKE/GCE)
- Postgres-backed auxiliary store for multi-instance consistency

--------------------------------------------------------------------------------

When you provide the primary gateway URL, we will add explicit endpoint examples, readiness/liveness checks, and any environment-specific settings to this specification.
