# 🚀 MLS Gateway Deployment Guide

## 📋 **Quick Start**

Your environment is now configured for project `loxation-f8e1c`. Follow these steps to deploy:

```bash
# 1. Make sure you're authenticated with Google Cloud
gcloud auth login
gcloud auth application-default login

# 2. Deploy the service (loads .env automatically)
chmod +x scripts/deploy.sh
./scripts/deploy.sh
```

## 🔧 **Environment Configuration**

### ✅ **Files Created**
- **`.env`** - Your actual configuration (git-ignored)
- **`.env.example`** - Template for other developers
- **`.gitignore`** - Protects secrets from being committed

### 📊 **Current Configuration**
```bash
# Google Cloud
GOOGLE_CLOUD_PROJECT=loxation-f8e1c        # Your project ID
GOOGLE_CLOUD_PROJECT_NUMBER=696735170560   # Your project number

# MLS Gateway
MLS_GATEWAY_ENABLED=true                    # Enable MLS extension
MLS_GATEWAY_MESSAGE_ARCHIVE_ENABLED=true   # Offline message delivery
MLS_GATEWAY_MESSAGE_ARCHIVE_TTL_DAYS=30    # Message retention
```

## 🎯 **What the Deploy Script Does**

The [`scripts/deploy.sh`](scripts/deploy.sh) script:

1. **Loads Environment**: Reads from `.env` automatically
2. **Builds Container**: Creates optimized Docker image via Cloud Build
3. **Deploys to Cloud Run**: 
   - Service name: `loxation-messaging` (replaces existing)
   - Region: `us-central1`
   - Resources: 1 CPU, 1Gi memory
   - Instances: 1 (for LMDB consistency)
   - Uses your service account: `696735170560-compute@developer.gserviceaccount.com`

## 🔐 **Security Configuration**

### **Authentication Architecture**
- **NIP-42**: Handled by rust-nostr-relay core
- **Client Attestation**: Ready for integration with your REST server
- **No IP Restrictions**: Mobile-friendly (dynamic IPs)

### **Future Pubkey Allowlisting**
To restrict to only attested clients, edit [`config/rnostr.toml`](config/rnostr.toml):

```toml
[auth.event]
pubkey_whitelist = [
  "npub1your_attested_client_pubkey_1",
  "npub1your_attested_client_pubkey_2"
]
```

## 📦 **Message Archival System**

**Problem Solved**: Cloud Run restarts frequently → LMDB storage is ephemeral → offline users lose messages

**Solution**: 
- **Firestore Storage**: Persistent message archival
- **REST API**: `POST /api/v1/messages/missed` for catching up
- **Automatic Cleanup**: Expires after 30 days
- **Efficient Queries**: Indexed by recipient pubkey

## 🛠 **Deployment Commands**

### **Standard Deployment**
```bash
./scripts/deploy.sh
```

### **Custom Options**
```bash
./scripts/deploy.sh --project-id loxation-f8e1c --region us-central1 --service-name my-relay
```

### **Check Deployment Status**
```bash
# View logs
gcloud logs tail --follow projects/loxation-f8e1c/services/loxation-messaging

# Check service health
curl -f https://your-service-url/health
```

## 🔄 **Migration Strategy**

### **Phase 1: Parallel Deployment**
1. Deploy rust-nostr-relay alongside existing loxation-messaging
2. Use different service name temporarily: `mls-relay-test`

### **Phase 2: Client Testing**
1. Update client apps to support Nostr WebSocket protocol
2. Implement NIP-42 authentication in clients
3. Test with a subset of users

### **Phase 3: Traffic Cutover**
1. Update DNS/load balancer to point to rust-nostr-relay
2. Replace service name to `loxation-messaging`
3. Monitor metrics and performance

### **Phase 4: Cleanup**
1. Decommission old WebSocket service
2. Remove unused infrastructure

## 📊 **Monitoring & Observability**

### **Key Metrics**
- `mls_gateway_events_processed{kind="445"}` - MLS group messages
- `mls_gateway_events_processed{kind="446"}` - Noise DMs
- `mls_gateway_groups_updated` - Group registry updates

### **Health Checks**
- **Primary**: `GET /health` 
- **API**: `GET /api/v1/groups`
- **Archive**: `POST /api/v1/messages/missed`

### **Log Monitoring**
```bash
# Follow logs in real-time
gcloud logs tail --follow projects/loxation-f8e1c/services/loxation-messaging

# Filter for MLS events
gcloud logs read 'resource.type="cloud_run_revision" AND textPayload:"MLS"' \
  --project=loxation-f8e1c --limit=100
```

## 🧪 **Testing the Deployment**

### **1. Basic Connectivity**
```bash
# Test WebSocket connection
cargo run --bin test_deployment
```

### **2. API Endpoints**
```bash
# Test REST API
curl https://your-service-url/api/v1/groups
curl -X POST https://your-service-url/api/v1/messages/missed \
  -H "Content-Type: application/json" \
  -d '{"pubkey":"your-pubkey","since":1234567890,"limit":10}'
```

### **3. MLS Message Flow**
1. Send kind 445 (MLS group message) via WebSocket
2. Verify message is archived in Firestore
3. Query for missed messages via REST API
4. Confirm message is retrieved

## 🔧 **Firestore Configuration**

### **Required Indexes**
Deploy with:
```bash
gcloud firestore indexes create --file=firestore.indexes.json
```

### **Collections Used**
- `archived_events` - Message archival for offline delivery
- `mls_groups` - Group registry (non-authoritative)
- `mls_keypackages` - Key package mailbox
- `mls_welcomes` - Welcome message mailbox

## 🚨 **Troubleshooting**

### **Common Issues**

**Build Failures**
```bash
# If search dependencies fail, use no-default-features
cargo build --no-default-features --features mls_gateway_firestore
```

**Authentication Errors**
```bash
# Re-authenticate if needed
gcloud auth login
gcloud auth application-default login
```

**Firestore Permissions**
- Ensure service account has Firestore read/write permissions
- Check IAM roles: `roles/datastore.user`

## 🎯 **Next Steps**

1. **Deploy the Service**: Run `./scripts/deploy.sh`
2. **Test Basic Functionality**: Use the test script
3. **Update Client Apps**: Implement Nostr WebSocket + NIP-42
4. **Configure Allowlists**: Add trusted pubkeys when ready
5. **Monitor Performance**: Watch metrics and logs
6. **Plan Migration**: Schedule cutover from existing service

Your MLS Gateway is now ready for high-security messaging! 🚀