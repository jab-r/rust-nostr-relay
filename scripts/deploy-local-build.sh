#!/bin/bash
set -euo pipefail

# Local build and deploy script for rust-nostr-relay
# This script builds the Docker image locally and pushes to GCR

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Function to display usage
show_help() {
    echo "Usage: $0 [OPTIONS]"
    echo ""
    echo "Deploy rust-nostr-relay to Google Cloud Run (with local Docker build)"
    echo ""
    echo "Options:"
    echo "  --project-id PROJECT_ID    GCP project ID (default: loxation-f8e1c)"
    echo "  --region REGION           GCP region (default: us-central1)"
    echo "  --service-name NAME       Cloud Run service name (default: loxation-messaging)"
    echo "  --tag TAG                Docker image tag (default: latest)"
    echo "  --help                   Show this help message"
    echo ""
    echo "Example:"
    echo "  $0 --project-id my-project --region us-east1"
}

# Default values
PROJECT_ID="loxation-f8e1c"
REGION="us-central1"
SERVICE_NAME="loxation-messaging"
TAG="latest"
NAMESPACE="696735170560"
SERVICE_ACCOUNT="696735170560-compute@developer.gserviceaccount.com"

# Parse command line arguments
while [[ $# -gt 0 ]]; do
  case $1 in
    --project-id)
      PROJECT_ID="$2"
      shift 2
      ;;
    --region)
      REGION="$2"
      shift 2
      ;;
    --service-name)
      SERVICE_NAME="$2"
      shift 2
      ;;
    --tag)
      TAG="$2"
      shift 2
      ;;
    --help)
      show_help
      exit 0
      ;;
    *)
      echo "Unknown option: $1"
      show_help
      exit 1
      ;;
  esac
done

# Validate deployment configuration
echo "Validating deployment configuration..."

# Check if gcloud is configured
if ! gcloud config get-value project &> /dev/null; then
    echo -e "${RED}Error: gcloud is not configured. Run 'gcloud init' first.${NC}"
    exit 1
fi

# Set the project
gcloud config set project ${PROJECT_ID} || exit 1

# Check Docker is available
if ! command -v docker &> /dev/null; then
    echo -e "${RED}Error: Docker is not installed or not in PATH${NC}"
    exit 1
fi

# Check if Docker daemon is running
if ! docker info &> /dev/null; then
    echo -e "${RED}Error: Docker daemon is not running${NC}"
    exit 1
fi

# Configure Docker authentication for GCR
echo "Configuring Docker authentication for Google Container Registry..."
gcloud auth configure-docker gcr.io --quiet

echo ""
echo -e "${GREEN}Deploying MLS Secure Relay (Local Build)...${NC}"
echo "Project: ${PROJECT_ID}"
echo "Region: ${REGION}"
echo "Service: ${SERVICE_NAME}"
echo "Tag: ${TAG}"
echo "Storage: Firestore"

# Build Docker image locally
IMAGE_NAME="gcr.io/${PROJECT_ID}/${SERVICE_NAME}:${TAG}"
echo ""
echo -e "${YELLOW}Building Docker image locally...${NC}"

# Create a temporary build context that includes loxation-mls
BUILD_DIR=$(mktemp -d)
echo "Creating temporary build context at ${BUILD_DIR}"

# Copy the rust-nostr-relay project, excluding unnecessary files
echo "Copying rust-nostr-relay project (excluding build artifacts)..."
rsync -av \
    --exclude='.git' \
    --exclude='target' \
    --exclude='node_modules' \
    --exclude='*.log' \
    --exclude='data' \
    --exclude='.DS_Store' \
    . "${BUILD_DIR}/rust-nostr-relay/"

# Copy only the loxation-mls/rust dependency (that's all we need)
if [ -d "../loxation-mls/rust" ]; then
    echo "Copying loxation-mls/rust dependency..."
    mkdir -p "${BUILD_DIR}/loxation-mls"
    rsync -av \
        --exclude='target' \
        --exclude='.git' \
        --exclude='*.log' \
        ../loxation-mls/rust/ "${BUILD_DIR}/loxation-mls/rust/"
else
    echo -e "${RED}Error: loxation-mls/rust directory not found at ../loxation-mls/rust${NC}"
    rm -rf "${BUILD_DIR}"
    exit 1
fi

# Show size of build context
echo "Build context size:"
du -sh "${BUILD_DIR}"/*

# Create a modified Dockerfile that handles the correct paths
cat > "${BUILD_DIR}/Dockerfile" << 'EOF'
# Multi-stage build for rust-nostr-relay with MLS Gateway Extension
# Modified for local build context

# Build stage
FROM rust:1.89-bookworm AS builder

# Install system dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy loxation-mls first (for dependency)
COPY loxation-mls /loxation-mls

# Copy rust-nostr-relay source
COPY rust-nostr-relay .

# Update the Cargo.toml to use the copied loxation-mls path
RUN sed -i 's|path = "/Users/jon/Documents/GitHub/loxation-mls/rust"|path = "/loxation-mls/rust"|g' extensions/Cargo.toml

# Build the application with MLS features
RUN cargo build --release --bin rnostr --no-default-features --features mls_gateway_firestore

# Runtime stage
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Create app user for security
RUN useradd -r -u 1001 -g root appuser

WORKDIR /app

# Copy the binary from builder stage
COPY --from=builder /app/target/release/rnostr ./

# Copy configuration files
COPY --from=builder /app/config/ ./config/

# Create directory for LMDB database
RUN mkdir -p ./data && chown appuser:root ./data

# Set permissions
RUN chown appuser:root ./rnostr && chmod +x ./rnostr

# Switch to non-root user
USER appuser

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:8080/health || exit 1

# Expose port
EXPOSE 8080

# Set environment variables
ENV RUST_LOG=info
ENV RNOSTR_CONFIG_PATH=./config/rnostr.toml

# Run the application
CMD ["./rnostr", "relay", "-c", "./config/rnostr.toml"]
EOF

# Build the Docker image for x86_64 (Cloud Run requirement)
cd "${BUILD_DIR}"
echo "Building for x86_64 architecture (required for Cloud Run)..."
docker buildx build --platform linux/amd64 -t "${IMAGE_NAME}" -f Dockerfile . --load

# Clean up build directory
cd - > /dev/null
rm -rf "${BUILD_DIR}"

# Push the image to GCR
echo ""
echo -e "${YELLOW}Pushing image to Google Container Registry...${NC}"
docker push "${IMAGE_NAME}"

# Deploy to Cloud Run
echo ""
echo -e "${YELLOW}Deploying to Cloud Run...${NC}"

DEPLOY_CMD="gcloud run deploy ${SERVICE_NAME} \
  --image ${IMAGE_NAME} \
  --platform managed \
  --region ${REGION} \
  --project ${PROJECT_ID} \
  --service-account ${SERVICE_ACCOUNT} \
  --allow-unauthenticated \
  --port 8080 \
  --min-instances 1 \
  --max-instances 1 \
  --memory 1Gi \
  --cpu 1 \
  --concurrency 80 \
  --timeout 3600 \
  --cpu-boost \
  --session-affinity \
  --no-use-http2 \
  --ingress all"

# Add environment variables (matching existing loxation-messaging setup)
ENV_VARS="RUST_LOG=debug"
ENV_VARS="${ENV_VARS},GOOGLE_CLOUD_PROJECT=${PROJECT_ID}"
ENV_VARS="${ENV_VARS},FIREBASE_PROJECT_ID=${PROJECT_ID}"
ENV_VARS="${ENV_VARS},ALLOWED_ORIGINS=*"
ENV_VARS="${ENV_VARS},LOG_LEVEL=debug"

# Add optional environment variables
if [[ -n "${METRICS_AUTH_KEY:-}" ]]; then
    ENV_VARS="${ENV_VARS},METRICS_AUTH_KEY=${METRICS_AUTH_KEY}"
fi

# Add all environment variables to the command
DEPLOY_CMD="${DEPLOY_CMD} --set-env-vars=\"${ENV_VARS}\""

# Deploy to Cloud Run
eval $DEPLOY_CMD

# Get the service URL
SERVICE_URL=$(gcloud run services describe ${SERVICE_NAME} \
  --platform managed \
  --region ${REGION} \
  --project ${PROJECT_ID} \
  --format 'value(status.url)')

echo ""
echo -e "${GREEN}✅ Main service deployment complete!${NC}"

# Deploy Cloud Run Job for keypackage cleanup
echo ""
echo -e "${YELLOW}Setting up Cloud Run Job for keypackage cleanup...${NC}"

JOB_NAME="${SERVICE_NAME}-cleanup"
SCHEDULE_NAME="${JOB_NAME}-schedule"

# Create or update the Cloud Run Job
echo "Creating/updating Cloud Run Job: ${JOB_NAME}"
gcloud run jobs replace ${JOB_NAME} \
  --image ${IMAGE_NAME} \
  --args "cleanup" \
  --region ${REGION} \
  --project ${PROJECT_ID} \
  --service-account ${SERVICE_ACCOUNT} \
  --parallelism 1 \
  --task-timeout 5m \
  --max-retries 1 \
  --memory 512Mi \
  --cpu 1 \
  --set-env-vars="${ENV_VARS}" \
  2>/dev/null || \
gcloud run jobs create ${JOB_NAME} \
  --image ${IMAGE_NAME} \
  --args "cleanup" \
  --region ${REGION} \
  --project ${PROJECT_ID} \
  --service-account ${SERVICE_ACCOUNT} \
  --parallelism 1 \
  --task-timeout 5m \
  --max-retries 1 \
  --memory 512Mi \
  --cpu 1 \
  --set-env-vars="${ENV_VARS}"

# Get the job URI for the scheduler
JOB_URI="https://${REGION}-run.googleapis.com/apis/run.googleapis.com/v1/namespaces/${NAMESPACE}/jobs/${JOB_NAME}:run"

# Create or update the Cloud Scheduler job (runs weekly on Sunday at 2 AM ET)
echo "Setting up Cloud Scheduler: ${SCHEDULE_NAME}"
gcloud scheduler jobs update http ${SCHEDULE_NAME} \
  --location ${REGION} \
  --schedule "0 2 * * 0" \
  --time-zone "America/New_York" \
  --uri "${JOB_URI}" \
  --http-method POST \
  --oauth-service-account-email ${SERVICE_ACCOUNT} \
  2>/dev/null || \
gcloud scheduler jobs create http ${SCHEDULE_NAME} \
  --location ${REGION} \
  --schedule "0 2 * * 0" \
  --time-zone "America/New_York" \
  --uri "${JOB_URI}" \
  --http-method POST \
  --oauth-service-account-email ${SERVICE_ACCOUNT}

echo -e "${GREEN}✅ Cloud Run Job and Scheduler configured successfully${NC}"

echo ""
echo "Service URL: ${SERVICE_URL}"
echo "Cleanup job: ${JOB_NAME} (runs daily at 2 AM ET)"
echo ""
echo "Next steps:"
echo "1. Deploy Firestore indexes: firebase deploy --only firestore:indexes"
echo "2. Test the WebSocket endpoint: ${SERVICE_URL}"
echo "3. Monitor logs: gcloud logs tail --project ${PROJECT_ID}"
echo "4. Test cleanup job: gcloud run jobs execute ${JOB_NAME} --region ${REGION}"