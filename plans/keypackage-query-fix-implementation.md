# KeyPackage Query Fix Implementation Plan

## Problem Summary

Two issues with KeyPackage queries (REQ for kind 443):

1. **Signature field error**: Event reconstruction is providing an empty string for the signature field, but the Event deserialization expects a 128-character hex string (64 bytes).
2. **Too many keypackages returned**: The query is returning 44 keypackages when it should only return 1-2 per author according to the `max_keypackages_per_query` configuration.

## Root Causes

### 1. Signature Field Error
In `extensions/src/mls_gateway/mod.rs` line 1488, the signature is set to an empty string:
```rust
"sig": ""    // Signature not stored separately, but client doesn't verify on receipt
```

However, the Event structure expects:
```rust
#[serde(with = "hex::serde")]
sig: [u8; 64],
```

This mismatch causes the deserialization to fail with "Invalid string length".

### 2. Query Limit Not Enforced
The `max_keypackages_per_query` configuration (default: 1, max: 2) is not being enforced. The Firestore query uses the limit from the REQ message directly, which can be much higher.

## Solution

1. **Fix signature field**: Replace the empty signature with a dummy 128-character hex string (all zeros)
2. **Enforce query limit**: Respect the `max_keypackages_per_query` configuration when querying Firestore

## Implementation Steps

1. **Update the event reconstruction in `process_req`** (line 1488)
   - Change from `"sig": ""` to the full 128-character zero signature:
   ```rust
   "sig": "0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"
   ```

2. **Enforce max_keypackages_per_query limit** (around line 1470)
   - Use the configuration value instead of the raw limit from the REQ message
   - Current code: `Some(limit.min(u32::MAX as usize) as u32)`
   - Should be: `Some(self.config.max_keypackages_per_query.min(2))`

3. **Consider per-author limiting**
   - The current query returns all keypackages for all requested authors
   - Should potentially limit to 1-2 keypackages per individual author
   - This may require modifying the Firestore query logic

## Testing Plan

1. Deploy the fix
2. Upload a keypackage from a client
3. Query for keypackages using REQ: `["REQ","sub1",{"kinds":[443],"authors":["<pubkey>"]}]`
4. Verify the relay returns the keypackage event
5. Test MLS group creation flow end-to-end

## Code Changes

### 1. Fix signature field in `extensions/src/mls_gateway/mod.rs`:

```rust
// Line 1481-1489, update the sig field:
let event_json = serde_json::json!({
    "id": event_id,
    "pubkey": owner_pubkey,
    "created_at": created_at,
    "kind": 443,
    "content": keypackage_content,
    "tags": [],
    "sig": "0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"
});
```

### 2. Fix default limit in `extensions/src/mls_gateway/mod.rs`:

```rust
// Line 1455, change:
.unwrap_or(100);
// To:
.unwrap_or(1);  // Default to 1 when no limit specified
```

### 3. Enforce max_keypackages_per_query limit in `extensions/src/mls_gateway/mod.rs`:

```rust
// Line 1470, change:
Some(limit.min(u32::MAX as usize) as u32),
// To:
Some((limit as u32).min(self.config.max_keypackages_per_query).min(2)),
```

## Risk Assessment

- **Low risk**: These are minimal changes that only affect event reconstruction and query limits
- **No data migration needed**: Existing keypackages in Firestore remain unchanged
- **No protocol changes**: This fix maintains compatibility with the Nostr protocol
- **No security impact**: Signatures aren't verified on keypackage receipt anyway
- **Performance improvement**: Returning fewer keypackages reduces processing overhead

## Why So Many Errors?

The logs show 44 "Failed to reconstruct KeyPackage event" errors because:
1. The query returned 44 keypackages (likely all keypackages for the requested author)
2. Each keypackage reconstruction attempt failed due to the signature field error
3. The `max_keypackages_per_query` limit wasn't being enforced

With these fixes:
- Only 1-2 keypackages will be returned per query
- Those keypackages will successfully reconstruct with valid dummy signatures

## Interaction with Consumption Logic

According to NIP-EE-RELAY protocol: **"once a KeyPackage is exposed (returned in a query), it must be considered consumed"**

The current implementation (lines 1689-1732 in mod.rs):
- **DOES** have consumption logic that spawns an async task after returning keypackages
- Calls `keypackage_consumer::consume_keypackage` for each returned keypackage
- Protects the last resort keypackage (won't delete if it's the last one)

However, this consumption logic is **currently broken** because:
1. Event reconstruction fails before we get to the consumption logic
2. The consumption happens asynchronously after the response is sent

## Summary of All Issues

1. **Signature field error**: Empty string instead of 128-char hex (causes reconstruction failure)
2. **Wrong default limit**: Defaults to 100 when no limit specified (should be 1)
3. **Config not enforced**: Not using `max_keypackages_per_query` configuration
4. **Consumption blocked**: Can't consume keypackages because reconstruction fails first

## Complete Fix Required

All four issues must be fixed for the system to work correctly:
1. Fix signature field format
2. Change default limit from 100 to 1
3. Enforce max_keypackages_per_query config
4. Then consumption logic will work automatically