# MLS Secure Relay - High-Security Nostr Relay with MLS Gateway

A vendored deployment of [rnostr](https://github.com/rnostr/rnostr) enhanced with MLS Gateway Extension for high-security MLS-over-Nostr messaging. Designed to replace loxation-messaging infrastructure on Google Cloud Run.

## Features

- **Standards-Compliant Nostr Relay**: Full Nostr protocol support with WebSocket connectivity
- **MLS Gateway Extension**: 
  - MLS group messaging (kind 445) and Noise DM (kind 446) routing
  - Key package and welcome message mailbox services
  - Group registry (non-authoritative)
  - REST API endpoints for auxiliary flows
- **High-Security Architecture**:
  - NIP-42 authentication with pubkey allowlisting
  - IP-based access control via Cloud Armor
  - Isolated environment with no public federation
  - Cloud SQL integration for MLS metadata
- **Cloud-Native Deployment**:
  - Google Cloud Run with session affinity
  - Auto-scaling with LMDB consistency (single instance)
  - Cloud SQL PostgreSQL backend
  - Comprehensive observability

## Quick Start

### Prerequisites

- Google Cloud Project with billing enabled
- Cloud SQL PostgreSQL instance
- Docker and gcloud CLI installed
- Authentication configured: `gcloud auth login`

### Deployment

1. **Set Environment Variables**:
```bash
export GOOGLE_CLOUD_PROJECT="your-project-id"
export DATABASE_URL="postgresql://user:password@host/mls_gateway"
export INSTANCE_CONNECTION_NAME="project:region:instance"
export METRICS_AUTH_KEY="$(openssl rand -hex 32)"
```

2. **Deploy to Cloud Run**:
```bash
./scripts/deploy.sh --project-id $GOOGLE_CLOUD_PROJECT --sql-instance $INSTANCE_CONNECTION_NAME
```

3. **Configure Database Schema**:
```bash
# Connect to Cloud SQL and run the migrations
# The schema will be automatically created on first startup
```

## Architecture

```
┌─────────────────┐    WSS     ┌─────────────────┐
│   Nostr Client  │ ────────── │   Cloud Run     │
└─────────────────┘  NIP-42    │   rnostr Core   │
                                └─────────────────┘
                                         │
                                         ▼
                                ┌─────────────────┐
                                │ MLS Gateway Ext │
                                └─────────────────┘
                                         │
                                         ▼
                                ┌─────────────────┐
                                │  Cloud SQL      │
                                │  PostgreSQL     │
                                └─────────────────┘
```

### Event Flow

1. **Kind 445 (MLS Group Messages)**:
   - Extract `#h` (group ID) and `#e` (epoch) tags
   - Update group registry with metadata
   - Store event in LMDB for relay functionality

2. **Kind 446 (Noise DMs)**:
   - Extract `#p` (recipient) tags for routing
   - Content remains opaque (end-to-end encrypted)
   - Standard Nostr relay functionality

3. **Mailbox Services** (REST API):
   - `POST /api/v1/keypackages` - Store key packages
   - `GET /api/v1/keypackages?recipient=npub...` - Retrieve packages
   - `POST /api/v1/welcome` - Store welcome messages
   - `GET /api/v1/welcome?recipient=npub...` - Retrieve welcomes

## Configuration

### Environment Variables

| Variable | Description | Required |
|----------|-------------|----------|
| `GOOGLE_CLOUD_PROJECT` | GCP project ID | ✅ |
| `DATABASE_URL` | Cloud SQL connection string | ✅ |
| `INSTANCE_CONNECTION_NAME` | Cloud SQL instance identifier | ✅ |
| `METRICS_AUTH_KEY` | Authentication key for /metrics endpoint | ✅ |
| `RUST_LOG` | Log level (default: info) | ❌ |
| `ALLOWED_ORIGINS` | WebSocket CORS origins | ❌ |

### Security Configuration

Edit [`config/rnostr.toml`](config/rnostr.toml):

```toml
[auth.req]
ip_whitelist = ["10.0.0.0/8", "172.16.0.0/12"]

[auth.event]
pubkey_whitelist = ["npub1abc...", "npub1def..."]
event_pubkey_whitelist = ["npub1ghi..."]
```

## Migration from loxation-messaging

### Phase 1: Parallel Deployment
1. Deploy MLS Secure Relay alongside existing loxation-messaging
2. Configure test clients to connect to new relay
3. Validate feature parity and performance

### Phase 2: Client Migration
1. Update client applications to use Nostr protocol
2. Implement NIP-42 authentication in clients
3. Test MLS message routing and mailbox functionality

### Phase 3: Cutover
1. Update DNS/load balancer to point to new service
2. Monitor metrics and error rates
3. Decommission loxation-messaging after stability period

### Protocol Changes

| **loxation-messaging** | **MLS Secure Relay** |
|------------------------|----------------------|
| Custom WebSocket API | Standard Nostr Protocol |
| JWT token auth | NIP-42 cryptographic auth |
| Firebase/Firestore | LMDB + Cloud SQL |
| Custom message format | Nostr events (kinds 445/446) |

## Operations

### Monitoring

- **Service Health**: `GET /health` endpoint
- **Metrics**: `GET /metrics` (authenticated)
- **Logs**: `gcloud logs tail --follow projects/$PROJECT/services/mls-secure-relay`

### Key Metrics

- `mls_gateway_events_processed` - Events by kind
- `mls_gateway_groups_updated` - Group registry updates  
- `mls_gateway_keypackages_stored` - Mailbox deposits
- `nostr_relay_connections_active` - Active WebSocket connections

### Maintenance

#### Database Cleanup
```bash
# Cleanup expired mailbox items (runs automatically)
# Manual cleanup via SQL if needed:
DELETE FROM mail_keypackages WHERE expires_at < NOW();
DELETE FROM mail_welcomes WHERE expires_at < NOW();
```

#### Configuration Updates
```bash
# Update configuration and restart service
gcloud run services replace cloud-run-service.yaml
```

#### Scaling Considerations
- **Single Instance**: Required for LMDB consistency
- **Memory**: Monitor LMDB size, increase if needed
- **CPU**: Scale based on WebSocket connection load
- **Network**: Internal ingress with VPC connectivity

## Development

### Local Development
```bash
# Install dependencies
cargo build

# Run with local config
cargo run -- relay -c config/rnostr.toml

# Test MLS Gateway extension
curl http://localhost:8080/api/v1/groups
```

### Testing
```bash
# Run unit tests
cargo test

# Integration tests with database
DATABASE_URL=postgresql://localhost/test_mls cargo test
```

## Security Considerations

- **Network Isolation**: Deploy with internal ingress only
- **Authentication**: Require NIP-42 for all connections
- **Authorization**: Use pubkey allowlists for event publishing
- **Data Protection**: All message content remains encrypted and opaque
- **Secrets Management**: Use Cloud Secret Manager for sensitive data
- **Audit Logging**: Enable Cloud Audit Logs for compliance

## Troubleshooting

### Common Issues

1. **Database Connection Failed**
   - Verify Cloud SQL instance is running
   - Check `INSTANCE_CONNECTION_NAME` format
   - Ensure service account has Cloud SQL Client role

2. **WebSocket Connection Rejected**
   - Verify NIP-42 authentication implementation
   - Check pubkey allowlist configuration
   - Review IP allowlist settings

3. **High Memory Usage**
   - Monitor LMDB database size
   - Consider data retention policies
   - Scale Cloud Run memory allocation

4. **MLS Events Not Processing**
   - Check event kind filtering (445/446)
   - Verify tag extraction (`#h`, `#e`, `#p`)
   - Review Cloud SQL connection

### Support

For deployment issues:
1. Check Cloud Run logs: `gcloud logs tail --follow`
2. Verify configuration: `kubectl get service mls-secure-relay -o yaml`
3. Test database connectivity: Check Cloud SQL logs
4. Monitor metrics: Review `/metrics` endpoint

## License

This project inherits the license from the upstream rnostr project: MIT OR Apache-2.0
