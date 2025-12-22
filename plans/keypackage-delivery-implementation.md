# KeyPackage Delivery Implementation Plan

## The Core Problem
The relay currently handles KeyPackage requests (447) but **does not deliver the requested KeyPackages back to the requester**. This is the single most critical issue preventing the MLS flow from working.

## Current State Analysis

### What Works ✓
```rust
// In handle_keypackage_request():
1. Extracts recipient from 447 request ✓
2. Queries stored KeyPackages (443) ✓  
3. Tracks consumption (deletes non-last packages) ✓
4. Logs what it WOULD send ✓
```

### What's Broken ✗
```rust
// MISSING: Actually sending the KeyPackages to the requester!
// The code says:
info!("Sending {} keypackage events to requester {}", ...);
// But then tries to:
self.send_event_to_requester(&event_pubkey, &kp_event)?;
// Which doesn't exist and isn't implemented!
```

## Solution: Implement KeyPackage Delivery

### Architecture Overview
```
┌─────────────┐       ┌─────────────┐       ┌──────────────┐
│   Alice     │       │    Relay    │       │     Bob      │
│  (Online)   │       │             │       │  (Offline)   │
└──────┬──────┘       └──────┬──────┘       └──────┬───────┘
       │                     │                      │
       │  1. REQ/SUB         │                      │
       │────────────────────>│                      │
       │                     │                      │
       │  2. EVENT 447       │                      │
       │  (Request Bob's     │                      │
       │   KeyPackages)      │                      │
       │────────────────────>│                      │
       │                     │                      │
       │              3. Query Bob's                │
       │                 stored 443s                │
       │                     │                      │
       │  4. EVENT 443       │                      │
       │  (Bob's KeyPackage) │                      │
       │<────────────────────│                      │
       │                     │                      │
       │  5. OK (447 stored) │                      │
       │<────────────────────│                      │
```

### Implementation Approach

Since we cannot modify the Extension trait to include Session context immediately, we need a different approach:

#### Option 1: Store and Match Pattern (Recommended)
Instead of trying to push events directly during 447 processing, we can:

1. **Store a "pending delivery" record** when processing 447
2. **Match against active subscriptions** in the relay core
3. **Deliver matching 443 events** through normal subscription flow

```rust
// In handle_keypackage_request:
async fn handle_keypackage_request(&self, event: &Event) -> anyhow::Result<()> {
    // ... existing validation and query logic ...
    
    // NEW: Store pending deliveries
    for (event_id, owner, _content, created_at) in keypackages.iter() {
        self.store_pending_delivery(
            &event_pubkey,  // requester
            event_id,       // keypackage to deliver
            owner,          // keypackage owner
            created_at
        ).await?;
    }
    
    // Continue with consumption tracking...
}
```

#### Option 2: Event Echo Pattern
Create synthetic EOSE (End of Stored Events) markers that trigger delivery:

```rust
// After processing 447, emit a special internal event
self.emit_delivery_trigger(EventDeliveryTrigger {
    requester: event_pubkey.clone(),
    keypackage_ids: keypackages_to_return,
    request_id: event.id_str(),
})?;
```

### Detailed Implementation Steps

## Step 1: Add Pending Delivery Storage

```rust
// In Firestore schema
#[derive(Debug, Serialize, Deserialize)]
struct PendingKeyPackageDelivery {
    pub requester_pubkey: String,
    pub keypackage_event_id: String,
    pub keypackage_owner_pubkey: String,
    pub request_event_id: String,
    #[serde(with = "chrono::serde::ts_seconds")]
    pub created_at: DateTime<Utc>,
    #[serde(with = "chrono::serde::ts_seconds")]
    pub expires_at: DateTime<Utc>,
}
```

## Step 2: Modify KeyPackage Request Handler

```rust
async fn handle_keypackage_request(&self, event: &Event) -> anyhow::Result<()> {
    let store = self.store()?;
    let event_pubkey = hex::encode(event.pubkey());
    
    // Extract recipient
    let recipient = event.tags().iter()
        .find(|tag| tag.len() >= 2 && tag[0] == "p")
        .map(|tag| tag[1].clone())
        .ok_or_else(|| anyhow::anyhow!("Missing recipient"))?;
    
    // Query available keypackages
    let keypackages = store.query_keypackages(
        Some(&[recipient.clone()]),
        None,
        Some(min_count.max(10)),
        Some("created_at_asc")
    ).await?;
    
    // NEW: Create pending deliveries
    let expires_at = chrono::Utc::now() + chrono::Duration::minutes(5);
    for (event_id, owner, _, _) in &keypackages {
        let pending = PendingKeyPackageDelivery {
            requester_pubkey: event_pubkey.clone(),
            keypackage_event_id: event_id.clone(),
            keypackage_owner_pubkey: owner.clone(),
            request_event_id: event.id_str(),
            created_at: chrono::Utc::now(),
            expires_at,
        };
        
        store.create_pending_delivery(&pending).await?;
        info!("Created pending delivery of {} to {}", event_id, event_pubkey);
    }
    
    // Continue with consumption logic...
    // Mark non-last packages as consumed
    
    Ok(())
}
```

## Step 3: Add Delivery Processor

Create a separate component that processes pending deliveries:

```rust
pub struct KeyPackageDeliveryProcessor {
    store: Arc<dyn MlsStorage>,
    db: Arc<Database>,
}

impl KeyPackageDeliveryProcessor {
    pub async fn process_pending_deliveries(
        &self,
        active_sessions: &HashMap<String, Arc<Session>>
    ) -> Result<()> {
        // Get all pending deliveries
        let pending = self.store.get_pending_deliveries().await?;
        
        for delivery in pending {
            // Check if requester has active session
            if let Some(session) = active_sessions.get(&delivery.requester_pubkey) {
                // Retrieve the keypackage event
                if let Ok(Some(kp_event)) = self.get_event(&delivery.keypackage_event_id) {
                    // Send through session
                    if session.matches_filters(&kp_event) {
                        session.send_event(kp_event).await?;
                        info!("Delivered KeyPackage {} to {}", 
                              delivery.keypackage_event_id, 
                              delivery.requester_pubkey);
                        
                        // Mark as delivered
                        self.store.mark_delivered(&delivery).await?;
                    }
                }
            }
            
            // Clean up expired deliveries
            if delivery.expires_at < chrono::Utc::now() {
                self.store.delete_pending_delivery(&delivery).await?;
            }
        }
        
        Ok(())
    }
}
```

## Step 4: Integration with Relay Core

Add periodic processing in the relay's main loop:

```rust
// In relay server main loop
tokio::spawn(async move {
    let processor = KeyPackageDeliveryProcessor::new(store, db);
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    
    loop {
        interval.tick().await;
        
        // Get active sessions snapshot
        let sessions = get_active_sessions().await;
        
        // Process pending deliveries
        if let Err(e) = processor.process_pending_deliveries(&sessions).await {
            error!("Failed to process KeyPackage deliveries: {}", e);
        }
    }
});
```

## Alternative: Direct Database Query Pattern

A simpler approach that leverages existing REQ/subscription matching:

```rust
// When Alice sends REQ with filter for Bob's 443 events:
{
  "kinds": [443],
  "authors": ["bob_pubkey"],
  "#e": ["keypackage_request_event_id"]  // Tag the request
}

// The relay's normal query would return Bob's KeyPackages
// We just need to ensure they're not filtered out
```

## Testing Strategy

### 1. Unit Test: Pending Delivery Creation
```rust
#[tokio::test]
async fn test_keypackage_request_creates_pending_delivery() {
    let gateway = setup_test_gateway().await;
    let request = create_test_447_event();
    
    gateway.handle_keypackage_request(&request).await.unwrap();
    
    let pending = gateway.store.get_pending_deliveries().await.unwrap();
    assert_eq!(pending.len(), 3); // Requested 3 packages
}
```

### 2. Integration Test: End-to-End Delivery
```rust
#[tokio::test] 
async fn test_keypackage_delivery_flow() {
    // 1. Bob stores KeyPackages
    // 2. Alice connects and subscribes
    // 3. Alice sends 447 request
    // 4. Assert Alice receives Bob's 443 events
}
```

## Rollout Plan

### Phase 1: Add Pending Delivery Storage (Day 1)
- Add Firestore schema
- Deploy storage migration
- No behavior change yet

### Phase 2: Create Pending Deliveries (Day 2)  
- Update 447 handler to store pending deliveries
- Monitor creation rates
- Still no delivery

### Phase 3: Enable Delivery Processor (Day 3)
- Deploy delivery processor
- Start with low frequency (every 5s)
- Monitor delivery success

### Phase 4: Optimize and Scale (Week 2)
- Increase processing frequency
- Add batching for efficiency  
- Clean up expired deliveries

## Success Metrics

1. **Delivery Success Rate**: >99% of pending deliveries completed
2. **Delivery Latency**: <2s from request to delivery
3. **Zero KeyPackage Exhaustion**: No users left without packages
4. **Request Fulfillment**: >95% of requests get packages

## Next Immediate Action

The fastest path to a working system:

1. Add the pending delivery storage schema
2. Update handle_keypackage_request to create pending deliveries  
3. Create a simple delivery processor
4. Test with real client flow

This approach works within current architectural constraints while solving the core problem.