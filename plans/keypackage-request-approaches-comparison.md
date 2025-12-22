# KeyPackage Request Approaches: 447 vs Direct REQ

## The Question
Do we actually need kind 447 events, or can clients just use standard REQ queries to fetch KeyPackages (443)?

## Approach 1: Using Kind 447 (Current Spec)

### How it works:
```
Alice → Relay: EVENT kind:447 requesting Bob's KeyPackages
Relay → Alice: Sends Bob's 443 events via subscription
Relay: Marks KeyPackages as consumed (except last one)
```

### Advantages:
1. **Explicit Consumption Tracking**: Relay knows these KeyPackages are being used
2. **Last-Resort Protection**: Can preserve last KeyPackage automatically  
3. **Rate Limiting**: Can limit requests per user
4. **Audit Trail**: Clear record of who requested whose KeyPackages
5. **Future Extensions**: Can add parameters (min count, ciphersuite preferences)

### Disadvantages:
1. **Complexity**: New event type to implement
2. **Gift-Wrap Issue**: Spec says gift-wrap but relay can't decrypt
3. **State Management**: Relay must track pending deliveries

## Approach 2: Direct REQ Queries (No 447)

### How it works:
```
Alice → Relay: REQ {"kinds":[443], "authors":["bob_pubkey"]}
Relay → Alice: Returns Bob's 443 events
Alice: Uses KeyPackages to create Welcome
```

### Advantages:
1. **Simplicity**: Uses existing Nostr REQ/EVENT flow
2. **No State**: Relay just serves stored events
3. **No Gift-Wrap Issues**: Regular queries aren't encrypted
4. **Already Works**: This would work today without changes

### Disadvantages:
1. **No Consumption Tracking**: KeyPackages never deleted, pile up
2. **No Last-Resort Protection**: Could exhaust all KeyPackages  
3. **No Rate Limiting**: Anyone can query anyone's KeyPackages
4. **Less Semantic**: Just a query, not a "request"

## Hybrid Approach: REQ with Consumption Hints

### How it works:
```
// Alice queries for KeyPackages
Alice → Relay: REQ {"kinds":[443], "authors":["bob_pubkey"], "limit":3}
Relay → Alice: Returns Bob's 443 events

// Alice tells relay which ones she used
Alice → Relay: EVENT kind:447 {"consumed": ["event_id_1", "event_id_2"]}
Relay: Marks those KeyPackages as consumed
```

### This gives us:
- Simple queries using standard REQ
- Optional consumption tracking
- Best of both worlds

## Recommendation

For immediate implementation, **go with Approach 2 (Direct REQ)** because:

1. **It works today** - No relay changes needed for basic functionality
2. **Clients can implement immediately** - Just query for 443s
3. **Incremental improvement** - Can add 447 for consumption later

Then enhance with consumption tracking:

```typescript
// Phase 1: Just use REQ (works now)
const keyPackages = await relay.req({
  kinds: [443],
  authors: [bobPubkey],
  limit: 3
});

// Phase 2: Add consumption notification (future)
await relay.publish({
  kind: 447,
  content: "",
  tags: [
    ["p", bobPubkey],
    ["consumed", eventId1],
    ["consumed", eventId2]
  ]
});
```

## Implementation Path

### Immediate (Week 1):
1. Document that clients should REQ for 443s directly
2. Ensure relay indexes 443 events by author
3. Test basic flow works

### Enhancement (Week 2-3):
1. Add 447 as consumption notification (not request)
2. Implement last-resort protection
3. Add metrics and monitoring

### Future (Month 2+):
1. Rate limiting based on REQ patterns
2. Intelligent KeyPackage rotation
3. Ciphersuite filtering

## Conclusion

You're right - **we don't strictly need kind 447 for basic KeyPackage retrieval**. Standard REQ queries work fine. 

Kind 447 becomes valuable for:
- Consumption tracking
- Last-resort protection  
- Rate limiting
- Audit trails

But these are enhancements, not requirements for basic MLS functionality.