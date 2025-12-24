# AUTH Flow and KeyPackage Request Relationship

## Summary
**No, the relay does NOT automatically query and send keypackage requests when a client authenticates.**

## AUTH Flow Analysis

### 1. Authentication Process (NIP-42)
When a client connects to the relay:

```
Client → Server: WebSocket connection
Server → Client: ["AUTH", "challenge-uuid"]
Client → Server: ["AUTH", {signed event with challenge}]
Server → Client: ["OK", "event-id", true, ""]
```

### 2. Post-Authentication Behavior
After successful authentication:
- Session stores authenticated pubkey in `AuthState::Pubkey(pubkey)`
- MLS Gateway's `connected()` method only logs: `"Client connected to MLS Gateway: {session_id}"`
- No automatic keypackage requests are triggered

### 3. KeyPackage Request Flow
KeyPackages are only delivered when explicitly requested:

```
Client → Server: ["REQ", "sub-id", {"kinds": [443], "authors": ["target-pubkey"]}]
Server → Client: ["EVENT", "sub-id", {keypackage event}]
Server → Client: ["EOSE", "sub-id"]
```

## Design Rationale

This separation makes sense because:

1. **On-demand consumption**: KeyPackages are consumed when used, so automatic delivery could waste them
2. **Efficiency**: Not all authenticated clients need KeyPackages immediately
3. **Explicit control**: Clients request KeyPackages only when needed for MLS operations (group creation, member addition)
4. **Security**: Prevents unnecessary exposure of KeyPackages

## Implementation Details

### Auth Extension (`extensions/src/auth.rs`)
- Handles NIP-42 authentication challenge/response
- Sets `AuthState::Pubkey(pubkey)` on successful auth
- No keypackage-related logic

### Session Handler (`relay/src/session.rs`)
- Manages WebSocket connection lifecycle
- Calls extension `connected()` callbacks after authentication
- No automatic keypackage requests

### MLS Gateway (`extensions/src/mls_gateway/mod.rs`)
```rust
fn connected(&self, session: &mut Session, _ctx: &mut <Session as actix::Actor>::Context) {
    info!("Client connected to MLS Gateway: {}", session.id());
    // No keypackage requests here
}
```

## Client Responsibilities

Clients must:
1. Authenticate first (NIP-42 AUTH)
2. Explicitly request KeyPackages when needed:
   - Before creating a group (need other users' KeyPackages)
   - When adding members to existing groups
   - When replenishing their own KeyPackage stock

## KeyPackage Replenishment Mechanism

### Current Implementation Status

**Important Finding**: The automatic keypackage replenishment mechanism is **NOT implemented** in the current codebase.

### What the Specification Describes

The NIP-EE-RELAY specification describes a replenishment mechanism where:

1. **Relay Monitoring** (not implemented):
   - Track each user's available KeyPackage count
   - Set threshold (e.g., 3 KeyPackages minimum)
   - Check periodically (e.g., every 5 minutes)

2. **Signaling Low Inventory** (not implemented):
   - When user drops below threshold, relay would send a REQ query
   - `["REQ", "sub-id", {"kinds": [443], "authors": ["user_pubkey"]}]`
   - This would signal to the client that more KeyPackages are needed

3. **Client Response** (client responsibility):
   - Client receives REQ for their own KeyPackages
   - Should interpret this as a signal to replenish
   - Generate and publish 5-10 new KeyPackages

### Current Reality

The actual implementation today:

1. **No Automatic Monitoring**: The relay does not monitor KeyPackage inventory levels
2. **No Proactive Requests**: The relay never initiates requests for more KeyPackages
3. **Client-Driven Process**: Clients are entirely responsible for maintaining their KeyPackage supply

### How Replenishment Works Today

Keypackage replenishment is purely client-driven:
- Clients must track their own KeyPackage inventory
- Clients should monitor when others request their KeyPackages (via REQ queries)
- Frequent queries may indicate high demand
- Clients should publish new KeyPackages before running out

## Current Relay KeyPackage Management

The relay currently implements:

1. **Per-User Limits**:
   - Default: 10 KeyPackages per user (`max_keypackages_per_user: 10`)
   - **Currently rejects new KeyPackages when limit reached**

2. **Automatic Cleanup**:
   - Expires KeyPackages based on `exp` tag
   - Runs daily cleanup of expired KeyPackages
   - Preserves at least one KeyPackage per user (even if expired)

3. **Consumption Tracking**:
   - Non-last-resort KeyPackages marked as consumed when delivered
   - Last KeyPackage never consumed (last-resort protection)
   - Max 2 KeyPackages delivered per query per author

## Proposed Enhancement: Delayed Pruning

Your proposal would enhance the relay to:

1. **Accept new KeyPackages** even when over limit
2. **Start a 5-minute timer** when limit exceeded
3. **After 5 minutes, prune oldest KeyPackages** to get back to limit (e.g., 15)

This would allow:
- Clients to publish 5 KeyPackages on startup without coordination
- Natural rotation of old KeyPackages out
- No rejected KeyPackages during normal operation

### Implementation Approach
```rust
// When new KeyPackage arrives:
if user_keypackage_count > max_per_user {
    // Accept it anyway
    store_keypackage(event);
    
    // Schedule pruning in 5 minutes
    schedule_delayed_pruning(user_pubkey, 5_minutes);
}

// After 5 minutes:
async fn prune_oldest_keypackages(user_pubkey: &str) {
    let keypackages = get_user_keypackages_sorted_by_age(user_pubkey);
    let excess = keypackages.len() - max_per_user;
    
    // Delete the oldest ones
    for kp in keypackages.iter().take(excess) {
        delete_keypackage(kp.id);
    }
}
```

## Client Implementation with Proposed Enhancement

With this relay enhancement, clients can simply:

1. **On startup**: Publish 5 fresh KeyPackages
2. **No pruning needed**: Relay handles it automatically
3. **Monitor inventory**: Replenish when getting low

The relay would:
- Accept the 5 new KeyPackages
- Wait 5 minutes
- Automatically prune the 5 oldest to maintain limit

## Conclusion

**Current state**: Relay rejects new KeyPackages when limit reached

**Proposed enhancement**: Accept new KeyPackages, then prune oldest after 5 minutes

This would create a self-managing system where:
- Clients publish KeyPackages freely
- Old KeyPackages naturally rotate out
- No coordination needed between client and relay limits