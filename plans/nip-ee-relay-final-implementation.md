# NIP-EE-RELAY Final Implementation Plan

## Executive Summary

We're implementing a KeyPackage store-and-forward mechanism that intercepts standard REQ queries for KeyPackages (kind 443) and automatically handles consumption tracking. This approach is simpler than the original kind 447 design and works with existing Nostr infrastructure.

## Core Mechanism

When Alice wants Bob's KeyPackages:
```
1. Alice sends: REQ {"kinds":[443], "authors":["bob_pubkey"]}
2. Relay intercepts this query
3. Relay returns Bob's KeyPackages (max 2 per query)  
4. Relay marks returned packages as consumed (except the last one)
5. Relay enforces rate limits (10 queries/hour per requester-author pair)
```

## Key Features

### 1. Automatic Consumption
- KeyPackages are consumed when returned via REQ
- No separate consumption notification needed
- Transparent to clients

### 2. Last Resort Protection  
- Never consume the last remaining KeyPackage
- Return it but keep it available
- Existing logic remains unchanged

### 3. Rate Limiting
- Per requester-author pair: 10 queries/hour
- Max 2 KeyPackages per query
- Sliding window implementation

## Implementation Tasks

### Phase 1: Core REQ Interception (Priority 1)

**File: `relay/src/session.rs`**
```rust
// Add to Session struct
impl Session {
    async fn handle_req_message(&mut self, req: ReqMessage) -> Result<()> {
        // Check if querying for KeyPackages
        if self.is_keypackage_query(&req) {
            return self.extensions.process_keypackage_req(req, self).await;
        }
        
        // Normal REQ processing
        self.process_standard_req(req).await
    }
    
    fn is_keypackage_query(&self, req: &ReqMessage) -> bool {
        req.filters.iter().any(|f| {
            f.kinds.as_ref().map_or(false, |k| k.contains(&443))
        })
    }
}
```

### Phase 2: Extension Interface Update

**File: `relay/src/extension.rs`**
```rust
#[async_trait]
pub trait Extension: Send + Sync {
    // Existing
    async fn process_event(&self, event: &Event) -> ExtensionMessageResult;
    
    // NEW: Handle REQ messages
    async fn process_req(
        &self,
        req: &ReqMessage,
        session: &Session,
    ) -> Option<Vec<Event>> {
        None // Default: don't handle
    }
}
```

### Phase 3: MLS Gateway REQ Handler

**File: `extensions/src/mls_gateway/mod.rs`**
```rust
impl Extension for MlsGateway {
    async fn process_req(
        &self,
        req: &ReqMessage,
        session: &Session,
    ) -> Option<Vec<Event>> {
        // Only handle KeyPackage queries
        if !req.filters.iter().any(|f| f.kinds.as_ref().map_or(false, |k| k.contains(&443))) {
            return None;
        }
        
        let requester = session.auth_pubkey()?;
        let mut all_events = Vec::new();
        
        // Process each filter
        for filter in &req.filters {
            if let Some(authors) = &filter.authors {
                for author in authors {
                    match self.get_and_consume_keypackages(&requester, author).await {
                        Ok(events) => all_events.extend(events),
                        Err(e) => warn!("Failed to get KeyPackages: {}", e),
                    }
                }
            }
        }
        
        Some(all_events)
    }
}
```

### Phase 4: Consumption Logic

**File: `extensions/src/mls_gateway/mod.rs`**
```rust
impl MlsGateway {
    async fn get_and_consume_keypackages(
        &self,
        requester: &str,
        author: &str,
    ) -> Result<Vec<Event>> {
        // Rate limit check
        if !self.rate_limiter.check(requester, author).await? {
            return Err(anyhow!("Rate limit exceeded"));
        }
        
        // Get count
        let total = self.store.count_user_keypackages(author).await?;
        
        // Query KeyPackages (oldest first, max 2)
        let kps = self.store.query_keypackages(
            Some(&[author.to_string()]),
            None,
            Some(2),
            Some("created_at_asc")
        ).await?;
        
        let mut events = Vec::new();
        let mut to_consume = Vec::new();
        
        for (event_id, _, _, _) in kps {
            // Get the actual event
            if let Some(event) = self.get_event_from_db(&event_id).await? {
                events.push(event);
                
                // Consume if not the last one
                if total - to_consume.len() as u32 > 1 {
                    to_consume.push(event_id);
                }
            }
        }
        
        // Delete consumed KeyPackages
        for id in to_consume {
            self.store.delete_consumed_keypackage(&id).await?;
        }
        
        // Update metrics
        counter!("mls_keypackages_served").increment(events.len() as u64);
        counter!("mls_keypackages_consumed").increment(to_consume.len() as u64);
        
        Ok(events)
    }
}
```

### Phase 5: Rate Limiter

**File: `extensions/src/mls_gateway/rate_limiter.rs`**
```rust
pub struct KeyPackageRateLimiter {
    limits: Arc<Mutex<HashMap<(String, String), RateLimit>>>,
}

struct RateLimit {
    queries: Vec<Instant>,
    window_start: Instant,
}

impl KeyPackageRateLimiter {
    pub async fn check(&self, requester: &str, author: &str) -> Result<bool> {
        let mut limits = self.limits.lock().await;
        let key = (requester.to_string(), author.to_string());
        
        let now = Instant::now();
        let window = Duration::from_secs(3600); // 1 hour
        
        let limit = limits.entry(key).or_insert_with(|| RateLimit {
            queries: Vec::new(),
            window_start: now,
        });
        
        // Clean old queries
        limit.queries.retain(|&t| now.duration_since(t) < window);
        
        // Check limit
        if limit.queries.len() >= 10 {
            counter!("mls_rate_limit_exceeded").increment(1);
            return Ok(false);
        }
        
        // Record query
        limit.queries.push(now);
        Ok(true)
    }
}
```

## Testing Plan

### Unit Tests
1. **Consumption Logic**: Verify correct packages are consumed
2. **Last Resort**: Ensure last package is never consumed
3. **Rate Limiting**: Test limit enforcement

### Integration Tests  
1. **End-to-End Flow**: Alice queries Bob's packages
2. **Multiple Queries**: Verify consumption tracking
3. **Rate Limit**: Exceed limits and verify rejection

### Manual Testing
```bash
# 1. Bob publishes KeyPackages
nostr-tool publish --kind 443 --content "<keypackage>"

# 2. Alice queries Bob's KeyPackages  
nostr-tool req --kinds 443 --authors <bob_pubkey>

# 3. Verify packages returned and consumed
# 4. Query again, verify different packages
# 5. Query until rate limited
```

## Deployment Steps

### Week 1: Development
1. Implement REQ interception in relay core
2. Add Extension trait method for REQ handling  
3. Implement consumption logic in MLS Gateway
4. Add rate limiting

### Week 2: Testing & Refinement
1. Unit and integration tests
2. Load testing with multiple clients
3. Fix any issues found
4. Performance optimization

### Week 3: Deployment
1. Deploy to staging environment
2. Test with real clients
3. Monitor metrics
4. Deploy to production

## Success Metrics

1. **Consumption Rate**: >95% of non-last KeyPackages consumed
2. **Query Success**: >99% of valid queries return KeyPackages  
3. **Rate Limit Effectiveness**: <1% of queries rate limited
4. **Performance**: <50ms added latency for KeyPackage queries

## Phase 6: Proactive KeyPackage Replenishment

The relay should monitor KeyPackage supplies and request replenishment before users run out.

**File: `extensions/src/mls_gateway/replenishment.rs`**
```rust
pub struct KeyPackageReplenisher {
    store: Arc<dyn MlsStorage>,
    min_threshold: u32, // Default: 3
    check_interval: Duration, // Default: 5 minutes
}

impl KeyPackageReplenisher {
    pub async fn start(self, relay_sender: Sender<NostrEvent>) {
        let mut interval = tokio::time::interval(self.check_interval);
        
        loop {
            interval.tick().await;
            
            if let Err(e) = self.check_and_request_replenishment(&relay_sender).await {
                error!("Replenishment check failed: {}", e);
            }
        }
    }
    
    async fn check_and_request_replenishment(
        &self,
        relay_sender: &Sender<NostrEvent>
    ) -> Result<()> {
        // Get users with low KeyPackage counts
        let low_users = self.store.get_users_below_threshold(self.min_threshold).await?;
        
        for (user_pubkey, count) in low_users {
            info!("User {} has only {} KeyPackages, requesting replenishment",
                  user_pubkey, count);
            
            // Create a KeyPackage Request (447) FROM the relay
            let request = self.create_replenishment_request(&user_pubkey, count)?;
            
            // Send to user's inbox relays
            relay_sender.send(request).await?;
            
            counter!("mls_replenishment_requests_sent").increment(1);
        }
        
        Ok(())
    }
    
    fn create_replenishment_request(&self, user_pubkey: &str, current_count: u32) -> Result<Event> {
        // Create kind 447 request targeting the user
        let suggested_count = 10 - current_count; // Aim for 10 total
        
        Event::new(
            447,
            &self.relay_identity, // Relay's identity
            json!({
                "reason": "low_keypackage_supply",
                "current_count": current_count,
                "suggested_count": suggested_count
            }),
            vec![
                vec!["p", user_pubkey],
                vec!["relay", &self.relay_url],
                vec!["min", &suggested_count.to_string()],
                vec!["reason", "Your KeyPackage supply is running low"]
            ]
        )
    }
}

// Add to MlsStorage trait
#[async_trait]
pub trait MlsStorage: Send + Sync {
    // ... existing methods ...
    
    /// Get users with KeyPackage count below threshold
    async fn get_users_below_threshold(&self, threshold: u32) -> Result<Vec<(String, u32)>>;
}
```

### Replenishment Flow

```
1. Relay monitors all users' KeyPackage counts
2. When user drops below threshold (e.g., 3 packages):
   - Relay creates kind 447 request
   - Sends to user's known relays
3. User's client receives 447:
   - Generates new KeyPackages
   - Publishes kind 443 events
4. Relay stores new KeyPackages
```

### Configuration

```toml
[extensions.mls_gateway]
# Replenishment settings
replenishment_enabled = true
replenishment_threshold = 3  # Request more when below this
replenishment_check_interval = 300  # Check every 5 minutes
replenishment_target_count = 10  # Aim for this many total
```

## Key Benefits

1. **Simplicity**: Uses standard Nostr REQ/EVENT flow
2. **Compatibility**: Works with existing clients
3. **Transparency**: Consumption happens automatically
4. **Security**: Preserves MLS security properties
5. **Scalability**: Rate limiting prevents abuse
6. **Proactive**: Prevents KeyPackage exhaustion

## Next Steps

1. **Immediate**: Start implementing REQ interception
2. **This Week**: Complete core consumption logic
3. **Next Week**: Add rate limiting and replenishment
4. **Two Weeks**: Deploy to staging for testing

This approach delivers all required functionality while maintaining simplicity and compatibility with existing Nostr infrastructure.