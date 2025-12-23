# Revised KeyPackage Delivery Implementation Plan

## Why Long-Term Solution is Better

After analyzing the last resort keypackage logic, the long-term solution is actually the better immediate choice because:

1. **Preserves Consumption Logic**: The system tracks keypackage consumption in Firestore, not LMDB
2. **Maintains Last Resort Protection**: The count-based last resort detection stays in Firestore
3. **No Data Duplication**: Avoids storing full events in both LMDB and Firestore
4. **Clean Separation**: Firestore handles MLS logic, LMDB handles event storage

## How Last Resort Logic Works

1. When querying keypackages, the system checks the count for each user
2. If a user has only 1 keypackage left, it's marked as "last resort" and NOT consumed
3. When new keypackages arrive and user had only 1, a 10-minute timer starts to delete the old one
4. This ensures users always have at least 1 keypackage available

## Recommended Implementation (Updated)

### Step 1: Simple PassThrough Approach

Instead of trying to parse Firestore content as events, we'll:

1. **Let the normal query happen first**
2. **Use Firestore only for consumption tracking**

```rust
fn process_req(&self, session_id: usize, subscription: &Subscription) -> ExtensionReqResult {
    // For keypackage queries, just continue with normal LMDB query
    // We'll handle consumption in post_process_query_results
    if is_keypackage_query(&subscription) {
        ExtensionReqResult::Continue
    }
}
```

### Step 2: Post-Process for Consumption

Implement the `post_process_query_results` method:

```rust
fn post_process_query_results(
    &self,
    session_id: usize,
    subscription: &Subscription,
    events: Vec<Event>,
) -> PostProcessResult {
    // Check if these are keypackages
    let keypackage_events: Vec<Event> = events.iter()
        .filter(|e| e.kind() == 443)
        .collect();
    
    if keypackage_events.is_empty() {
        return PostProcessResult {
            events,
            consumed_events: vec![],
        };
    }
    
    // Track consumption in Firestore
    let consumed = self.process_keypackage_consumption(keypackage_events);
    
    PostProcessResult {
        events, // Return all events
        consumed_events: consumed, // Track which were consumed
    }
}
```

### Step 3: Fix process_keypackage_consumption

```rust
async fn process_keypackage_consumption(&self, events: Vec<Event>) -> Vec<Event> {
    let mut consumed = Vec::new();
    
    for event in events {
        let owner = hex::encode(event.pubkey());
        let event_id = event.id_str();
        
        // Check count in Firestore
        let count = self.store.count_user_keypackages(&owner).await.unwrap_or(0);
        
        if count > 1 {
            // Safe to consume
            if self.store.delete_consumed_keypackage(&event_id).await.unwrap_or(false) {
                consumed.push(event.clone());
            }
        } else {
            // Last resort - don't consume
            info!("Preserving last resort keypackage {} for {}", event_id, owner);
        }
    }
    
    consumed
}
```

## Implementation Steps

### Immediate Fix (Today)

1. **Remove the broken Firestore query logic** in `process_req`
   - Simply return `ExtensionReqResult::Continue` for keypackage queries
   
2. **Implement post-processing** to handle consumption tracking
   - This preserves the last resort logic
   - Uses Firestore only for what it's designed for

3. **Test the flow**:
   ```
   Client REQ → Normal LMDB query → Post-process (consumption tracking) → Return events
   ```

### Benefits of This Approach

1. **Minimal Code Changes**: Remove broken code, add post-processing
2. **Preserves All Logic**: Last resort protection remains intact
3. **No Data Model Changes**: Works with existing storage
4. **Performance**: No double queries or complex joins

### Testing Checklist

- [ ] Verify keypackages are returned to clients
- [ ] Confirm last resort keypackages are NOT consumed
- [ ] Test the 10-minute timer for old keypackage deletion
- [ ] Verify new keypackages trigger last resort transition
- [ ] Check consumption tracking accuracy

## Why This is Better Than Quick Fix

1. **No Data Duplication**: Events stay in LMDB only
2. **Clean Architecture**: Each storage system does what it's designed for
3. **Maintains Existing Logic**: All consumption/last resort logic unchanged
4. **Easier to Debug**: Clear separation of concerns

## Code Changes Required

### 1. Update process_req (simplify it)
```rust
// extensions/src/mls_gateway/mod.rs
fn process_req(&self, session_id: usize, subscription: &Subscription) -> ExtensionReqResult {
    // Just continue - let LMDB handle the query
    ExtensionReqResult::Continue
}
```

### 2. Implement post_process_query_results
```rust
// extensions/src/mls_gateway/mod.rs
fn post_process_query_results(
    &self,
    session_id: usize,
    subscription: &Subscription,
    events: Vec<Event>,
) -> PostProcessResult {
    // Implementation as shown above
}
```

### 3. Remove broken Firestore query code
- Delete the complex async block in current `process_req`
- Remove the event parsing attempt

## Timeline

- **Hour 1**: Remove broken code, implement simple passthrough
- **Hour 2**: Add post-processing with consumption tracking  
- **Hour 3**: Test with real clients
- **Hour 4**: Deploy to staging

This approach is simpler, cleaner, and preserves all the important last resort logic!