# Fix MLS Keypackages Firestore Index Error

## Problem Summary
The Firestore query for `mls_keypackages` collection is failing with:
```
Error code: FailedPrecondition. status: 'The system is not in a state required for the operation's execution'
The query requires an index. That index is currently building and cannot be used yet.
```

## Root Cause
The `firestore.indexes.json` file is missing index definitions for the `mls_keypackages` collection.

## Required Index
Based on the code in `extensions/src/mls_gateway/firestore.rs`, the `query_keypackages` function needs an index that supports:
1. Filtering by `owner_pubkey` (using IN clause)
2. Ordering by `created_at` (ascending or descending)

## Solution

### 1. Add Missing Index to firestore.indexes.json
Add the following index definition to support the current query pattern:

```json
{
  "collectionGroup": "mls_keypackages",
  "queryScope": "COLLECTION",
  "fields": [
    {
      "fieldPath": "owner_pubkey",
      "order": "ASCENDING"
    },
    {
      "fieldPath": "created_at",
      "order": "ASCENDING"
    }
  ]
},
{
  "collectionGroup": "mls_keypackages",
  "queryScope": "COLLECTION",
  "fields": [
    {
      "fieldPath": "owner_pubkey",
      "order": "ASCENDING"
    },
    {
      "fieldPath": "created_at",
      "order": "DESCENDING"
    }
  ]
}
```

### 2. Additional Indexes to Consider
Based on other potential query patterns, consider adding:

```json
{
  "collectionGroup": "mls_keypackages",
  "queryScope": "COLLECTION",
  "fields": [
    {
      "fieldPath": "event_id",
      "order": "ASCENDING"
    }
  ]
},
{
  "collectionGroup": "mls_keypackages",
  "queryScope": "COLLECTION",
  "fields": [
    {
      "fieldPath": "expires_at",
      "order": "ASCENDING"
    }
  ]
}
```

### 3. Deployment Steps
1. Update the `firestore.indexes.json` file
2. Deploy the index configuration using Firebase CLI:
   ```bash
   firebase deploy --only firestore:indexes
   ```
3. Monitor index build progress in Firebase Console
4. Wait for index to complete building (can take several minutes)
5. Test the queries once index is ready

### 4. Temporary Workaround
While the index is building, you could:
- Implement a temporary in-memory cache for keypackages
- Use a different query pattern that doesn't require the composite index
- Add retry logic with exponential backoff

## Implementation Priority
1. **Immediate**: Add the basic index for owner_pubkey + created_at
2. **Soon**: Add indexes for event_id and expires_at queries
3. **Later**: Review all query patterns and ensure proper indexes exist