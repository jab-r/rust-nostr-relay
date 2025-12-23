# KeyPackage Delivery Implementation Plan

## Objective
Fix the keypackage delivery system so clients can retrieve keypackages (kind 443) via REQ messages to create MLS groups.

## Implementation Strategy

We'll implement a **quick fix** that works with the current architecture, followed by a proper long-term solution.

### Quick Fix (Immediate)

**Approach:** Store the full event JSON in Firestore's `content` field instead of just the raw keypackage data.

**Why this works:**
- Minimal code changes
- Immediate resolution
- Maintains backward compatibility

**Changes needed:**

1. **In `handle_keypackage` method when storing to Firestore:**
   ```rust
   // Instead of storing just the keypackage content
   // Store the entire event as JSON
   let event_json = serde_json::to_string(&event)?;
   store.store_keypackage(
       &event_id,
       &owner_pubkey,
       &event_json,  // Store full event JSON instead of just content
       ...
   ).await?;
   ```

2. **No changes needed in `process_req`:**
   - The existing parsing logic will work since it expects full event JSON

### Long-term Solution (Phase 2)

**Approach:** Properly integrate Firestore metadata queries with LMDB event retrieval.

**Implementation steps:**

1. **Modify Extension trait to support event filtering:**
   ```rust
   pub struct FilteredQuery {
       pub event_ids: Vec<String>,
       pub original_filters: Vec<Filter>,
   }
   
   pub enum ExtensionReqResult {
       Continue,
       AddEvents(Vec<Event>),
       Handle(Vec<Event>),
       FilterQuery(FilteredQuery), // New variant
   }
   ```

2. **Update MLS Gateway `process_req`:**
   ```rust
   fn process_req(&self, session_id: usize, subscription: &Subscription) -> ExtensionReqResult {
       // Query Firestore for metadata
       let event_ids = self.query_firestore_for_ids(...);
       
       ExtensionReqResult::FilterQuery(FilteredQuery {
           event_ids,
           original_filters: subscription.filters.clone(),
       })
   }
   ```

3. **Modify Reader to handle filtered queries:**
   - Check for `FilterQuery` result
   - Query LMDB for specific event IDs
   - Apply original filters as secondary validation

## Step-by-Step Implementation

### Day 1: Quick Fix
1. [ ] Modify `handle_keypackage` to store full event JSON
2. [ ] Test with local deployment
3. [ ] Verify clients can retrieve keypackages
4. [ ] Deploy to staging

### Day 2: Testing & Validation
1. [ ] Run integration tests
2. [ ] Test MLS group creation flow
3. [ ] Monitor logs for any errors
4. [ ] Deploy to production if stable

### Week 2: Long-term Solution
1. [ ] Design Extension trait changes
2. [ ] Implement FilteredQuery support
3. [ ] Update MLS Gateway process_req
4. [ ] Modify Reader to handle filtered queries
5. [ ] Comprehensive testing
6. [ ] Performance benchmarking

## File Changes

### Quick Fix Files
1. `extensions/src/mls_gateway/groups.rs` - Update `handle_keypackage` method
2. `extensions/src/mls_gateway/firestore.rs` - Verify storage method handles full JSON

### Long-term Solution Files
1. `relay/src/extension.rs` - Add FilteredQuery support
2. `extensions/src/mls_gateway/mod.rs` - Update process_req implementation
3. `relay/src/reader.rs` - Handle filtered queries
4. `relay/src/message.rs` - Add FilteredQuery message type

## Testing Plan

### Unit Tests
- Test storing full event JSON in Firestore
- Test parsing retrieved events
- Test filtered query creation

### Integration Tests
```rust
#[tokio::test]
async fn test_keypackage_retrieval() {
    // 1. Store keypackages
    // 2. Send REQ for kind 443
    // 3. Verify events returned
    // 4. Check consumption tracking
}
```

### Manual Testing
1. Deploy to local environment
2. Use test client to:
   - Upload keypackages
   - Query for keypackages
   - Create MLS group
   - Verify group messaging works

## Rollback Strategy

### If Quick Fix Fails
1. Disable keypackage interception in process_req
2. Let standard LMDB queries handle kind 443
3. Debug offline

### Monitoring
- Watch for "Failed to parse KeyPackage event" errors
- Monitor successful keypackage retrievals
- Track MLS group creation success rate

## Success Metrics

1. **Immediate (24 hours)**
   - Zero "Failed to parse KeyPackage event" errors
   - Clients successfully retrieve keypackages
   - MLS group creation works

2. **Short-term (1 week)**
   - 100% keypackage query success rate
   - No performance degradation
   - Consumption tracking functional

3. **Long-term (1 month)**
   - Improved query performance with filtered queries
   - Reduced Firestore costs
   - Clean separation of concerns

## Risk Assessment

### Low Risk
- Quick fix is backward compatible
- Easy to rollback
- Minimal code changes

### Medium Risk
- Increased Firestore storage usage
- Potential for data duplication

### Mitigation
- Monitor Firestore costs
- Plan migration to long-term solution
- Keep rollback procedure ready

## Next Actions

1. **Immediate:** Implement quick fix in development branch
2. **Today:** Test locally with real keypackage data
3. **Tomorrow:** Deploy to staging after successful tests
4. **This week:** Monitor production deployment
5. **Next week:** Begin long-term solution implementation