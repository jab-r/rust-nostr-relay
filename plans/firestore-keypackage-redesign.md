# Firestore KeyPackage Query Redesign - SIMPLIFIED

## Problem
Complex Firestore index requirements due to unnecessary query complexity.

## Solution: Remove Unnecessary Complexity

### 1. Remove Creation Time Ordering
- **Current**: Ordering by `created_at` DESC
- **New**: No ordering needed - keypackages are fungible

### 2. Remove Expiration Filtering from Queries
- **Current**: Every query filters `expires_at > now`
- **New**: Delete expired keypackages with a background job
- Queries only see valid keypackages

### 3. Simplified Query
```rust
// Just filter by owner if needed
if let Some(authors) = authors {
    query = query.filter(|f| f.field("owner_pubkey").is_in(authors));
}
// Limit results
query = query.limit(limit);
// That's it!
```

## Implementation

### Step 1: Update the Query (Immediate Fix)
Remove ordering and expiration filtering from `query_keypackages`:
```rust
async fn query_keypackages(
    &self,
    authors: Option<&[String]>,
    since: Option<i64>, // Ignore this parameter
    limit: Option<u32>,
) -> anyhow::Result<Vec<(String, String, String, i64)>> {
    let mut query = self.db
        .fluent()
        .select()
        .from("mls_keypackages");

    // Filter by authors if specified
    if let Some(author_list) = authors {
        if !author_list.is_empty() {
            query = query.filter(|f| f.field("owner_pubkey").is_in(author_list));
        }
    }

    // Apply limit
    let limit_val = limit.unwrap_or(100).min(1000) as u32;
    query = query.limit(limit_val);

    // Execute query - no complex indexes needed!
    let docs = query.query().await?;
    // ... rest of the code
}
```

### Step 2: Clean Up Expired Keypackages
Add a cleanup method that runs periodically:
```rust
async fn cleanup_expired_keypackages(&self) -> anyhow::Result<u32> {
    let now = Utc::now();
    let expired_docs = self.db
        .fluent()
        .select()
        .from("mls_keypackages")
        .filter(|f| f.field("expires_at").less_than_or_equal(now))
        .query()
        .await?;
    
    let mut deleted = 0;
    for doc in expired_docs {
        // Delete each expired keypackage
        self.db.fluent()
            .delete()
            .from("mls_keypackages")
            .document_id(doc.name)
            .execute()
            .await?;
        deleted += 1;
    }
    Ok(deleted)
}
```

### Step 3: Schedule Cleanup
Run cleanup every hour via:
- Cloud Scheduler calling an endpoint
- Or a background task in the application

## Required Indexes
With this approach, we only need a simple index:
```json
{
  "collectionGroup": "mls_keypackages",
  "queryScope": "COLLECTION",
  "fields": [
    {
      "fieldPath": "owner_pubkey",
      "order": "ASCENDING"
    }
  ]
}
```

## Benefits
- No complex composite indexes
- No index errors
- Simpler, faster queries
- Cleaner database (expired entries removed)
- Much easier to understand and maintain