# Simplified KeyPackage Pruning Implementation Plan

## Overview
Remove the per-user limit check when accepting new KeyPackages, and instead enforce the limit during the daily cleanup task that already runs for expired KeyPackages.

## Key Changes

### 1. Remove Limit Check in handle_keypackage()

In `extensions/src/mls_gateway/mod.rs`, modify the `handle_keypackage()` method:

```rust
async fn handle_keypackage(&self, event: &Event) -> anyhow::Result<()> {
    // ... existing validation ...
    
    // REMOVE this entire block:
    // let max_keypackages = self.config.max_keypackages_per_user.unwrap_or(10);
    // let current_count = store.count_user_keypackages(&event_pubkey).await?;
    // if current_count >= max_keypackages {
    //     warn!("User {} has reached keypackage limit ({} >= {})", 
    //           event_pubkey, current_count, max_keypackages);
    //     return Err(anyhow::anyhow!("User keypackage limit exceeded"));
    // }
    
    // Just store the keypackage without limit check
    store.store_keypackage(
        &event.id_str(),
        &event_pubkey,
        // ... other parameters ...
    ).await?;
    
    info!("Stored KeyPackage {} for user {}", event.id_str(), event_pubkey);
    
    // ... rest of method ...
}
```

### 2. Enhance Daily Cleanup Task

The existing cleanup runs in the `init()` method. Modify `cleanup_expired_keypackages()` in both Firestore and SQL implementations:

#### Firestore Implementation (`extensions/src/mls_gateway/firestore.rs`)

```rust
pub async fn cleanup_expired_keypackages(&self) -> Result<u32> {
    let now = Utc::now();
    let max_per_user = self.config.max_keypackages_per_user.unwrap_or(15);
    
    info!("Starting keypackage cleanup - removing expired and enforcing {} per user limit", max_per_user);
    
    let mut total_deleted = 0;
    
    // Step 1: Delete expired keypackages (existing logic)
    total_deleted += self.delete_expired_keypackages().await?;
    
    // Step 2: Enforce per-user limits by pruning oldest
    total_deleted += self.prune_excess_keypackages(max_per_user).await?;
    
    info!("Cleanup complete: deleted {} total keypackages", total_deleted);
    Ok(total_deleted)
}

async fn prune_excess_keypackages(&self, max_per_user: u32) -> Result<u32> {
    // Get all users with their keypackage counts
    let users = self.get_all_keypackage_owners().await?;
    let mut pruned = 0;
    
    for user_pubkey in users {
        // Get all keypackages for this user, sorted by created_at ASC (oldest first)
        let keypackages = self.db
            .fluent()
            .select()
            .from("mls_keypackages")
            .filter(
                Col::field(path!(KeyPackageDoc::owner_pubkey))
                    .eq(&user_pubkey)
            )
            .order_by([(path!(KeyPackageDoc::created_at), Ordering::Asc)])
            .query()
            .await?;
        
        let count = keypackages.len();
        if count > max_per_user as usize {
            let to_delete = count - max_per_user as usize;
            info!("User {} has {} keypackages, pruning {} oldest ones", 
                  user_pubkey, count, to_delete);
            
            // Delete the oldest ones
            for doc in keypackages.iter().take(to_delete) {
                if let Ok(kp) = firestore::FirestoreDb::deserialize_doc_to::<KeyPackageDoc>(&doc) {
                    if let Ok(_) = self.db
                        .fluent()
                        .delete()
                        .from("mls_keypackages")
                        .document_id(&kp.event_id)
                        .execute()
                        .await
                    {
                        pruned += 1;
                        debug!("Pruned old keypackage {} for user {}", kp.event_id, user_pubkey);
                    }
                }
            }
        }
    }
    
    if pruned > 0 {
        info!("Pruned {} keypackages to enforce per-user limits", pruned);
    }
    
    Ok(pruned)
}
```

### 3. Update Configuration Documentation

```toml
# In config/rnostr.toml
[extra.mls_gateway]
# Maximum keypackages per user (enforced during daily cleanup)
# No limit enforced on upload - cleanup will prune oldest
max_keypackages_per_user = 15
```

### 4. Testing

```rust
#[tokio::test]
async fn test_unlimited_upload_with_cleanup() {
    let gateway = create_test_gateway_with_limit(5);
    
    // Upload 10 keypackages (no rejection even though limit is 5)
    for i in 0..10 {
        let kp = create_test_keypackage("user1", &format!("kp{}", i));
        gateway.handle_keypackage(&kp).await.unwrap(); // Should not fail
    }
    
    // Verify all 10 are stored
    assert_eq!(gateway.count_user_keypackages("user1").await, 10);
    
    // Run cleanup
    let deleted = gateway.cleanup_expired_keypackages().await.unwrap();
    
    // Verify only 5 remain (oldest 5 pruned)
    assert_eq!(gateway.count_user_keypackages("user1").await, 5);
    
    // Verify the remaining are the newest ones (kp5 through kp9)
    let remaining = gateway.query_user_keypackages("user1").await;
    for i in 5..10 {
        assert!(remaining.contains(&format!("kp{}", i)));
    }
    for i in 0..5 {
        assert!(!remaining.contains(&format!("kp{}", i)));
    }
}
```

## Benefits

1. **Simpler Implementation**: No need for delayed tasks or scheduling
2. **No Rejection**: Clients never get errors when uploading KeyPackages
3. **Predictable**: Pruning happens during scheduled daily cleanup
4. **Efficient**: Batch processing during off-peak hours
5. **Natural Rotation**: Old KeyPackages automatically removed daily

## Implementation Steps

1. **Remove limit check** from `handle_keypackage()` - 30 minutes
2. **Add pruning logic** to `cleanup_expired_keypackages()` - 2 hours
3. **Test thoroughly** - 1 hour
4. **Deploy to staging** - 30 minutes
5. **Monitor and deploy to production** - 1 day

## Metrics to Add

```rust
describe_counter!("mls_gateway_keypackages_pruned_for_limit", 
                  "Number of keypackages pruned to enforce per-user limit");
```

## Migration

No migration needed:
- Existing KeyPackages remain until next daily cleanup
- New behavior applies immediately
- First cleanup after deployment will enforce limits

## Security Considerations

1. **DoS Protection**: Daily cleanup prevents unlimited growth
2. **Fair Usage**: Each user limited to configured max
3. **Last Resort**: Implementation should still preserve at least 1 KeyPackage per user