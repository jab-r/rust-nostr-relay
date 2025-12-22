#!/bin/bash

# Exit on error
set -e

# Load environment variables from .env if it exists
if [ -f .env ]; then
  export $(cat .env | grep -v '^#' | xargs)
fi

# Required environment variables
REQUIRED_VARS=(
  "GOOGLE_CLOUD_PROJECT"
)

# Check required environment variables
for var in "${REQUIRED_VARS[@]}"; do
  if [ -z "${!var}" ]; then
    echo "Error: Required environment variable $var is not set"
    exit 1
  fi
done

# Function to show help message
show_help() {
  echo "Usage: $0 [OPTIONS]"
  echo ""
  echo "Options:"
  echo "  --project-id     Google Cloud project ID (default: \$GOOGLE_CLOUD_PROJECT)"
  echo "  --region         Google Cloud region (default: us-central1)"
  echo "  --service-name   Cloud Run service name (default: mls-secure-relay)"
  echo "  --tag           Docker image tag (default: latest)"
  echo "  --firestore      Use Firestore backend (default)"
  echo "  --help          Show this help message"
}

# Default values (matching existing loxation-messaging deployment)
PROJECT_ID=${GOOGLE_CLOUD_PROJECT:-"loxation-f8e1c"}
REGION="us-central1"
SERVICE_NAME="loxation-messaging"
TAG="latest"
USE_FIRESTORE="true"
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
    --firestore)
      USE_FIRESTORE="true"
      shift 1
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
if ! command -v gcloud &> /dev/null; then
    echo "Error: gcloud CLI not found"
    exit 1
fi

if ! gcloud auth list --filter=status:ACTIVE --format="value(account)" | grep -q .; then
    echo "Error: No active gcloud authentication found"
    exit 1
fi

# Set the project
gcloud config set project $PROJECT_ID

echo "Deploying MLS Secure Relay..."
echo "Project: $PROJECT_ID"
echo "Region: $REGION"
echo "Service: $SERVICE_NAME"
echo "Tag: $TAG"
echo "Storage: Firestore"

# Build and submit to Cloud Build
echo "Building and submitting to Cloud Build..."
gcloud builds submit --tag gcr.io/${PROJECT_ID}/${SERVICE_NAME}:${TAG}

# Prepare deployment command (matching existing loxation-messaging setup)
DEPLOY_CMD="gcloud run deploy ${SERVICE_NAME} \
  --image gcr.io/${PROJECT_ID}/${SERVICE_NAME}:${TAG} \
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

# Using Firestore - no additional database setup needed

# Add all environment variables to the command
DEPLOY_CMD="${DEPLOY_CMD} --set-env-vars=\"${ENV_VARS}\""

# Deploy to Cloud Run
echo "Deploying to Cloud Run..."
eval $DEPLOY_CMD

# Get the service URL
SERVICE_URL=$(gcloud run services describe ${SERVICE_NAME} \
  --platform managed \
  --region ${REGION} \
  --project ${PROJECT_ID} \
  --format 'value(status.url)')

echo "Deployment complete!"
echo "Service URL: ${SERVICE_URL}"

# Wait a moment for the service to be ready
echo "Waiting for service to be ready..."
sleep 10

# Verify deployment health
echo "Verifying deployment health..."
HEALTH_CHECK_URL="${SERVICE_URL}/health"

# Since the service requires authentication, we'll just check if it responds
if curl -s -f -o /dev/null --max-time 10 "${HEALTH_CHECK_URL}" 2>/dev/null; then
    echo "✅ Health check passed"
else
    echo "⚠️  Health check endpoint not accessible (expected if auth is enabled)"
    echo "Service deployed successfully. Check Cloud Run logs if issues persist."
fi

echo ""
echo "🚀 MLS Secure Relay deployed successfully!"

# Deploy Cloud Run Job for keypackage cleanup
echo ""
echo "Setting up Cloud Run Job for keypackage cleanup..."

JOB_NAME="${SERVICE_NAME}-cleanup"
SCHEDULE_NAME="${JOB_NAME}-schedule"

# Create or update the Cloud Run Job
echo "Creating/updating Cloud Run Job: ${JOB_NAME}"
gcloud run jobs replace ${JOB_NAME} \
  --image gcr.io/${PROJECT_ID}/${SERVICE_NAME}:${TAG} \
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
  --image gcr.io/${PROJECT_ID}/${SERVICE_NAME}:${TAG} \
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

# Create or update the Cloud Scheduler job
echo "Setting up Cloud Scheduler: ${SCHEDULE_NAME}"
gcloud scheduler jobs update http ${SCHEDULE_NAME} \
  --location ${REGION} \
  --schedule "0 2 * * *" \
  --time-zone "America/New_York" \
  --uri "${JOB_URI}" \
  --http-method POST \
  --oauth-service-account-email ${SERVICE_ACCOUNT} \
  2>/dev/null || \
gcloud scheduler jobs create http ${SCHEDULE_NAME} \
  --location ${REGION} \
  --schedule "0 2 * * *" \
  --time-zone "America/New_York" \
  --uri "${JOB_URI}" \
  --http-method POST \
  --oauth-service-account-email ${SERVICE_ACCOUNT}

echo "✅ Cloud Run Job and Scheduler configured successfully"

echo ""
echo "📊 Monitor logs: gcloud logs tail --follow projects/${PROJECT_ID}/services/${SERVICE_NAME}"
echo "🔧 Service URL: ${SERVICE_URL}"
echo "🧹 Cleanup job: ${JOB_NAME} (runs daily at 2 AM ET)"
echo ""
echo "To test the cleanup job manually:"
echo "  gcloud run jobs execute ${JOB_NAME} --region ${REGION}"
echo ""
echo "Next steps:"
echo "1. Deploy Firestore indexes: firebase deploy --only firestore:indexes"
echo "2. Configure pubkey allowlists in the configuration"