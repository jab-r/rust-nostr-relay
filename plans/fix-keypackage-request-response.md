# Fix KeyPackage Request Response Flow

## Problem Statement
The current keypackage request (kind 447) implementation diverges from the NIP-EE-RELAY specification, causing a critical gap where clients cannot discover which keypackages are available after making a request.

## Specification vs Implementation Mismatch

### NIP-EE-RELAY Specification (Kind 447)
According to NIP-EE-RELAY, kind 447 should be:
- A gift-wrapped notification from Alice to Bob
- Requesting Bob to publish new keypackages
- Bob then publishes new kind 443 events
- Alice discovers them by querying Bob's keypackage relays

### Current Server Implementation
Our server treats kind 447 as:
- A direct server request (not gift-wrapped)
- Server processes it and marks existing keypackages as available
- No response mechanism to notify the requester
- Client times out waiting for keypackages

## The Core Issue

The server is implementing a **server-mediated keypackage discovery** flow, while NIP-EE-RELAY defines a **peer-to-peer notification** flow. This fundamental mismatch causes:

1. Clients send kind 447 to the server expecting a response
2. Server processes the request but has no defined way to respond
3. Clients can't discover which keypackages were made available
4. The flow fails despite the server successfully finding keypackages

## Analysis of Current Flow

From the logs:
1. Client "jb" (userId: f82d4f97...) tries to create a group
2. Client requests keypackages for recipient a8c3402c... (via kind 447)
3. Server successfully finds and marks keypackages as available
4. Server preserves the last keypackage (correct behavior)
5. **GAP**: No mechanism to inform the client about available keypackages
6. Client times out after 8 seconds

## Proposed Solutions

### Option 1: Align with NIP-EE-RELAY Specification
Transform kind 447 into a proper gift-wrapped peer notification:
- Server stops processing kind 447 requests directly
- Kind 447 becomes a user-to-user notification only
- Recipients publish new keypackages upon receiving notifications
- Requires significant client-side changes

### Option 2: Server-Specific Request/Response (Recommended)
Keep the server-mediated flow but add a response mechanism:

#### New Event Kind 448: KeyPackage Response
```json
{
  "kind": 448,
  "pubkey": "<relay_system_pubkey>",
  "tags": [
    ["p", "<requester_pubkey>"],     // Who this response is for
    ["e", "<request_event_id>"],     // Original 447 request
    ["recipient", "<recipient_pubkey>"], // Who the keypackages are from
    ["kp", "<keypackage_event_id_1>"],  // Available keypackage
    ["kp", "<keypackage_event_id_2>"],  // Another available keypackage
    ["status", "available"],         // Status of the request
    ["total", "2"],                  // Total keypackages found
    ["available", "2"],              // How many can be retrieved
    ["consumed", "1"],               // How many will be consumed
    ["preserved", "1"]               // How many preserved (last resort)
  ],
  "content": "",
  "created_at": <timestamp>,
  "sig": "..."
}
```

### Option 3: Direct KeyPackage Push
After processing kind 447, the server could:
1. Query the available kind 443 events
2. Push them directly to the requester's active subscription
3. No new event kind needed
4. Requires subscription tracking

## Recommendation: Option 2 with Kind 448 Response

This approach:
- Maintains backward compatibility with existing client behavior
- Provides explicit feedback about keypackage availability
- Preserves the server-mediated discovery benefits
- Aligns with the relay's role as an MLS gateway
- Creates a clear audit trail

## Understanding the Intended Flow

Based on clarification, the server implements a **keypackage caching and mediation** strategy:

1. **Client Alice** publishes kind 447 to obtain Bob's keypackage
2. **Server intercepts** the request and returns a cached Bob KeyPackage (if available)
3. **When Bob's count is low**, server publishes kind 447 to Bob requesting new KeyPackages
4. **Bob uploads** new KeyPackages (kind 443) when he receives the notification

This is a hybrid approach that combines server-side caching with the peer notification mechanism from NIP-EE-RELAY.

## The Missing Piece

The server successfully processes requests but has no way to tell Alice which specific keypackages are now available for retrieval. The server logs show:
- Keypackages are found and marked as available
- Last keypackage is properly preserved
- But Alice never learns which event IDs to query

## Implementation Plan - Using Existing Event Kinds

### The Solution: Direct KeyPackage Delivery

After the server processes a kind 447 request and identifies available keypackages, it should directly push those specific kind 443 events to the requester. No new event kind needed!

### Server-Side Changes

1. **Modify `handle_keypackage_request` to Push KeyPackages**

```rust
// After line 1055 in extensions/src/mls_gateway/mod.rs
// Instead of just logging, actively deliver the keypackages

if !keypackages_to_return.is_empty() {
    // Query the actual kind 443 events that were made available
    let keypackage_events = self.query_events(
        vec![443],                    // kind 443
        Some(&keypackages_to_return), // specific event IDs
        None                          // no other filters
    ).await?;
    
    // Push these events to the requester's active subscription
    for event in keypackage_events {
        self.send_event_to_subscriber(&event_pubkey, event).await?;
    }
    
    info!("Pushed {} keypackage events to requester {}",
          keypackage_events.len(), event_pubkey);
}

// If count is low, trigger replenishment via gift-wrapped 447
if total_count - consumed_count < KEYPACKAGE_LOW_THRESHOLD {
    self.send_keypackage_request_to_user(&recipient_pubkey).await?;
}
```

2. **Add Subscription Tracking**

The server needs to track active subscriptions to know where to send events:

```rust
// Add to MlsGateway struct
pub struct MlsGateway {
    // ... existing fields ...
    active_subscriptions: Arc<RwLock<HashMap<String, SubscriptionInfo>>>,
}

// Track subscriptions when clients connect
pub async fn track_subscription(&self, pubkey: &str, subscription_id: &str) {
    let mut subs = self.active_subscriptions.write().await;
    subs.insert(pubkey.to_string(), SubscriptionInfo {
        subscription_id: subscription_id.to_string(),
        connected_at: Utc::now(),
    });
}
```

### Client-Side Flow

1. **Client subscribes to kind 443 events BEFORE sending request**
```javascript
// Set up subscription for keypackages
const keypackageFilter = {
    kinds: [443],
    authors: [recipientPubkey],
    since: Math.floor(Date.now() / 1000) - 10 // Recent events
};

// Subscribe and wait for events
const subscription = relay.sub(keypackageFilter);

// Send the kind 447 request
const request = {
    kind: 447,
    tags: [
        ["p", recipientPubkey],
        ["min", "1"],
        // other tags...
    ],
    content: "",
    // ... sign and publish
};

// Events will arrive via the existing subscription
subscription.on('event', (event) => {
    // Process received keypackage
    console.log('Received keypackage:', event.id);
});
```

### Benefits of Direct Push Approach

1. **NIP-EE-RELAY Compliant** - Follows the specification exactly
2. **Immediate Delivery** - No polling or secondary queries needed
3. **Simple Client Logic** - Subscribe to kind 443, send request, receive keypackages
4. **Efficient** - One subscription handles the entire flow
5. **Clear Intent** - KeyPackages arrive as kind 443 events, as documented

### Server Replenishment Logic

When keypackage count is low:
1. Server creates a gift-wrapped kind 447 to Bob (the keypackage owner)
2. Bob's client receives notification and uploads new keypackages
3. Server caches the new keypackages for future requests

## Testing Plan

1. **Happy Path**
   - Alice subscribes to kind 443 events
   - Alice requests Bob's keypackage via kind 447
   - Server pushes available kind 443 events to Alice
   - Verify consumption and preservation logic

2. **Low Stock Trigger**
   - Deplete Bob's keypackages to threshold
   - Verify server sends gift-wrapped kind 447 to Bob
   - Bob uploads new keypackages
   - Verify server caches them

3. **Edge Cases**
   - No keypackages available (no events pushed)
   - Last keypackage preservation
   - Client not subscribed (events queued or dropped)
   - Concurrent requests

## Implementation Timeline
- Server-side push mechanism: 3-4 hours
- Subscription tracking: 2 hours
- Testing: 2-3 hours
- Documentation updates: 1 hour

## Summary

The fix is straightforward: when the server processes a kind 447 request and identifies available keypackages, it should immediately push those specific kind 443 events to the requesting client's active subscription. This aligns perfectly with NIP-EE-RELAY expectations and requires no new event kinds or alternative approaches.