# KeyPackage Store-and-Forward Implementation Summary

## Executive Summary

We've implemented the foundation for automatic KeyPackage consumption when they're queried via standard REQ messages. However, the relay's current architecture doesn't support intercepting REQ messages in extensions, which blocks the core functionality.

## What Was Implemented

### 1. KeyPackage Consumption Logic (`keypackage_consumer.rs`)
- ✅ Automatic consumption tracking when KeyPackages are delivered
- ✅ Last-resort protection (never consume the last KeyPackage)
- ✅ Rate limiting: 10 queries/hour per requester-author pair
- ✅ Metrics for tracking consumption and rate limits

### 2. Rate Limiter
- ✅ Sliding window rate limiting implementation
- ✅ Per requester-author pair tracking
- ✅ Configurable limits (10 queries/hour, 2 KeyPackages/query)

### 3. Consumption Tracking
- ✅ Track which KeyPackages have been delivered to which requesters
- ✅ Automatic marking as consumed after delivery
- ✅ Preserve last KeyPackage for each user

### 4. Test Framework
- ✅ Unit tests for rate limiting
- ✅ Test helpers for KeyPackage flow demonstration

## What's Missing (Architectural Limitations)

### 1. REQ Message Interception
The Extension trait doesn't support intercepting REQ messages. Current message flow:
```
Client → REQ → Relay Core → Database → Results → Client
                    ↓
               Extension (only sees EVENT messages)
```

We need:
```
Client → REQ → Relay Core → Extension (intercept) → Database → Results → Client
                               ↓
                        Consumption Logic
```

### 2. Database Access from Extensions
Extensions don't have direct access to the event database, making it impossible to retrieve KeyPackage events for delivery.

## Proposed Solutions

### Option 1: Modify Relay Core (Recommended)
Add REQ interception support to the Extension trait:

```rust
pub trait Extension: Send + Sync {
    // Existing method
    fn message(&self, msg: ClientMessage, ...) -> ExtensionMessageResult;
    
    // NEW: Intercept REQ messages
    fn intercept_req(&self, req: &Subscription, ...) -> Option<Vec<Event>> {
        None // Default: don't intercept
    }
    
    // NEW: Post-process query results
    fn post_process_results(&self, filter: &Filter, events: &[Event], ...) {
        // Track consumption after delivery
    }
}
```

### Option 2: Webhook/Callback System
Create a callback system where the relay notifies extensions after delivering query results:

```rust
// In reader.rs after sending events
if let Some(callback) = extension.get_delivery_callback() {
    callback(filter, events, session_id).await;
}
```

### Option 3: External Service
Move KeyPackage management to a separate service that:
- Receives notifications when KeyPackages are queried
- Tracks consumption separately
- Periodically syncs with the relay

## Integration Guide for Relay Operators

Until the relay core is modified, operators can:

1. **Monitor KeyPackage Supply**: Use the existing metrics to track KeyPackage availability
2. **Clean Up Expired**: The cleanup task already runs hourly
3. **Rate Limiting**: Configure limits in the MLS Gateway config

## Integration Guide for Client Developers

Since kind 447 is deprecated, clients should:

1. **Query KeyPackages Directly**:
   ```json
   ["REQ", "sub_id", {"kinds": [443], "authors": ["target_pubkey"], "limit": 2}]
   ```

2. **Publish Multiple KeyPackages**: Keep at least 3-5 KeyPackages published
3. **Monitor Supply**: Replenish when notified or on a regular schedule

## Next Steps

1. **Immediate**: Document the architectural limitation in NIP-EE-RELAY
2. **Short-term**: Propose Extension trait modification to relay maintainers
3. **Medium-term**: Implement REQ interception in relay core
4. **Long-term**: Full automatic consumption with proactive replenishment

## Code Status

All code compiles but the core functionality (automatic consumption on REQ) cannot work without relay core changes. The implementation is ready to activate once the Extension trait supports REQ interception.