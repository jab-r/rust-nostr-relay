# Deployment Checklist - MLS Secure Relay

Complete this checklist before deploying the MLS Secure Relay to production.

## ✅ Pre-Deployment Requirements

### 1. Google Cloud Project Setup

- [ ] **Project ID**: `_____________________`
- [ ] **Billing Enabled**: Verify billing is enabled on the project
- [ ] **Required APIs Enabled**:
  - [ ] Cloud Run API
  - [ ] Cloud SQL API  
  - [ ] Container Registry API
  - [ ] Cloud Build API
  - [ ] Secret Manager API
  - [ ] Cloud Logging API
  - [ ] Cloud Monitoring API

### 2. Cloud SQL PostgreSQL Instance

- [ ] **Instance Name**: `_____________________`
- [ ] **Region**: `_____________________` (should match Cloud Run region)
- [ ] **Database Name**: `mls_gateway` (recommended)
- [ ] **Connection Name**: `PROJECT_ID:REGION:INSTANCE_NAME`
- [ ] **Database User**: `_____________________`
- [ ] **Database Password**: `_____________________` (store in Secret Manager)
- [ ] **Public IP**: Disabled (recommended)
- [ ] **Private IP**: Enabled with VPC network
- [ ] **Authorized Networks**: Configure if using public IP

### 3. Environment Variables to Configure

#### Required Variables
```bash
export GOOGLE_CLOUD_PROJECT="your-project-id"
export DATABASE_URL="postgresql://username:password@host:5432/mls_gateway"
export INSTANCE_CONNECTION_NAME="project-id:region:instance-name"
export METRICS_AUTH_KEY="$(openssl rand -hex 32)"
```

#### Optional Variables
```bash
export ALLOWED_ORIGINS="https://yourdomain.com,https://app.yourdomain.com"
export RUST_LOG="info"  # or debug for verbose logging
```

### 4. Security Configuration

#### NIP-42 Authentication Setup
- [ ] **Event Publishing Allowlist**: List of npub keys allowed to publish events
  ```toml
  # Add to config/rnostr.toml
  [auth.event]
  pubkey_whitelist = [
    "npub1abc123...",  # Replace with actual pubkeys
    "npub1def456...",
    # Add more as needed
  ]
  ```

#### Secrets Management
- [ ] **Database Password**: Store in Google Secret Manager
  ```bash
  gcloud secrets create mls-gateway-db-password --data-file=db_password.txt
  ```
- [ ] **Metrics Auth Key**: Store in Google Secret Manager
  ```bash
  echo -n "your-metrics-key" | gcloud secrets create mls-gateway-metrics-key --data-file=-
  ```

### 5. Network Configuration

- [ ] **VPC Network**: `_____________________` (if using private Cloud SQL)
- [ ] **Subnet**: `_____________________`
- [ ] **VPC Connector**: `_____________________` (for Cloud Run to VPC access)
- [ ] **Firewall Rules**: Configure if needed for Cloud SQL access

### 6. DNS and Load Balancing (Optional)

- [ ] **Custom Domain**: `_____________________`
- [ ] **SSL Certificate**: Configure managed SSL certificate
- [ ] **Load Balancer**: Set up if using custom domain

## 🔧 Configuration Files to Update

### 1. Update [`config/rnostr.toml`](config/rnostr.toml)

Replace placeholders:
- [ ] `pubkey_whitelist = ["npub1..."]` - Add your authorized pubkeys
- [ ] `event_pubkey_whitelist = ["npub1..."]` - Add event publishing pubkeys
- [ ] Review rate limits for your use case
- [ ] Adjust memory and performance settings if needed

### 2. Update [`cloud-run-service.yaml`](cloud-run-service.yaml)

Replace placeholders:
- [ ] `PROJECT_ID` - Your Google Cloud project ID
- [ ] `REGION` - Your deployment region  
- [ ] `INSTANCE_NAME` - Your Cloud SQL instance name
- [ ] `CONNECTOR_NAME` - Your VPC connector name (if using)

### 3. Update [`scripts/deploy.sh`](scripts/deploy.sh)

Verify default values match your environment:
- [ ] `REGION="us-central1"` - Change if using different region
- [ ] `SERVICE_NAME="mls-secure-relay"` - Change if desired
- [ ] Review deployment parameters

## 🚀 Deployment Steps

### 1. Authentication and Setup
```bash
# Authenticate with Google Cloud
gcloud auth login
gcloud auth application-default login

# Set project
gcloud config set project YOUR_PROJECT_ID

# Enable required APIs
gcloud services enable run.googleapis.com
gcloud services enable sql.googleapis.com
gcloud services enable cloudbuild.googleapis.com
gcloud services enable secretmanager.googleapis.com
```

### 2. Create Cloud SQL Instance
```bash
# Example - adjust for your needs
gcloud sql instances create mls-gateway-db \
  --database-version=POSTGRES_15 \
  --tier=db-f1-micro \
  --region=us-central1 \
  --storage-type=SSD \
  --storage-size=10GB
```

### 3. Create Database and User
```bash
# Create database
gcloud sql databases create mls_gateway --instance=mls-gateway-db

# Create user (replace password)
gcloud sql users create mls_user \
  --instance=mls-gateway-db \
  --password=YOUR_SECURE_PASSWORD
```

### 4. Store Secrets
```bash
# Store database connection string
echo -n "postgresql://mls_user:YOUR_PASSWORD@/mls_gateway?host=/cloudsql/YOUR_PROJECT:us-central1:mls-gateway-db" | \
  gcloud secrets create mls-gateway-db-url --data-file=-

# Store metrics auth key
openssl rand -hex 32 | gcloud secrets create mls-gateway-metrics-key --data-file=-
```

### 5. Deploy the Service
```bash
# Set environment variables
export GOOGLE_CLOUD_PROJECT="your-project-id"
export DATABASE_URL="postgresql://mls_user:password@/mls_gateway?host=/cloudsql/your-project:region:instance"
export INSTANCE_CONNECTION_NAME="your-project:region:instance"

# Deploy
./scripts/deploy.sh
```

## 🔍 Post-Deployment Verification

### 1. Service Health
- [ ] Service deployed successfully
- [ ] Health endpoint responding: `curl $SERVICE_URL/health`
- [ ] Logs show no errors: `gcloud logs tail --follow`

### 2. Database Connection
- [ ] Cloud SQL connection established
- [ ] Database schema created automatically
- [ ] No connection errors in logs

### 3. Authentication Testing
- [ ] WebSocket connections require NIP-42 auth
- [ ] Unauthorized connections are rejected
- [ ] Authorized pubkeys can publish events

### 4. MLS Gateway Testing
- [ ] REST API endpoints accessible: `curl $SERVICE_URL/api/v1/groups`
- [ ] Kind 445/446 events are processed
- [ ] Group registry updates working
- [ ] Mailbox services functional

### 5. Monitoring Setup
- [ ] Metrics endpoint protected: `curl $SERVICE_URL/metrics` (should require auth)
- [ ] Cloud Monitoring alerts configured
- [ ] Log-based metrics configured
- [ ] Error reporting enabled

## 📋 Environment-Specific Notes

### Development Environment
- [ ] Use smaller Cloud SQL instance (db-f1-micro)
- [ ] Enable debug logging: `RUST_LOG=debug`
- [ ] Use test pubkeys in allowlist
- [ ] Consider using public IP for easier access

### Production Environment  
- [ ] Use appropriately sized Cloud SQL instance
- [ ] Enable private IP only for Cloud SQL
- [ ] Configure VPC connector for secure access
- [ ] Set up monitoring and alerting
- [ ] Configure backup policies
- [ ] Review security settings thoroughly

## 🆘 Troubleshooting

### Common Configuration Issues
- **Database connection fails**: Check Cloud SQL instance status and credentials
- **WebSocket connections rejected**: Verify NIP-42 implementation and pubkey allowlist
- **502 errors**: Check service logs and ensure proper health check response
- **High memory usage**: Monitor LMDB size and adjust Cloud Run memory allocation

### Support Resources
- **Cloud Run Logs**: `gcloud logs tail --follow projects/$PROJECT/services/mls-secure-relay`
- **Cloud SQL Logs**: Check in Cloud Console
- **Health Check**: `curl $SERVICE_URL/health`
- **Metrics**: `curl -H "Authorization: Bearer $METRICS_AUTH_KEY" $SERVICE_URL/metrics`

---

**✨ Ready to Deploy?**

Once all items are checked and configured, run:
```bash
./scripts/deploy.sh --project-id $GOOGLE_CLOUD_PROJECT --sql-instance $INSTANCE_CONNECTION_NAME