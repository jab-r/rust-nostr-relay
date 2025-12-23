# KeyPackage Delivery Fix Design

## Problem Summary
The MLS Gateway extension cannot deliver keypackages to clients because it's trying to parse Firestore's `content` field as a complete Nostr event, when it actually contains only the raw keypackage data.

## Current Architecture

### Storage Layers
1. **LMDB** - Primary event storage
   - Stores complete Nostr events with all metadata
   - Events are indexed by ID
   
2. **Firestore** - Auxiliary storage for MLS-specific queries
   - Stores structured keypackage metadata
   - Enables efficient queries by owner, expiry, etc.
   - Does NOT store complete events

### Current Flow (Broken)
1. Client sends REQ for kind 443
2. Extension intercepts and queries Firestore
3. Firestore returns metadata with `content` field
4. Extension tries to parse `content` as Event ❌
5. Parsing fails, no events returned

## Proposed Solution

### Option 1: Query LMDB After Firestore (Recommended)
```
Client REQ → Extension → Firestore (get event IDs) → LMDB (get full events) → Client
```

**Advantages:**
- Maintains existing Firestore query benefits
- Returns complete, valid Nostr events
- Minimal code changes needed

**Implementation Steps:**
1. Modify `process_req` to return event IDs instead of trying to construct events
2. Let the relay's normal query process handle fetching from LMDB
3. Use Firestore results to filter which events to return

### Option 2: Store Complete Events in Firestore
```
Store full event JSON in Firestore content field
```

**Advantages:**
- Simple to implement
- No LMDB query needed

**Disadvantages:**
- Duplicates data between LMDB and Firestore
- Increases storage costs
- Potential consistency issues

### Option 3: Skip Firestore for REQ Queries
```
Use Firestore only for storage, let LMDB handle all queries
```

**Advantages:**
- Simplest implementation
- No cross-storage queries

**Disadvantages:**
- Loses Firestore query optimizations
- May not scale well

## Recommended Implementation Plan

### Phase 1: Quick Fix
Modify the `process_req` method to properly handle the Firestore response:

```rust
// In process_req method
let firestore_metadata = query_firestore(...).await?;
let event_ids: Vec<String> = firestore_metadata.iter()
    .map(|(event_id, _, _, _)| event_id.clone())
    .collect();

// Return hint to filter by these specific event IDs
ExtensionReqResult::Continue // Let LMDB handle the actual query
```

### Phase 2: Proper Integration
1. Extend the Extension trait to support event ID filtering
2. Pass event IDs from Firestore to the LMDB query
3. Implement consumption tracking after successful delivery

### Phase 3: Optimization
1. Add caching layer for frequently requested keypackages
2. Implement batch queries for multiple authors
3. Add metrics for query performance

## Code Changes Required

### 1. Extension trait enhancement
```rust
pub enum ExtensionReqResult {
    Continue,
    AddEvents(Vec<Event>),
    Handle(Vec<Event>),
    // New variant for filtering
    FilterByIds(Vec<String>), 
}
```

### 2. MLS Gateway process_req
```rust
fn process_req(&self, session_id: usize, subscription: &Subscription) -> ExtensionReqResult {
    // Query Firestore for metadata
    let metadata = self.query_firestore_sync(...);
    
    // Extract event IDs
    let event_ids = metadata.into_iter()
        .map(|(id, _, _, _)| id)
        .collect();
    
    // Return IDs for filtering
    ExtensionReqResult::FilterByIds(event_ids)
}
```

### 3. Reader modification
Update the reader to handle `FilterByIds` and query LMDB for specific events.

## Testing Strategy

1. **Unit Tests**
   - Test Firestore query returns correct metadata
   - Test event ID extraction
   - Test LMDB query with ID filter

2. **Integration Tests**
   - Test full flow from REQ to event delivery
   - Test multiple keypackages from different authors
   - Test consumption tracking

3. **Load Tests**
   - Test performance with many concurrent requests
   - Verify no regression in query speed

## Rollback Plan

If issues arise:
1. Remove the extension interceptor for kind 443
2. Let standard LMDB queries handle keypackages
3. Debug and fix offline

## Success Criteria

- Clients can retrieve keypackages via REQ messages
- Keypackages are properly consumed after delivery
- No performance regression
- All existing tests pass