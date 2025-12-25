# Client Guide for KeyPackage Queries

## Overview

This guide explains how clients should query KeyPackages from the NIP-EE-RELAY implementation. KeyPackages are essential for adding new members to MLS groups, and the relay provides automatic consumption tracking to ensure secure distribution.

## Key Points

- **Yes, the KeyPackage limit applies to EACH user individually**
- You can request KeyPackages from multiple users in a single query
- Default: 1 KeyPackage per user per query (can be configured up to 2)
- Rate limit: 10 queries per hour per requester-author pair

### Encoding (client compatibility)

- **Clients SHOULD publish KeyPackages with `encoding=base64`** in the `tags` and base64 in `content`.
- **The relay will accept both encodings on ingest**:
  - If the `encoding` tag is absent, the relay treats `content` as **hex** (legacy behavior)
  - If `encoding=base64`, the relay treats `content` as **base64**
- **The relay stores KeyPackages canonically as base64 internally** (efficiency).
- **When querying via REQ, the relay returns KeyPackages with hex `content` by default** for backward compatibility with deployed clients.

#### Requesting base64 output via REQ

Clients that want base64 output can add a format hint using a standard single-letter tag filter:

```json
["REQ", "kp_query_b64", {
  "kinds": [443],
  "authors": ["alice_hex_pubkey"],
  "#f": ["base64"]
}]
```

When `#f:["base64"]` is present on a `kind:443` query:

- returned 443 events use **base64** in `content`
- returned events include tag `["encoding","base64"]`

If `#f` is absent, the relay returns **hex** in `content` (default).

## Query Format

### Basic Query Structure

```json
["REQ", "subscription_id", {
  "kinds": [443],
  "authors": ["hex_pubkey1", "hex_pubkey2", ...],
  "limit": 100
}]
```

### Single User Query

To request KeyPackages from a single user:

```json
["REQ", "kp_query_1", {
  "kinds": [443],
  "authors": ["alice_hex_pubkey"]
}]
```

**Response**: 1 KeyPackage from Alice (default)

### Multiple User Query

To request KeyPackages from multiple users in one request:

```json
["REQ", "kp_query_2", {
  "kinds": [443],
  "authors": [
    "alice_hex_pubkey",
    "bob_hex_pubkey",
    "charlie_hex_pubkey"
  ]
}]
```

**Response**: 1 KeyPackage from EACH user by default (3 total in this example)

## Implementation Details

### Per-User Limits

When you query multiple authors:
- Each author returns 1 KeyPackage by default
- The limit is applied individually to each author
- If Alice has 5 KeyPackages and Bob has 1, you'll get 1 from each
- Can be configured to return up to 2 per author if needed

### Rate Limiting

Rate limits are tracked per requester-author pair:
- 10 queries per hour for each unique (your_pubkey, target_pubkey) combination
- Querying 3 users counts as 3 separate rate limit checks
- Each user's rate limit is independent

Example:
- You can query Alice 10 times per hour
- You can query Bob 10 times per hour
- These limits don't affect each other

### Automatic Consumption

KeyPackages are automatically marked as consumed when delivered:
- The relay tracks which KeyPackages have been sent to which requesters
- Last-resort protection: The last KeyPackage is never consumed
- No manual consumption requests needed (kind 447 is deprecated)

## Best Practices

### 1. Batch Requests When Possible

Instead of:
```json
// ❌ Multiple separate requests
["REQ", "sub1", {"kinds": [443], "authors": ["alice"]}]
["REQ", "sub2", {"kinds": [443], "authors": ["bob"]}]
["REQ", "sub3", {"kinds": [443], "authors": ["charlie"]}]
```

Do:
```json
// ✅ Single batched request
["REQ", "sub1", {
  "kinds": [443], 
  "authors": ["alice", "bob", "charlie"]
}]
```

### 2. Handle Partial Results

Not all requested users may have KeyPackages available:

```javascript
// Example client code
const response = await queryKeyPackages(["alice", "bob", "charlie"]);

// Check which users returned KeyPackages
const userKeyPackages = {};
response.events.forEach(event => {
  const author = event.pubkey;
  if (!userKeyPackages[author]) {
    userKeyPackages[author] = [];
  }
  userKeyPackages[author].push(event);
});

// Handle missing KeyPackages
["alice", "bob", "charlie"].forEach(user => {
  if (!userKeyPackages[user] || userKeyPackages[user].length === 0) {
    console.log(`No KeyPackages available for ${user}`);
    // Consider alternative actions or retry later
  }
});
```

### 3. Respect Rate Limits

Implement exponential backoff when rate limited:

```javascript
async function queryWithRetry(authors, maxRetries = 3) {
  for (let attempt = 0; attempt < maxRetries; attempt++) {
    try {
      return await queryKeyPackages(authors);
    } catch (error) {
      if (error.rateLimited && attempt < maxRetries - 1) {
        const backoff = Math.pow(2, attempt) * 1000;
        await sleep(backoff);
        continue;
      }
      throw error;
    }
  }
}
```

### 4. Monitor KeyPackage Availability

Track which users frequently run out of KeyPackages:

```javascript
// Track success rates
const keyPackageStats = {};

function updateStats(user, success) {
  if (!keyPackageStats[user]) {
    keyPackageStats[user] = { attempts: 0, successes: 0 };
  }
  keyPackageStats[user].attempts++;
  if (success) {
    keyPackageStats[user].successes++;
  }
}

// Notify users with low availability
Object.entries(keyPackageStats).forEach(([user, stats]) => {
  const successRate = stats.successes / stats.attempts;
  if (successRate < 0.5) {
    notifyUserToReplenishKeyPackages(user);
  }
});
```

## Common Scenarios

### Group Creation

When creating a group with multiple members:

```json
// Query KeyPackages for all initial members
["REQ", "group_create", {
  "kinds": [443],
  "authors": ["member1", "member2", "member3", "member4", "member5"]
}]
```

Default response: 5 KeyPackages (1 per member)

### Adding Members to Existing Group

When adding new members to an existing group:

```json
// Query KeyPackages for new members only
["REQ", "add_members", {
  "kinds": [443],
  "authors": ["new_member1", "new_member2"]
}]
```

Default response: 2 KeyPackages (1 per new member)

### Retry Failed Additions

If some members couldn't be added due to missing KeyPackages:

```json
// Retry only for users who failed
["REQ", "retry_add", {
  "kinds": [443],
  "authors": ["failed_user1", "failed_user2"]
}]
```

## Error Handling

### No KeyPackages Available

If a user has no KeyPackages:
- The response will not include any events for that user
- Client should handle this gracefully
- Consider notifying the user to publish new KeyPackages

### Rate Limit Exceeded

If rate limited:
- The relay may return an error or empty result
- Implement exponential backoff
- Track rate limits client-side to avoid hitting limits

### Network Errors

Always implement proper error handling:
- Connection timeouts
- WebSocket disconnections
- Invalid responses

## Security Considerations

1. **Verify KeyPackage Signatures**: Always verify the KeyPackage signatures match the claimed author
2. **Track Consumption**: Keep track of which KeyPackages you've used to avoid reuse
3. **Fresh Queries**: Don't cache KeyPackages for extended periods; query fresh ones when needed
4. **Secure Storage**: If temporarily storing KeyPackages, ensure they're encrypted at rest

## Migration from Kind 447

The deprecated kind 447 request-response pattern is no longer needed:

```json
// ❌ Old way (deprecated)
["EVENT", {...kind: 447, content: encrypted_request}]

// ✅ New way
["REQ", "sub_id", {"kinds": [443], "authors": ["target"]}]
```

Benefits of the new approach:
- Simpler client implementation
- Automatic consumption tracking
- Better performance (no extra round trip)
- Standard Nostr query pattern

## Debugging Tips

1. **Check Event Logs**: Monitor relay logs for query processing
2. **Verify Rate Limits**: Track your query frequency
3. **Validate Filters**: Ensure author pubkeys are correctly formatted
4. **Monitor Metrics**: Use relay metrics to track KeyPackage availability

## Example Implementation

Here's a complete example of a KeyPackage query client:

```javascript
class KeyPackageClient {
  constructor(relay) {
    this.relay = relay;
    this.rateLimits = new Map();
  }

  async queryKeyPackages(targetPubkeys, subscriptionId = null) {
    const subId = subscriptionId || `kp_${Date.now()}`;
    
    // Build filter
    const filter = {
      kinds: [443],
      authors: targetPubkeys
    };
    
    // Send request
    const request = ["REQ", subId, filter];
    await this.relay.send(JSON.stringify(request));
    
    // Collect responses
    const keyPackages = new Map();
    const timeout = setTimeout(() => {
      this.relay.send(JSON.stringify(["CLOSE", subId]));
    }, 5000);
    
    return new Promise((resolve) => {
      this.relay.on('message', (msg) => {
        const parsed = JSON.parse(msg);
        
        if (parsed[0] === 'EVENT' && parsed[1] === subId) {
          const event = parsed[2];
          if (!keyPackages.has(event.pubkey)) {
            keyPackages.set(event.pubkey, []);
          }
          keyPackages.get(event.pubkey).push(event);
        }
        
        if (parsed[0] === 'EOSE' && parsed[1] === subId) {
          clearTimeout(timeout);
          this.relay.send(JSON.stringify(["CLOSE", subId]));
          resolve(keyPackages);
        }
      });
    });
  }

  async queryWithRateLimit(targetPubkeys) {
    // Check rate limits
    const now = Date.now();
    const allowedTargets = targetPubkeys.filter(pubkey => {
      const lastQuery = this.rateLimits.get(pubkey) || 0;
      return now - lastQuery > 360000; // 6 minutes minimum between queries
    });
    
    if (allowedTargets.length === 0) {
      throw new Error('All targets are rate limited');
    }
    
    // Update rate limit tracking
    allowedTargets.forEach(pubkey => {
      this.rateLimits.set(pubkey, now);
    });
    
    // Query only allowed targets
    return this.queryKeyPackages(allowedTargets);
  }
}
```

## Summary

- Query multiple users' KeyPackages in a single request for efficiency
- Each user returns up to 2 KeyPackages independently
- Rate limits apply per requester-author pair
- Automatic consumption happens transparently
- Always handle partial results and missing KeyPackages gracefully
