# NIP-EE Modification Proposal: KeyPackage Request (Kind 447)

## Problem Statement

The current NIP-EE-RELAY specification requires KeyPackage Request Events (kind 447) to be gift-wrapped per NIP-59. This creates a fundamental incompatibility with relay-based KeyPackage store-and-forward functionality:

1. **Gift-wrapped events are end-to-end encrypted** to the recipient
2. **Relays cannot decrypt** these events without the recipient's private key
3. **Relays cannot intercept and process** gift-wrapped 447 requests
4. **The async MLS flow breaks** when the recipient is offline

## Proposed Solution

Modify the specification for kind 447 events to NOT require gift-wrapping when sent to relays that implement KeyPackage store-and-forward functionality.

## Rationale

### Why Gift-Wrapping Was Originally Specified

Gift-wrapping was likely specified to:
- Hide metadata about who is requesting whose KeyPackages
- Prevent observers from tracking group formation patterns
- Maintain consistency with other sensitive events (444)

### Why Gift-Wrapping Doesn't Work Here

1. **Relay Must Read the Request**: To function as a store-and-forward service, the relay MUST be able to:
   - Identify that this is a KeyPackage request
   - Extract the recipient's pubkey
   - Query stored KeyPackages
   - Deliver them to the requester

2. **Different Trust Model**: KeyPackage requests have a different privacy model than Welcome messages:
   - Welcome messages contain sensitive group keys (must be encrypted)
   - KeyPackage requests only reveal "Alice wants Bob's public KeyPackages"
   - KeyPackages themselves are already public (kind 443)

3. **Timing Correlation Exists Anyway**: Even with gift-wrapping:
   - Observer sees Alice send a gift-wrapped event
   - Observer sees relay respond with Bob's KeyPackages
   - The correlation is obvious

## Proposed Specification Change

### Current Specification (Problematic)
```json
{
   "id": <id>,
   "kind": 447,
   "created_at": <unix timestamp in seconds>,
   "pubkey": <nostr identity pubkey of sender>,
   "content": "",
   "tags": [
      ["p", <pubkey of the recipient who should publish new KeyPackages>],
      ["ciphersuite", <optional preferred MLS CipherSuite ID>],
      ["extensions", <optional array of required MLS Extension IDs>],
      ["reason", <optional human-readable reason>]
   ],
   "sig": <NOT SIGNED>
}
// Then sealed and gift-wrapped per NIP-59
```

### Proposed Specification (Relay-Compatible)
```json
{
   "id": <id>,
   "kind": 447,
   "created_at": <unix timestamp in seconds>,
   "pubkey": <requester's pubkey>,
   "content": "",
   "tags": [
      ["p", <pubkey of user whose KeyPackages are requested>],
      ["relay", <relay URL implementing store-and-forward>],
      ["ciphersuite", <optional preferred MLS CipherSuite ID>],
      ["extensions", <optional array of required MLS Extension IDs>],
      ["min", <optional minimum number of KeyPackages requested>],
      ["expiration", <optional unix timestamp when request expires>]
   ],
   "sig": <signed by requester>
}
```

### Key Changes

1. **Direct Events**: Kind 447 sent as regular signed Nostr events
2. **Relay Tag**: Explicitly indicates which relay should handle the request
3. **Signed**: Provides authenticity and non-repudiation
4. **Public**: Acknowledges that KeyPackage requests are not highly sensitive

## Privacy Analysis

### What This Reveals
- Alice is requesting Bob's KeyPackages
- Alice might be creating a group with Bob
- The timing of group formation activity

### What This Does NOT Reveal
- The group ID or any group metadata
- The actual MLS Welcome message (still gift-wrapped)
- The content of any messages
- Whether Bob actually joins the group

### Comparison with Alternatives

1. **Status Quo (Broken)**: Perfect metadata privacy but system doesn't work
2. **Direct Requests**: Some metadata leakage but system works
3. **Alternative**: Proxy requests through relay's own identity (complex, breaks authenticity)

## Implementation Guidelines

### For Clients

1. **Discovery**: Query relay capabilities to identify store-and-forward support
2. **Request Flow**:
   ```
   // Create signed 447 request
   const request = {
     kind: 447,
     pubkey: myPubkey,
     created_at: now(),
     tags: [
       ["p", targetPubkey],
       ["relay", "wss://relay.example.com"],
       ["min", "3"]
     ],
     content: ""
   };
   
   // Sign and send to relay
   const signed = signEvent(request, myPrivateKey);
   relay.publish(signed);
   ```

3. **Response Handling**: Listen for 443 events in response

### For Relays

1. **Intercept 447 Events**: Process instead of just storing/forwarding
2. **Query KeyPackages**: Find matching 443 events for requested user
3. **Deliver to Requester**: Push via active subscription
4. **Track Consumption**: Mark KeyPackages as used (except last-resort)

## Migration Path

### Phase 1: Relay Implementation
- Relays implement handler for non-gift-wrapped 447s
- Maintain backward compatibility (store gift-wrapped ones)

### Phase 2: Client Adoption
- Clients check relay capabilities
- Send direct 447s to compatible relays
- Fall back to gift-wrapped for old relays

### Phase 3: Specification Update
- Update NIP-EE-RELAY with this modification
- Document relay capability negotiation

## Alternative Approaches Considered

### 1. Relay-Specific Private Keys
- Relay has a keypair for unwrapping specific events
- Clients encrypt 447s to relay's pubkey
- **Rejected**: Adds complexity, breaks standard NIP-59 flow

### 2. Cleartext Request Identifier
- Add unencrypted "request hint" to gift-wrapped 447s
- **Rejected**: Defeats purpose of gift-wrapping

### 3. Out-of-Band Requests
- Use REST API or other channel for requests
- **Rejected**: Breaks Nostr-native flow

### 4. Client-Side Polling
- Clients periodically check for new KeyPackages
- **Rejected**: Inefficient, doesn't scale

## Security Considerations

### Request Authentication
- Signed 447s provide non-repudiation
- Relays can implement access control
- Rate limiting prevents abuse

### KeyPackage Freshness
- Requests can include timestamp requirements
- Relays can filter stale KeyPackages
- Clients can request minimum quantities

### Relay Trust
- Clients explicitly choose relays for store-and-forward
- Multiple relays can be used for redundancy
- KeyPackages are public data anyway

## Conclusion

This modification acknowledges that:
1. KeyPackage requests have different privacy requirements than sensitive key material
2. Relay-based store-and-forward is essential for async MLS flows
3. Some metadata leakage is acceptable for a functioning system

The proposed change maintains the security properties of MLS while enabling practical deployment on Nostr infrastructure.