# Multi-stage build for rust-nostr-relay with MLS Gateway Extension
# Simplified version for Cloud Run deployment

# Build stage
FROM rust:1.89-slim AS builder

# Install system dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy all source code
COPY . .

# Build the application with MLS features (no-default-features to avoid search issues)
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
COPY config/ ./config/

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
