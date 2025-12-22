# NIP-EE KeyPackage Implementation Plan

## Overview

The Swift team requires full NIP-EE keypackage lifecycle management support in the rust-nostr-relay. Currently, we only validate keypackages but don't store or manage them properly.

## Required Features

### 1. Storage Layer Enhancements

Add the following methods to the `MlsStorage` trait:

```rust
#[async_trait::async_trait]
pub trait MlsStorage: Send + Sync {
    // ... existing methods ...
    
    /// Store a new keypackage
    async fn store_keypackage(
        &self,
        event_id: &str,
        owner_pubkey: &str,
        content: &str,
        ciphersuite: &str,
        extensions: &[String],
        relays: &[String],
        created_at: i64,
        expires_at: i64,
    ) -> anyhow::Result<()>;
    
    /// Query keypackages with filters
    async fn query_keypackages(
        &self,
        authors: Option<&[String]>,
        since: Option<i64>,
        limit: Option<u32>,
        exclude_consumed: bool,
    ) -> anyhow::Result<Vec<Event>>;
    
    /// Mark a keypackage as consumed
    async fn mark_keypackage_consumed(
        &self,
        event_id: &str,
    ) -> anyhow::Result<()>;
    
    /// Count keypackages per user
    async fn count_user_keypackages(
        &self,
        owner_pubkey: &str,
        exclude_consumed: bool,
    ) -> anyhow::Result<u32>;
    
    /// Clean up expired keypackages
    async fn cleanup_expired_keypackages(&self) -> anyhow::Result<u32>;
}
```

### 2. Event Handler Updates

Update `handle_keypackage()` in `mod.rs`:

```rust
async fn handle_keypackage(&self, event: &Event) -> anyhow::Result<()> {
    // ... existing validation ...
    
    // Check for last resort extension
    let has_last_resort = extensions.as_ref()
        .map(|exts| exts.iter().any(|e| e == "last_resort" || e == "0xF000"))
        .unwrap_or(false);
    
    // Check per-user limits
    let current_count = store.count_user_keypackages(&event_pubkey, true).await?;
    if current_count >= self.config.max_keypackages_per_user.unwrap_or(10) {
        return Err(anyhow::anyhow!("User keypackage limit exceeded"));
    }
    
    // Store the keypackage
    let expires_at = expiry.unwrap_or_else(|| {
        chrono::Utc::now().timestamp() + self.config.keypackage_ttl as i64
    });
    
    store.store_keypackage(
        &event.id().to_hex(),
        &event_pubkey,
        event.content(),
        &ciphersuite.unwrap_or_default(),
        &extensions.unwrap_or_default(),
        &all_relays,
        event.created_at().as_i64(),
        expires_at,
    ).await?;
}
```

### 3. Query Support Implementation

Add custom REQ handling for kind 443 queries:

```rust
impl Extension for MlsGateway {
    fn handle_req(&self, req: &ReqCmd) -> ExtensionResult<ReqCmd> {
        // Check if any filter requests kind 443
        let has_keypackage_filter = req.filters.iter().any(|f| {
            f.kinds.as_ref().map(|kinds| kinds.contains(&443)).unwrap_or(false)
        });
        
        if has_keypackage_filter {
            // Custom handling for keypackage queries
            // Filter out consumed keypackages
            // Apply per-user limits
            // Return only valid, unconsumed keypackages
        }
    }
}
```

### 4. Welcome Message Integration

When processing Welcome messages (kind 444):

```rust
async fn handle_welcome(&self, event: &Event) -> anyhow::Result<()> {
    // ... existing logic ...
    
    // Mark the referenced keypackage as consumed
    if let Some(keypackage_id) = keypackage_event_id {
        store.mark_keypackage_consumed(&keypackage_id).await?;
    }
}
```

### 5. Configuration Updates

Add configuration options:

```rust
pub struct MlsGatewayConfig {
    // ... existing fields ...
    
    /// Maximum keypackages per user (default: 10)
    pub max_keypackages_per_user: Option<u32>,
    
    /// Whether to validate last_resort extension (default: true)
    pub require_last_resort: bool,
    
    /// Cleanup interval for expired keypackages (seconds)
    pub keypackage_cleanup_interval: u64,
}
```

### 6. Database Schema Updates

For SQL backend:
```sql
ALTER TABLE mls_keypackages ADD COLUMN IF NOT EXISTS 
    ciphersuite TEXT,
    extensions TEXT[],
    relays TEXT[],
    has_last_resort BOOLEAN DEFAULT false;

CREATE INDEX idx_mls_keypackages_unconsumed 
    ON mls_keypackages(recipient_pubkey, expires_at) 
    WHERE picked_up_at IS NULL;
```

For Firestore:
- Add fields to KeyPackage document structure
- Create composite indexes for efficient querying

## Implementation Priority

1. **Phase 1 (Critical)**:
   - Store keypackages in database
   - Basic query support with author/since/limit filters
   - Mark keypackages as consumed

2. **Phase 2 (Important)**:
   - Per-user limit enforcement
   - Last resort extension validation
   - Automatic expiry cleanup

3. **Phase 3 (Nice to have)**:
   - REST API endpoints for keypackage management
   - Metrics and monitoring
   - Bulk operations support

## Testing Requirements

1. Unit tests for storage methods
2. Integration tests for lifecycle management
3. Load tests for query performance
4. E2E tests with NostrSDK client

## Security Considerations

1. Validate keypackage ownership (pubkey must match event author)
2. Prevent replay attacks by tracking consumed keypackages
3. Rate limiting for keypackage submissions
4. Audit logging for keypackage consumption

## Timeline Estimate

- Phase 1: 2-3 days
- Phase 2: 2 days  
- Phase 3: 1-2 days
- Testing: 2 days

Total: ~1 week of development