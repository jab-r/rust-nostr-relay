# MLS Gateway Extension - Developer Documentation

## Table of Contents

1. [Overview](#overview)
2. [Architecture](#architecture)
3. [Event Kinds & Protocol Compliance](#event-kinds--protocol-compliance)
4. [Configuration](#configuration)
5. [Deployment](#deployment)
6. [API Reference](#api-reference)
7. [Security Model](#security-model)
8. [Monitoring & Observability](#monitoring--observability)
9. [Development & Testing](#development--testing)
10. [Troubleshooting](#troubleshooting)
11. [Performance Considerations](#performance-considerations)

---

## Overview

The MLS Gateway Extension provides a complete implementation of **Message Layer Security (MLS) over Nostr** with **Noise Direct Messages** support for high-security messaging infrastructure. This extension transforms a standard Nostr relay into a sophisticated MLS-enabled communication hub with end-to-end encryption, group messaging, and enterprise-grade security features.

### Key Features

- 🔐 **Complete MLS Protocol Support**: KeyPackages, Welcome messages, Group messages
- 🎯 **Selective Message Delivery**: Group membership-based routing
- 🔒 **Noise Protocol DMs**: High-security direct messaging
- 🚀 **Cloud-Native**: Optimized for Google Cloud Run with Firestore backend
- 📊 **Production Monitoring**: Comprehensive metrics and logging
- 🔑 **NIP-42 Authentication**: Cryptographic client authentication
- 🛡️ **Security Hardening**: Pubkey allowlisting, rate limiting, expiry validation

---

## Architecture

### System Components

```
┌─────────────────────────────────────────────────────────────────┐
│                    MLS Gateway Extension                        │
├─────────────────────────────────────────────────────────────────┤
│  Event Processors                                              │
│  ┌─────────────┬─────────────┬─────────────┬─────────────────┐  │
│  │ KeyPackage  │ Giftwrap    │ MLS Group   │ Noise DM        │  │
│  │ (443)       │ (1059)      │ (445)       │ (446)           │  │
│  │ Validation  │ Onboarding  │ Messages    │ E2E Messages    │  │
│  └─────────────┴─────────────┴─────────────┴─────────────────┘  │
├─────────────────────────────────────────────────────────────────┤
│  Storage Layer                                                  │
│  ┌─────────────┬─────────────┬─────────────────────────────────┐ │
│  │ Firestore   │ Message     │ Group Registry                  │ │
│  │ Backend     │ Archive     │ & Membership                    │ │
│  └─────────────┴─────────────┴─────────────────────────────────┘ │
├─────────────────────────────────────────────────────────────────┤
│  Infrastructure                                                 │
│  ┌─────────────┬─────────────┬─────────────┬─────────────────┐  │
│  │ NIP-42      │ Rate        │ Metrics     │ REST API        │  │
│  │ Auth        │ Limiting    │ & Logging   │ Endpoints       │  │
│  └─────────────┴─────────────┴─────────────┴─────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
```

### Data Flow

```mermaid
sequenceDiagram
    participant Client as React Native Client
    participant Relay as MLS Relay
    participant Firebase as Firestore
    participant Archive as Message Archive

    Note over Client,Archive: MLS Onboarding Flow
    Client->>Relay: Publish KeyPackage (443)
    Relay->>Relay: Validate owner & expiry
    Relay->>Firebase: Store KeyPackage metadata
    
    Client->>Relay: Publish Giftwrap (1059)
    Relay->>Relay: Extract recipient & group_id
    Relay->>Firebase: Update group membership
    Relay->>Archive: Archive for offline delivery
    
    Note over Client,Archive: MLS Group Messaging
    Client->>Relay: Publish Group Message (445)
    Relay->>Relay: Selective delivery to members
    Relay->>Firebase: Update group registry
    Relay->>Archive: Archive message
    
    Note over Client,Archive: Noise Direct Messages
    Client->>Relay: Publish Noise DM (446)
    Relay->>Relay: Route to recipient only
    Relay->>Archive: Archive for offline delivery
```

---

## Event Kinds & Protocol Compliance

### Supported Nostr Event Kinds

| Kind | Purpose | Tags | Relay Rules |
|------|---------|------|-------------|
| **443** | MLS KeyPackage | `["p", owner]`, `["cs", ciphersuite]`, `["kpr", key_ref]`, `["exp", timestamp]` | Owner verification, expiry validation, public visibility |
| **444** | MLS Welcome (embedded) | None (inside 1059) | Never appears top-level, treated as opaque |
| **445** | MLS Group Message | `["h", group_id]`, `["k", epoch]`, `["p", hint]` | Group member-only delivery, registry updates |
| **446** | Noise Direct Message | `["p", recipient]`, `["v", "noise.1"]` | Recipient-only delivery, E2E encryption |
| **447** ⭐ | KeyPackage Request | `["p", target]`, `["h", group_id]`, `["cs", ciphersuite]`, `["min", count]`, `["ttl", seconds]` | System/admin only, recipient delivery, cross-relay interop |
| **450** ⭐ | Roster/Policy Control | `["h", group_id]`, `["seq", number]`, `["op", operation]`, `["p", members...]` | Admin-signed, monotonic sequence, deterministic membership |
| **1059** | Giftwrap Envelope | `["p", recipient]`, `["h", group_id]`, `["v", "gift.1"]` | Wraps 444 Welcome, membership management |

### Protocol Implementation Details

#### KeyPackage Validation (Kind 443)
```rust
// Security checks implemented:
1. Owner tag must match event pubkey
2. Expiry timestamp validation
3. Ciphersuite compatibility (logged)
4. Unique key reference tracking
```

#### Group Membership Management
```rust
// Membership sources (configurable):
Option 1: Sidecar ACL service (recommended)
Option 2: Implicit from Giftwrap (1059) events  
Option 3: Signed roster events (future)
```

#### Selective Delivery Rules
```rust
// Message routing logic:
- Kind 443: Public to authenticated users
- Kind 445: Group members only (by group_id)
- Kind 446: Recipient only (by p tag)
- Kind 447: Recipient only (by p tag), system/admin auth required
- Kind 450: Group members only, admin auth + sequence validation
- Kind 1059: Recipient only (by p tag)
```

#### New Event Kinds Implementation (447 & 450) ⭐

##### KeyPackage Request (Kind 447)
```rust
// Purpose: Cross-relay KeyPackage replenishment via Nostr
// Use case: When relay detects low KeyPackage stock or needs fresh keys

// Example event:
{
  "kind": 447,
  "pubkey": "<relay-system-key>",
  "created_at": 1724050000,
  "tags": [
    ["p", "<target_owner_pubkey>"],     // Required: recipient
    ["h", "grp_abc123"],                // Optional: group-specific request
    ["cs", "MLS_128_DHKEMX25519"],      // Optional: ciphersuite hint
    ["min", "5"],                       // Optional: minimum count requested
    ["ttl", "604800"]                   // Optional: request expiry (7 days)
  ],
  "content": "{\"reason\":\"stock_low\",\"note\":\"Group onboarding spike\"}"
}

// Security model:
- Only system_pubkey or admin_pubkeys can publish
- Recipient-only delivery (no broadcast)
- Short retention (1-7 days)
- Rate limiting per publisher
```

##### Roster/Policy Control (Kind 450)
```rust
// Purpose: Deterministic, admin-signed group membership management
// Use case: Explicit membership control with audit trail and idempotency

// Example event:
{
  "kind": 450,
  "pubkey": "<admin_pubkey>",
  "created_at": 1724050000,
  "tags": [
    ["h", "grp_abc123"],                // Required: group identifier
    ["seq", "17"],                      // Required: monotonic sequence number
    ["op", "add"],                      // Required: operation type
    ["p", "<member1_pubkey>"],          // Repeated: affected members
    ["p", "<member2_pubkey>"],
    ["role", "admin"],                  // Optional: role hint for promote/demote
    ["note", "Onboarding batch"]        // Optional: human context
  ],
  "content": "{\"batch_id\":\"onboard_2024_01\"}"
}

// Operations supported:
- "add": Add members to group
- "remove": Remove members from group
- "promote": Elevate member privileges
- "demote": Reduce member privileges
- "bootstrap": Initialize group with member list
- "replace": Replace entire member list atomically

// Security & Consistency:
- Only admin_pubkeys for specific group can publish
- Strictly monotonic sequence numbers per group_id
- Replay protection via sequence validation
- Long-term archival for audit/backfill
- Idempotent processing (duplicate sequences rejected)
```

---

## Configuration

### Core Configuration (`config/rnostr.toml`)

```toml
[server]
host = "0.0.0.0"
port = 8080
workers = 2

[auth]
enabled = true
pubkey_whitelist = [
    "your-allowed-pubkey-1",
    "your-allowed-pubkey-2"
]

[extensions.mls_gateway]
storage_backend = "firestore"
project_id = "your-gcp-project"
keypackage_ttl = 86400  # 24 hours
welcome_ttl = 3600      # 1 hour
enable_api = true
api_prefix = "/api/v1/mls"
enable_message_archive = true
message_archive_ttl_days = 30

# New configuration for kinds 447 (KeyPackage Request) and 450 (Roster/Policy)
# System pubkey for KeyPackage requests (optional - if not set, only admin_pubkeys can request)
# system_pubkey = "your_system_relay_pubkey_hex"

# Admin pubkeys allowed to send roster/policy events and KeyPackage requests
admin_pubkeys = [
    # Add your admin pubkeys here
    # "admin_pubkey_1_hex",
    # "admin_pubkey_2_hex"
]

# TTL for KeyPackage requests in seconds (default: 7 days)
keypackage_request_ttl = 604800

# TTL for roster/policy events in days (default: 365 days)
roster_policy_ttl_days = 365

[extensions.mls_gateway.rate_limits]
max_events_per_minute = 100
max_keypackages_per_hour = 10
max_group_messages_per_minute = 50
```

### Environment Variables

```bash
# Required
GOOGLE_CLOUD_PROJECT=your-project-id
FIREBASE_PROJECT_ID=your-project-id

# Optional
RUST_LOG=debug
LOG_LEVEL=debug
ALLOWED_ORIGINS=*

# Authentication
NOSTR_AUTH_REQUIRED=true
```

### Storage Backend Options

#### Firestore Backend (Recommended)
```toml
[extensions.mls_gateway]
storage_backend = "firestore"
project_id = "your-gcp-project"
```

#### Cloud SQL Backend (Optional)
```toml
[extensions.mls_gateway]
storage_backend = "cloudsql"
database_url = "postgresql://user:pass@host/db"
```

---

## Deployment

### Google Cloud Run Deployment

#### Prerequisites
```bash
# Install gcloud CLI
curl https://sdk.cloud.google.com | bash

# Authenticate
gcloud auth login
gcloud config set project your-project-id

# Enable required APIs
gcloud services enable run.googleapis.com
gcloud services enable cloudbuild.googleapis.com
gcloud services enable firestore.googleapis.com
```

#### Build and Deploy
```bash
# Build Docker image
docker build -t gcr.io/your-project/loxation-messaging:latest .

# Push to Google Container Registry
docker push gcr.io/your-project/loxation-messaging:latest

# Deploy to Cloud Run
gcloud run deploy loxation-messaging \
  --image gcr.io/your-project/loxation-messaging:latest \
  --platform managed \
  --region us-central1 \
  --service-account your-service-account@your-project.iam.gserviceaccount.com \
  --allow-unauthenticated \
  --port 8080 \
  --min-instances 1 \
  --max-instances 10 \
  --memory 1Gi \
  --cpu 1 \
  --concurrency 80 \
  --timeout 3600 \
  --set-env-vars="RUST_LOG=info,GOOGLE_CLOUD_PROJECT=your-project"
```

#### Required IAM Permissions
```json
{
  "bindings": [
    {
      "role": "roles/datastore.user",
      "members": ["serviceAccount:your-sa@project.iam.gserviceaccount.com"]
    },
    {
      "role": "roles/storage.objectViewer", 
      "members": ["serviceAccount:your-sa@project.iam.gserviceaccount.com"]
    }
  ]
}
```

### Local Development

```bash
# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Clone and build
git clone https://github.com/your-org/rust-nostr-relay
cd rust-nostr-relay
cargo build --release

# Run locally
GOOGLE_CLOUD_PROJECT=your-project ./target/release/rnostr relay -c config/rnostr.toml
```

---

## API Reference

### WebSocket API (Nostr Protocol)

#### Connect
```javascript
const ws = new WebSocket('wss://your-relay-url');
```

#### Authentication (NIP-42)
```javascript
// Handle auth challenge
ws.onmessage = (event) => {
  const [type, challenge] = JSON.parse(event.data);
  if (type === 'AUTH') {
    // Sign challenge and send AUTH event
    const authEvent = signAuthEvent(challenge, privateKey);
    ws.send(JSON.stringify(['AUTH', authEvent]));
  }
};
```

#### Publish Events
```javascript
// KeyPackage (443)
const keyPackage = {
  kind: 443,
  pubkey: 'your-pubkey',
  tags: [
    ['p', 'your-pubkey'],
    ['cs', 'MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519'],
    ['kpr', 'keypackage-reference'],
    ['exp', '1735689600']
  ],
  content: 'base64-encoded-keypackage',
  sig: 'signature'
};

ws.send(JSON.stringify(['EVENT', keyPackage]));
```

#### Subscribe to Events
```javascript
// Subscribe to MLS events
const subscription = {
  kinds: [443, 445, 446, 1059],
  limit: 100
};

ws.send(JSON.stringify(['REQ', 'sub-id', subscription]));
```

### REST API Endpoints

#### Health Check
```http
GET /health
Response: 200 OK
{
  "status": "healthy",
  "version": "0.4.8",
  "uptime_seconds": 3600
}
```

#### MLS Group Information
```http
GET /api/v1/mls/groups/{group_id}
Response: 200 OK
{
  "group_id": "grp_abc123",
  "display_name": "Team Chat",
  "member_count": 5,
  "last_epoch": 42,
  "created_at": "2025-01-01T00:00:00Z"
}
```

#### KeyPackage Retrieval
```http
GET /api/v1/mls/keypackages?owner={pubkey}
Response: 200 OK
{
  "keypackages": [
    {
      "owner": "pubkey-hex",
      "key_reference": "kpr_abc123",
      "ciphersuite": "MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519",
      "expires_at": "2025-01-02T00:00:00Z"
    }
  ]
}
```

---

## Security Model

### Authentication & Authorization

#### NIP-42 Challenge-Response
```rust
// Implementation details:
1. Server issues random challenge
2. Client signs challenge with private key
3. Server verifies signature against pubkey
4. Session authenticated for subsequent requests
```

#### Pubkey Allowlisting
```toml
[auth]
pubkey_whitelist = [
    "allowed-pubkey-1",
    "allowed-pubkey-2"
]
```

### Message Security

#### End-to-End Encryption
- **MLS Groups**: Forward secrecy, post-compromise security
- **Noise DMs**: XChaCha20-Poly1305 with Curve25519 key exchange
- **Welcome Messages**: MLS handshake protection

#### Selective Delivery
```rust
// Access control matrix:
Kind 443: Public (authenticated users)
Kind 445: Group members only  
Kind 446: Recipient only
Kind 1059: Recipient only
```

### Infrastructure Security

#### Cloud Run Security
```yaml
# Security configurations:
- No ingress from VPC
- HTTPS only (TLS 1.2+) 
- Service account with minimal permissions
- No persistent storage on container
- Automatic security updates
```

#### Firestore Security Rules
```javascript
rules_version = '2';
service cloud.firestore {
  match /databases/{database}/documents {
    match /mls_groups/{groupId} {
      allow read, write: if request.auth != null;
    }
    match /keypackages/{packageId} {
      allow read: if request.auth != null;
      allow write: if request.auth.uid == resource.data.owner;
    }
  }
}
```

---

## Monitoring & Observability

### Metrics (Prometheus Format)

#### Core Metrics
```
# Event processing
mls_gateway_events_processed_total{kind="443"} 1234
mls_gateway_events_processed_total{kind="445"} 5678
mls_gateway_events_processed_total{kind="446"} 2345
mls_gateway_events_processed_total{kind="1059"} 891

# Storage operations
mls_gateway_groups_updated_total 156
mls_gateway_keypackages_stored_total 234
mls_gateway_membership_updates_total 89

# Performance
mls_gateway_db_operation_duration_seconds_bucket{le="0.1"} 1234
mls_gateway_db_operation_duration_seconds_bucket{le="0.5"} 2345
```

#### Custom Metrics
```rust
// Add custom metrics in your application:
counter!("mls_gateway_custom_events", "custom" => "value").increment(1);
histogram!("mls_gateway_processing_time").record(duration);
```

### Logging

#### Structured Logging (JSON)
```json
{
  "timestamp": "2025-01-19T04:30:00Z",
  "level": "INFO", 
  "target": "mls_gateway::mod",
  "message": "Processing KeyPackage from owner: abc123",
  "fields": {
    "event_kind": 443,
    "owner": "abc123",
    "expiry": "2025-01-20T00:00:00Z"
  }
}
```

#### Log Levels
```bash
RUST_LOG=debug    # All debug info
RUST_LOG=info     # Production recommended  
RUST_LOG=warn     # Warnings and errors only
RUST_LOG=error    # Errors only
```

### Monitoring Queries

#### Key Performance Indicators
```promql
# Event processing rate
rate(mls_gateway_events_processed_total[5m])

# Error rate  
rate(mls_gateway_errors_total[5m]) / rate(mls_gateway_events_processed_total[5m])

# Average response time
rate(mls_gateway_db_operation_duration_seconds_sum[5m]) / 
rate(mls_gateway_db_operation_duration_seconds_count[5m])

# Group activity
increase(mls_gateway_groups_updated_total[1h])
```

#### Alerting Rules
```yaml
groups:
- name: mls_gateway_alerts
  rules:
  - alert: HighErrorRate
    expr: rate(mls_gateway_errors_total[5m]) > 0.1
    labels:
      severity: warning
    annotations:
      summary: "High error rate in MLS Gateway"
      
  - alert: SlowDatabase
    expr: histogram_quantile(0.95, mls_gateway_db_operation_duration_seconds_bucket) > 1.0
    labels:
      severity: warning
    annotations:
      summary: "Database operations are slow"
```

---

## Development & Testing

### Development Environment

#### Prerequisites
```bash
# Install Rust toolchain
rustup install stable
rustup default stable

# Install development tools
cargo install cargo-watch
cargo install cargo-audit
cargo install cargo-fmt
```

#### Local Setup
```bash
# Clone repository
git clone https://github.com/your-org/rust-nostr-relay
cd rust-nostr-relay

# Set up environment
cp .env.example .env
# Edit .env with your settings

# Install dependencies
cargo build

# Run tests
cargo test

# Run with auto-reload during development
cargo watch -x 'run -- relay -c config/rnostr.toml'
```

### Testing

#### Unit Tests
```bash
# Run all tests
cargo test

# Run specific extension tests
cargo test --package nostr-extensions

# Run with coverage
cargo tarpaulin --out Html
```

#### Integration Tests
```bash
# WebSocket functionality test
node test-websocket.js

# MLS protocol compliance test  
cargo test mls_protocol_compliance --features integration

# Performance benchmarks
cargo bench
```

#### Load Testing
```bash
# Install load testing tools
npm install -g artillery

# Run WebSocket load test
artillery run tests/load/websocket-test.yml

# Database performance test
cargo bench --bench storage_benchmark
```

### Code Quality

#### Linting and Formatting
```bash
# Format code
cargo fmt

# Check for issues
cargo clippy -- -D warnings

# Security audit
cargo audit

# Check dependencies
cargo outdated
```

#### Pre-commit Hooks
```bash
# Install pre-commit
pip install pre-commit

# Set up hooks
pre-commit install

# Run manually
pre-commit run --all-files
```

---

## Troubleshooting

### Common Issues

#### Build Failures

**Error: `firestore` dependency not found**
```bash
# Solution: Enable feature flag
cargo build --features mls_gateway_firestore
```

**Error: Missing system dependencies**
```bash
# Ubuntu/Debian
sudo apt-get install pkg-config libssl-dev

# macOS
brew install pkg-config openssl
```

#### Runtime Issues

**Error: `project_id required for Firestore backend`**
```bash
# Solution: Set environment variable
export GOOGLE_CLOUD_PROJECT=your-project-id
```

**Error: `Failed to connect to database`**
```bash
# Check Firestore permissions
gcloud firestore permissions list

# Verify service account key
export GOOGLE_APPLICATION_CREDENTIALS=/path/to/key.json
```

#### WebSocket Connection Issues

**Error: Connection refused**
```bash
# Check if service is running
curl -I https://your-relay-url/health

# Verify Cloud Run service status
gcloud run services describe loxation-messaging --region us-central1
```

**Error: Authentication failed**
```bash
# Verify NIP-42 implementation
# Check pubkey allowlist configuration
# Ensure proper signature format
```

### Performance Issues

#### High Memory Usage
```bash
# Monitor memory usage
docker stats

# Reduce concurrent connections
# Tune Firestore batch sizes
# Enable compression
```

#### Slow Database Operations
```bash
# Check Firestore metrics in Cloud Console
# Review indexing strategy
# Monitor concurrent operations
# Consider caching layer
```

### Debugging Tools

#### Application Logs
```bash
# View Cloud Run logs
gcloud logging read "resource.type=cloud_run_revision" --limit 100

# Local development logs
RUST_LOG=debug cargo run
```

#### Database Debugging
```bash
# Firestore operations monitoring
gcloud firestore operations list

# Query performance analysis  
# Use Cloud Console Firestore tab
```

#### Network Debugging
```bash
# Test WebSocket connection
websocat wss://your-relay-url

# Check TLS certificate
openssl s_client -connect your-relay-url:443

# Monitor network traffic
tcpdump -i any port 443
```

---

## Performance Considerations

### Scaling Guidelines

#### Horizontal Scaling
```yaml
# Cloud Run configuration
resource:
  limits:
    cpu: "2"
    memory: "2Gi"
autoscaling:
  minScale: 1
  maxScale: 100
  targetConcurrency: 80
```

#### Database Optimization
```javascript
// Firestore indexing strategy
{
  "indexes": [
    {
      "collectionGroup": "mls_groups",
      "fields": [
        {"fieldPath": "group_id", "order": "ASCENDING"},
        {"fieldPath": "last_updated", "order": "DESCENDING"}
      ]
    }
  ]
}
```

### Performance Benchmarks

#### Baseline Performance
```
Event Processing: 1,000 events/second/core
Database Operations: 500 ops/second/connection  
Memory Usage: ~50MB base + 1MB per 1000 concurrent connections
CPU Usage: ~10% per 1000 events/second
```

#### Optimization Techniques
```rust
// Connection pooling
max_connections = min(cpu_cores * 2, 20)

// Batch operations
firestore_batch_size = 500

// Caching
keypackage_cache_ttl = 300  // 5 minutes
group_cache_ttl = 60        // 1 minute
```

### Resource Planning

#### Capacity Planning
```
Expected Load: X events/second
Required Instances: X / 1000 (rounded up)
Database Connections: instances * 10  
Memory: instances * 200MB
```

#### Cost Optimization
```bash
# Use Cloud Run minimum instances strategically
min_instances = max(1, expected_baseline_load / 1000)

# Optimize Firestore usage
- Use batch operations
- Implement client-side caching
- Design efficient indexes
```

---

## Appendix

### Related Documentation
- [Nostr Protocol Specification](https://github.com/nostr-protocol/nips)
- [MLS RFC 9420](https://datatracker.ietf.org/doc/rfc9420/)
- [Noise Protocol Framework](https://noiseprotocol.org/)
- [Google Cloud Run Documentation](https://cloud.google.com/run/docs)

### Support and Community
- **Issues**: [GitHub Issues](https://github.com/your-org/rust-nostr-relay/issues)
- **Discussions**: [GitHub Discussions](https://github.com/your-org/rust-nostr-relay/discussions)
- **Security**: security@your-org.com

### License
This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

---

*Last updated: January 2025*
*Version: 1.0.0*