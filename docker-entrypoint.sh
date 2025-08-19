#!/bin/bash
set -euo pipefail

# Environment variable defaults
RUST_LOG=${RUST_LOG:-info}
RNOSTR_CONFIG_PATH=${RNOSTR_CONFIG_PATH:-/app/config/rnostr.toml}

# Cloud Run environment variables
export DATABASE_URL=${DATABASE_URL:-"postgresql://localhost/mls_gateway"}
export GOOGLE_CLOUD_PROJECT=${GOOGLE_CLOUD_PROJECT:-""}
export INSTANCE_CONNECTION_NAME=${INSTANCE_CONNECTION_NAME:-""}

# Create configuration from template if it doesn't exist
if [[ ! -f "$RNOSTR_CONFIG_PATH" ]]; then
    echo "Creating configuration from template..."
    
    # Create config directory
    mkdir -p "$(dirname "$RNOSTR_CONFIG_PATH")"
    
    # Generate configuration with environment substitution
    cat > "$RNOSTR_CONFIG_PATH" << EOF
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
auth = "${METRICS_AUTH_KEY:-$(openssl rand -hex 32)}"

[auth]
enabled = true

[auth.req]
# No IP restrictions - mobile devices need dynamic IP access
# Authentication handled via NIP-42 only

[auth.event]
# High-security: only allow listed event authors (NIP-42 verified pubkeys)
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

# MLS Gateway Extension Configuration
[extensions.mls_gateway]
enabled = true
database_url = "${DATABASE_URL}"
keypackage_ttl = 604800  # 7 days
welcome_ttl = 259200     # 3 days
enable_api = true
api_prefix = "/api/v1"
EOF

    echo "Configuration created at $RNOSTR_CONFIG_PATH"
fi

# Validate configuration
if [[ ! -f "$RNOSTR_CONFIG_PATH" ]]; then
    echo "Error: Configuration file not found at $RNOSTR_CONFIG_PATH"
    exit 1
fi

# Create data directory if it doesn't exist
mkdir -p /app/data

# Handle Cloud SQL connection
if [[ -n "${INSTANCE_CONNECTION_NAME:-}" ]]; then
    echo "Setting up Cloud SQL connection for: $INSTANCE_CONNECTION_NAME"
    
    # Cloud Run provides automatic Cloud SQL connectivity
    # Update DATABASE_URL to use Unix socket if needed
    if [[ "${DATABASE_URL}" == *"localhost"* ]]; then
        export DATABASE_URL="postgresql:///mls_gateway?host=/cloudsql/${INSTANCE_CONNECTION_NAME}"
        echo "Updated DATABASE_URL for Cloud SQL: $DATABASE_URL"
    fi
fi

# Log startup information
echo "Starting rnostr MLS Gateway..."
echo "Configuration: $RNOSTR_CONFIG_PATH"
echo "Data directory: /app/data"
echo "Database URL: ${DATABASE_URL//:\/\/*:*@/://***:***@}"  # Mask credentials
echo "Log level: $RUST_LOG"

# Health check endpoint for Cloud Run
if [[ "${1:-}" == "health" ]]; then
    curl -f http://localhost:8080/health 2>/dev/null || exit 1
    exit 0
fi

# Start the relay
exec /app/rnostr "$@"