# KeyPackage Delayed Pruning Implementation Plan

## Overview
Implement a delayed pruning mechanism that accepts new KeyPackages even when users are at their limit, then prunes the oldest ones after a 5-minute delay.

## Current Behavior
- Relay rejects new KeyPackages when `max_keypackages_per_user` limit is reached
- Limit is checked in `handle_keypackage()` method
- Returns error: "User keypackage limit exceeded"

## Proposed Behavior
1. Accept new KeyPackages even when over limit
2. Schedule pruning task for 5 minutes later
3. Prune oldest KeyPackages to stay within limit
4. Maintain last-resort protection (never go below 1)

## Implementation Steps

### Step 1: Add Pruning Task Structure
Create a new module `extensions/src/mls_gateway/keypackage_pruner.rs`:

```rust
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{Duration, Instant};
use chrono::{DateTime, Utc};

pub struct PruningTask {
    pub user_pubkey: String,
    pub scheduled_at: Instant,
    pub created_at: DateTime<Utc>,
}

pub struct KeyPackagePruner {
    tasks: Arc<RwLock<HashMap<String, PruningTask>>>,
    store: Arc<dyn MlsStorage>,
    max_per_user: u32,
}

impl KeyPackagePruner {
    pub fn new(store: Arc<dyn MlsStorage>, max_per_user: u32) -> Self {
        Self {
            tasks: Arc::new(RwLock::new(HashMap::new())),
            store,
            max_per_user,
        }
    }
    
    /// Schedule pruning for a user in 5 minutes
    pub async fn schedule_pruning(&self, user_pubkey: String) {
        let mut tasks = self.tasks.write().await;
        let task = PruningTask {
            user_pubkey: user_pubkey.clone(),
            scheduled_at: Instant::now() + Duration::from_secs(300), // 5 minutes
            created_at: Utc::now(),
        };
        tasks.insert(user_pubkey, task);
    }
    
    /// Run the pruning loop
    pub async fn start_pruning_loop(self: Arc<Self>) {
        let mut interval = tokio::time::interval(Duration::from_secs(30)); // Check every 30s
        
        loop {
            interval.tick().await;
            
            if let Err(e) = self.process_scheduled_pruning().await {
                error!("Error processing scheduled pruning: {}", e);
            }
        }
    }
    
    async fn process_scheduled_pruning(&self) -> Result<()> {
        let now = Instant::now();
        let mut tasks = self.tasks.write().await;
        
        // Find tasks that are ready
        let ready_tasks: Vec<String> = tasks
            .iter()
            .filter(|(_, task)| task.scheduled_at <= now)
            .map(|(pubkey, _)| pubkey.clone())
            .collect();
        
        // Remove from schedule
        for pubkey in &ready_tasks {
            tasks.remove(pubkey);
        }
        drop(tasks); // Release lock
        
        // Process each ready task
        for pubkey in ready_tasks {
            if let Err(e) = self.prune_user_keypackages(&pubkey).await {
                error!("Failed to prune keypackages for {}: {}", pubkey, e);
            }
        }
        
        Ok(())
    }
    
    async fn prune_user_keypackages(&self, user_pubkey: &str) -> Result<()> {
        // Get all user's keypackages sorted by created_at (oldest first)
        let keypackages = self.store.query_keypackages(
            Some(&[user_pubkey.to_string()]),
            None,
            None,
            Some("created_at_asc")
        ).await?;
        
        let current_count = keypackages.len();
        if current_count <= self.max_per_user as usize {
            // No pruning needed
            return Ok(());
        }
        
        let to_delete = current_count - self.max_per_user as usize;
        info!("Pruning {} oldest keypackages for user {} (has {}, max {})",
              to_delete, user_pubkey, current_count, self.max_per_user);
        
        // Delete the oldest ones
        for (event_id, _, _, _) in keypackages.iter().take(to_delete) {
            self.store.delete_keypackage_by_id(event_id).await?;
            info!("Pruned old keypackage {} for user {}", event_id, user_pubkey);
        }
        
        counter!("mls_gateway_keypackages_pruned").increment(to_delete as u64);
        
        Ok(())
    }
}
```

### Step 2: Modify handle_keypackage() 
Update `extensions/src/mls_gateway/mod.rs`:

```rust
// Add pruner to MlsGateway struct
pub struct MlsGateway {
    // ... existing fields ...
    pruner: Option<Arc<KeyPackagePruner>>,
}

// In init() method, create and start the pruner:
let pruner = Arc::new(KeyPackagePruner::new(
    store.clone(),
    self.config.max_keypackages_per_user.unwrap_or(15)
));
let pruner_clone = pruner.clone();
tokio::spawn(async move {
    pruner_clone.start_pruning_loop().await;
});
self.pruner = Some(pruner);

// Modify handle_keypackage() to remove the limit check:
async fn handle_keypackage(&self, event: &Event) -> anyhow::Result<()> {
    // ... existing validation ...
    
    // Remove this block:
    // let max_keypackages = self.config.max_keypackages_per_user.unwrap_or(10);
    // let current_count = store.count_user_keypackages(&event_pubkey).await?;
    // if current_count >= max_keypackages {
    //     return Err(anyhow::anyhow!("User keypackage limit exceeded"));
    // }
    
    // Store the keypackage
    store.store_keypackage(
        &event.id_str(),
        // ... parameters ...
    ).await?;
    
    // NEW: Check if we need to schedule pruning
    let current_count = store.count_user_keypackages(&event_pubkey).await?;
    let max_keypackages = self.config.max_keypackages_per_user.unwrap_or(15);
    
    if current_count > max_keypackages {
        if let Some(pruner) = &self.pruner {
            pruner.schedule_pruning(event_pubkey.clone()).await;
            info!("Scheduled pruning for user {} in 5 minutes (has {} keypackages)",
                  event_pubkey, current_count);
        }
    }
    
    // ... rest of method ...
}
```

### Step 3: Update Configuration
Modify default `max_keypackages_per_user` to 15:

```toml
# In config/rnostr.toml
[extra.mls_gateway]
# Maximum keypackages per user (will prune oldest after 5 min if exceeded)
max_keypackages_per_user = 15
```

### Step 4: Add Metrics
Add new metric descriptions:

```rust
describe_counter!("mls_gateway_keypackages_pruned", "Number of keypackages pruned due to limit");
describe_counter!("mls_gateway_pruning_tasks_scheduled", "Number of pruning tasks scheduled");
```

### Step 5: Update Storage Trait
No changes needed - existing `query_keypackages` and `delete_keypackage_by_id` methods are sufficient.

### Step 6: Testing

Create test in `extensions/src/mls_gateway/mod.rs`:

```rust
#[tokio::test]
async fn test_delayed_pruning() {
    // 1. Create gateway with limit of 5
    let gateway = create_test_gateway(5);
    
    // 2. Add 5 keypackages for a user
    for i in 0..5 {
        let kp = create_test_keypackage("user1", &format!("kp{}", i));
        gateway.handle_keypackage(&kp).await.unwrap();
    }
    
    // 3. Add 3 more (now at 8, over limit of 5)
    for i in 5..8 {
        let kp = create_test_keypackage("user1", &format!("kp{}", i));
        gateway.handle_keypackage(&kp).await.unwrap();
    }
    
    // 4. Verify all 8 are stored
    assert_eq!(gateway.count_user_keypackages("user1").await, 8);
    
    // 5. Wait 5+ minutes (or manually trigger pruning in test)
    gateway.pruner.process_scheduled_pruning().await.unwrap();
    
    // 6. Verify only 5 remain (oldest 3 pruned)
    assert_eq!(gateway.count_user_keypackages("user1").await, 5);
    
    // 7. Verify the remaining are the newest ones
    let remaining = gateway.query_user_keypackages("user1").await;
    assert!(remaining.contains("kp3"));
    assert!(remaining.contains("kp7"));
    assert!(!remaining.contains("kp0")); // Oldest pruned
}
```

## Benefits

1. **No Rejection**: Clients never get "limit exceeded" errors
2. **Natural Rotation**: Old KeyPackages automatically rotate out
3. **Simple Client Logic**: Just publish KeyPackages, no pruning needed
4. **Configurable**: Relay operators can set their own limits
5. **Grace Period**: 5-minute delay prevents race conditions

## Migration

This is backward compatible:
- Existing KeyPackages remain
- New behavior only affects future submissions
- No database schema changes needed

## Security Considerations

1. **DoS Protection**: Still enforces eventual limit
2. **Last Resort**: Never prunes below 1 KeyPackage
3. **Fair Rotation**: Always removes oldest first

## Timeline

1. **Day 1**: Implement KeyPackagePruner module
2. **Day 2**: Integrate with handle_keypackage()
3. **Day 3**: Add tests and metrics
4. **Day 4**: Deploy to staging and test
5. **Day 5**: Production deployment

## Configuration Options

```toml
[extra.mls_gateway]
# Maximum keypackages per user after pruning
max_keypackages_per_user = 15

# Delay before pruning (seconds)
pruning_delay = 300  # 5 minutes

# How often to check for scheduled pruning (seconds)
pruning_check_interval = 30