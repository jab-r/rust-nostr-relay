# Simplified KeyPackage Implementation: REQ-based Consumption

## Overview
Instead of implementing kind 447, we'll enhance the relay to automatically track KeyPackage consumption when they're returned via standard REQ queries.

## Key Design Decisions

### 1. Automatic Consumption on Query
**When a REQ query returns KeyPackages (443), they are automatically marked as consumed**

```
Alice → Relay: REQ {"kinds":[443], "authors":["bob_pubkey"]}
Relay: 
  1. Query Bob's KeyPackages
  2. Apply last-resort logic (keep last one)
  3. Return non-last KeyPackages to Alice
  4. Mark returned packages as consumed
  5. Track rate limits for Alice
```

### 2. Rate Limiting Per Requestor
Track and limit KeyPackage queries per requesting pubkey:
- Max queries per hour for a requesting publkey to an author: 10
- Max KeyPackages returned per query: 2
- Sliding window rate limiting

### 3. Last Resort Protection (Existing Logic)
Keep the current approach:
- Never consume the last remaining KeyPackage
- Return it in queries but don't delete it
- User must publish new KeyPackages before last one can be consumed

## Implementation Plan

### Phase 1: Intercept REQ Queries for KeyPackages

```rust
// In relay core, when processing REQ filters
impl Session {
    async fn process_req(&mut self, req: ReqMessage) -> Result<()> {
        // Check if this is a KeyPackage query
        let is_keypackage_query = req.filters.iter().any(|f| {
            f.kinds.as_ref().map_or(false, |kinds| kinds.contains(&443))
        });
        
        if is_keypackage_query {
            // Route to special handler
            return self.handle_keypackage_req(req).await;
        }
        
        // Normal REQ processing
        self.handle_standard_req(req).await
    }
}
```

### Phase 2: Enhanced KeyPackage Query Handler

```rust
impl Session {
    async fn handle_keypackage_req(&mut self, req: ReqMessage) -> Result<()> {
        let requester_pubkey = self.auth_pubkey.clone(); // Assuming authenticated
        
        // Rate limiting check
        if !self.check_rate_limit(&requester_pubkey).await? {
            return Err(anyhow!("Rate limit exceeded for KeyPackage queries"));
        }
        
        // Extract target authors from filters
        let target_authors = extract_authors_from_filters(&req.filters);
        
        for author in target_authors {
            // Query KeyPackages with consumption logic
            let keypackages = self.query_and_consume_keypackages(
                &author,
                &requester_pubkey,
                5 // max per author
            ).await?;
            
            // Send events through normal subscription flow
            for kp_event in keypackages {
                self.send_event(kp_event).await?;
            }
        }
        
        // Send EOSE
        self.send_eose(&req.subscription_id).await?;
        
        Ok(())
    }
}
```

### Phase 3: Query and Consume Implementation

```rust
impl MlsGateway {
    async fn query_and_consume_keypackages(
        &self,
        owner_pubkey: &str,
        requester_pubkey: &str,
        limit: usize,
    ) -> Result<Vec<Event>> {
        // Get total count first
        let total_count = self.store.count_user_keypackages(owner_pubkey).await?;
        
        // Query KeyPackages (oldest first)
        let stored_kps = self.store.query_keypackages(
            Some(&[owner_pubkey.to_string()]),
            None,
            Some(limit as u32),
            Some("created_at_asc")
        ).await?;
        
        let mut events_to_return = Vec::new();
        let mut ids_to_consume = Vec::new();
        
        // Determine which to return and consume
        for (idx, (event_id, _, _, _)) in stored_kps.iter().enumerate() {
            // Check if this is the last KeyPackage
            let remaining_count = total_count - ids_to_consume.len() as u32;
            let is_last = remaining_count <= 1;
            
            if !is_last {
                // Safe to consume
                ids_to_consume.push(event_id.clone());
            }
            
            // Always return it (even if last)
            if let Some(event) = self.db.get_event(event_id)? {
                events_to_return.push(event);
            }
        }
        
        // Mark consumed KeyPackages
        for event_id in &ids_to_consume {
            self.store.delete_consumed_keypackage(event_id).await?;
            info!("Consumed KeyPackage {} for requester {}", event_id, requester_pubkey);
        }
        
        // Update metrics
        counter!("keypackages_queried", "requester" => requester_pubkey.to_string())
            .increment(events_to_return.len() as u64);
        counter!("keypackages_consumed", "owner" => owner_pubkey.to_string())
            .increment(ids_to_consume.len() as u64);
        
        // Log consumption
        info!(
            "KeyPackage query: requester={}, owner={}, returned={}, consumed={}, remaining={}",
            requester_pubkey,
            owner_pubkey,
            events_to_return.len(),
            ids_to_consume.len(),
            total_count - ids_to_consume.len() as u32
        );
        
        Ok(events_to_return)
    }
}
```

### Phase 4: Rate Limiting Implementation

```rust
// Add to Firestore or in-memory cache
#[derive(Debug, Serialize, Deserialize)]
struct KeyPackageQueryRateLimit {
    pub requester_pubkey: String,
    pub query_count: u32,
    pub keypackages_served: u32,
    #[serde(with = "chrono::serde::ts_seconds")]
    pub window_start: DateTime<Utc>,
}

impl RateLimiter {
    async fn check_and_update_rate_limit(
        &self,
        requester_pubkey: &str,
        keypackages_requested: u32,
    ) -> Result<bool> {
        let now = Utc::now();
        let window_duration = Duration::hours(1);
        
        // Get or create rate limit record
        let mut rate_limit = self.get_rate_limit(requester_pubkey).await?
            .unwrap_or_else(|| KeyPackageQueryRateLimit {
                requester_pubkey: requester_pubkey.to_string(),
                query_count: 0,
                keypackages_served: 0,
                window_start: now,
            });
        
        // Reset if window expired
        if now - rate_limit.window_start > window_duration {
            rate_limit.query_count = 0;
            rate_limit.keypackages_served = 0;
            rate_limit.window_start = now;
        }
        
        // Check limits
        if rate_limit.query_count >= 10 {
            return Ok(false); // Too many queries
        }
        if rate_limit.keypackages_served + keypackages_requested > 50 {
            return Ok(false); // Too many KeyPackages
        }
        
        // Update counts
        rate_limit.query_count += 1;
        rate_limit.keypackages_served += keypackages_requested;
        
        // Save updated limits
        self.save_rate_limit(&rate_limit).await?;
        
        Ok(true)
    }
}
```

## Integration Points

### 1. Relay Core Modification
Need to intercept REQ messages in the relay core:
- `relay/src/session.rs` - Add KeyPackage query detection
- `relay/src/server.rs` - Route to MLS extension for handling

### 2. Extension Interface
Extend the extension trait to handle REQ messages:
```rust
#[async_trait]
pub trait Extension: Send + Sync {
    // Existing method
    async fn process_event(&self, event: &Event) -> ExtensionMessageResult;
    
    // NEW: Process REQ messages
    async fn process_req(
        &self, 
        req: &ReqMessage,
        session_context: &SessionContext,
    ) -> Option<ExtensionReqResult>;
}

pub struct SessionContext {
    pub auth_pubkey: Option<String>,
    pub subscription_id: String,
    pub connection_id: String,
}

pub enum ExtensionReqResult {
    Handled(Vec<Event>), // Events to send
    PassThrough,         // Let relay handle normally
}
```

### 3. Database Access
The extension needs read access to the event database to retrieve KeyPackages.

## Testing Strategy

### 1. Unit Tests
```rust
#[test]
async fn test_keypackage_consumption_on_query() {
    // Setup: Store 5 KeyPackages for Bob
    // Action: Alice queries Bob's KeyPackages
    // Assert: 4 consumed, 1 remaining (last resort)
}

#[test]
async fn test_rate_limiting() {
    // Setup: Configure rate limits
    // Action: Exceed query limit
    // Assert: Queries rejected
}
```

### 2. Integration Tests
```rust
#[test]
async fn test_req_interception() {
    // Setup: Full relay with MLS extension
    // Action: Send REQ for kind 443
    // Assert: KeyPackages returned and consumed
}
```

## Benefits of This Approach

1. **No Protocol Changes**: Uses standard Nostr REQ
2. **Transparent**: Consumption happens automatically
3. **Simple Client Implementation**: Just query for 443s
4. **Preserves Security**: Last resort protection works
5. **Rate Limited**: Prevents abuse
6. **No State Complexity**: No pending deliveries to track

## Migration Path

### Week 1: Core Implementation
1. Add REQ interception logic
2. Implement consumption on query
3. Test with existing KeyPackages

### Week 2: Rate Limiting & Metrics
1. Add rate limiting
2. Deploy metrics collection
3. Monitor usage patterns

### Week 3: Optimization
1. Add caching for frequent queries
2. Batch consumption updates
3. Performance tuning

## Conclusion

This approach is significantly simpler than implementing kind 447 while providing all the necessary functionality:
- ✅ Automatic consumption tracking
- ✅ Rate limiting per requester  
- ✅ Last resort protection
- ✅ Works with standard Nostr clients
- ✅ No gift-wrapping complexity

The relay becomes a smart KeyPackage broker that transparently handles the MLS requirements while maintaining compatibility with standard Nostr queries.