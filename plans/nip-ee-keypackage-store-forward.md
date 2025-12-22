# NIP-EE KeyPackage Store-and-Forward Implementation Plan

## The Fundamental Problem

When Alice wants to add Bob to an MLS group but Bob is offline, Alice needs Bob's KeyPackages to create the Welcome message. The relay MUST act as a store-and-forward service:

1. **Store**: Bob publishes KeyPackage Events (443) when online
2. **Intercept**: Alice sends KeyPackage Request (447) for Bob
3. **Forward**: Relay responds by pushing Bob's stored KeyPackages to Alice

This enables asynchronous MLS group creation without requiring both parties to be online simultaneously.

## Current Implementation Gap

The current code logs KeyPackage requests but DOES NOT deliver the requested KeyPackages back to the requester. This is the critical missing piece.

```rust
// Current code (BROKEN):
// 1. Queries KeyPackages from storage ✓
// 2. Marks them as consumed ✓ 
// 3. BUT NEVER SENDS THEM TO THE REQUESTER! ✗
```

## Solution Architecture

### Core Flow
```
Alice                    Relay                     Bob (offline)
  |                        |                          |
  |   REQ subscription     |                          |
  |----------------------->|                          |
  |                        |    (Bob's 443 events)   |
  |                        |<-------------------------|
  |                        |    (stored earlier)     |
  |                        |                          |
  |   447 Request for Bob  |                          |
  |----------------------->|                          |
  |                        |                          |
  |                 [Relay Intercepts]                |
  |                 [Queries Bob's 443s]              |
  |                 [Marks as consumed]               |
  |                        |                          |
  |   Bob's 443 events     |                          |
  |<-----------------------|                          |
  |   (via subscription)   |                          |
  |                        |                          |
```

### Implementation Components

#### 1. KeyPackage Request Handler Enhancement
```rust
async fn handle_keypackage_request(&self, event: &Event, session: &Session) -> Result<()> {
    // 1. Validate request (current code ✓)
    let recipient = extract_recipient(&event)?;
    
    // 2. Query stored KeyPackages (current code ✓)
    let keypackages = self.store.query_keypackages(
        Some(&[recipient]),
        None,
        Some(min_count),
        Some("created_at_asc")
    ).await?;
    
    // 3. NEW: Deliver KeyPackages to requester
    for (event_id, _, _, _) in keypackages {
        // Retrieve actual 443 event from database
        if let Some(kp_event) = self.db.get_event(&event_id)? {
            // Push to requester's active subscription
            session.send_stored_event(kp_event).await?;
        }
    }
    
    // 4. Mark non-last KeyPackages as consumed (current code ✓)
}
```

#### 2. Session Integration
The relay needs access to the requester's active WebSocket session to push events:

```rust
// Extension trait for Session
#[async_trait]
trait ExtensionContext {
    async fn get_session(&self) -> Option<&Session>;
    async fn send_event_to_session(&self, event: Event) -> Result<()>;
}
```

#### 3. Gift-Wrapped Request Handling
Since 447 requests should be gift-wrapped per NIP-EE:

```rust
async fn handle_giftwrap(&self, event: &Event, session: &Session) -> Result<()> {
    // Attempt to unwrap
    if let Ok(inner) = unwrap_gift_wrap(event) {
        if inner.kind == 447 {
            // Process as KeyPackage request
            return self.handle_keypackage_request(&inner, session).await;
        }
    }
    // Continue normal giftwrap processing...
}
```

## Implementation Steps

### Step 1: Add Session Context to Extension [CRITICAL]
```rust
pub struct ExtensionCall<'a> {
    pub event: &'a Event,
    pub session: &'a Session,  // NEW
    pub db: &'a Database,      // NEW
}

impl Extension for MlsGateway {
    async fn process_event(&self, ctx: ExtensionCall<'_>) -> ExtensionMessageResult {
        match ctx.event.kind() {
            447 => self.handle_keypackage_request(ctx.event, ctx.session, ctx.db).await,
            // ...
        }
    }
}
```

### Step 2: Implement Event Delivery Mechanism
```rust
impl MlsGateway {
    async fn deliver_keypackage_to_requester(
        &self,
        keypackage_event_id: &str,
        session: &Session,
        db: &Database,
    ) -> Result<()> {
        // Get the stored 443 event
        let reader = db.reader()?;
        let event_bytes = hex::decode(keypackage_event_id)?;
        
        if let Some(event) = db.get::<Event, _, _>(&reader, event_bytes)? {
            // Send via the requester's active subscription
            session.send_event(event).await?;
            info!("Delivered KeyPackage {} to requester", keypackage_event_id);
            Ok(())
        } else {
            Err(anyhow!("KeyPackage event not found in database"))
        }
    }
}
```

### Step 3: Modify Relay Core to Pass Session
The relay core needs modification to pass session context to extensions:

```rust
// In relay/src/server.rs or session.rs
impl Session {
    async fn process_event_with_extensions(&mut self, event: Event) -> Result<()> {
        // Create extension context
        let ctx = ExtensionCall {
            event: &event,
            session: self,
            db: &self.db,
        };
        
        // Process through extensions
        for extension in &self.extensions {
            if let ExtensionMessageResult::Consumed = extension.process_event(ctx).await? {
                return Ok(());
            }
        }
        
        // Normal event processing...
    }
}
```

### Step 4: Handle Gift-Wrapped Requests
```rust
async fn handle_giftwrap(&self, event: &Event, session: &Session, db: &Database) -> Result<()> {
    // Try to unwrap (this needs the recipient's private key, which the relay doesn't have)
    // For now, we'll need to handle 447s that aren't gift-wrapped
    // Or implement a special flow where the relay can identify gift-wrapped 447s
    
    // This is a challenge: NIP-59 gift-wraps are end-to-end encrypted
    // The relay cannot decrypt them without the recipient's private key
    // 
    // SOLUTION: Clients should send 447s directly to the relay, not gift-wrapped
    // OR: Use a different approach for async KeyPackage requests
}
```

## Critical Design Decisions

### 1. Gift-Wrapping Challenge
NIP-EE specifies that 447s should be gift-wrapped, but this creates a problem:
- Gift-wrapped events are encrypted to the recipient
- The relay cannot decrypt them without the recipient's private key
- Therefore, the relay cannot intercept and process gift-wrapped 447s

**Proposed Solution**: 
- KeyPackage requests (447) sent TO THE RELAY should NOT be gift-wrapped
- They should be regular signed events so the relay can process them
- The relay acts as a trusted KeyPackage broker

### 2. Authorization Model
- Any authenticated user can request KeyPackages (required for group creation)
- Rate limiting prevents abuse
- KeyPackage owners control their KeyPackage availability

### 3. Delivery Mechanism
- Use the requester's active WebSocket subscription
- Events are pushed as if they came from the network
- Maintains standard Nostr event flow

## Testing Plan

### 1. Unit Test: Request Handler
```rust
#[test]
async fn test_keypackage_request_delivery() {
    // Setup: Store Bob's KeyPackages
    // Action: Alice sends 447 request
    // Assert: Alice receives Bob's 443 events
}
```

### 2. Integration Test: Full Flow
```rust
#[test]
async fn test_offline_keypackage_flow() {
    // 1. Bob publishes KeyPackages
    // 2. Bob goes offline
    // 3. Alice requests Bob's KeyPackages
    // 4. Assert Alice receives them
    // 5. Assert consumption tracking works
}
```

### 3. Load Test: Concurrent Requests
- Multiple users requesting same KeyPackages
- Ensure proper consumption tracking
- Verify last-resort preservation

## Metrics

- `keypackage_requests_received`: Total 447 events
- `keypackage_requests_fulfilled`: Successful deliveries
- `keypackage_requests_empty`: Requests with no packages available
- `keypackage_delivery_latency`: Time from request to delivery

## Next Steps

1. **Immediate**: Implement event delivery mechanism in the 447 handler
2. **Short-term**: Add session context to extension interface
3. **Medium-term**: Resolve gift-wrapping approach for 447s
4. **Long-term**: Add comprehensive testing and metrics

The critical path is implementing the delivery mechanism - without it, the MLS flow is completely broken for offline users.