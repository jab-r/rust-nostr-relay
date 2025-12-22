//! Firestore storage implementation for MLS Gateway Extension
//! 
//! Provides Firestore-based storage for:
//! - Group registry metadata
//! - Key package mailbox  
//! - Welcome message mailbox
//! - TTL-based cleanup

use firestore::*;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{info, instrument};
use anyhow::Result;
use async_trait::async_trait;
use crate::mls_gateway::MlsStorage;

/// Group metadata stored in the registry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupInfo {
    pub group_id: String,
    pub display_name: Option<String>,
    pub owner_pubkey: String,
    pub last_epoch: Option<i64>,
    #[serde(default)]
    pub admin_pubkeys: Vec<String>,
    #[serde(default)]
    pub service_member: bool,
    #[serde(with = "chrono::serde::ts_seconds")]
    pub created_at: DateTime<Utc>,
    #[serde(with = "chrono::serde::ts_seconds")]
    pub updated_at: DateTime<Utc>,
}

///// Helper struct for partial admin updates
#[derive(Debug, Clone, Serialize, Deserialize)]
struct AdminsPatch {
    #[serde(default)]
    pub admin_pubkeys: Vec<String>,
    #[serde(with = "chrono::serde::ts_seconds")]
    pub updated_at: DateTime<Utc>,
}

/// KeyPackage Relays list document (kind 10051)
#[derive(Debug, Clone, Serialize, Deserialize)]
struct KeypackageRelays {
    pub owner_pubkey: String,
    #[serde(default)]
    pub relays: Vec<String>,
    #[serde(with = "chrono::serde::ts_seconds")]
    pub updated_at: DateTime<Utc>,
}

/// KeyPackage document structure for Firestore
#[derive(Debug, Clone, Serialize, Deserialize)]
struct KeyPackageDoc {
    pub event_id: String,
    pub owner_pubkey: String,
    pub content: String,
    pub ciphersuite: String,
    pub extensions: Vec<String>,
    pub relays: Vec<String>,
    #[serde(with = "chrono::serde::ts_seconds")]
    pub created_at: DateTime<Utc>,
    #[serde(with = "chrono::serde::ts_seconds")]
    pub expires_at: DateTime<Utc>,
}

/// Firestore storage implementation
#[derive(Debug)]
pub struct FirestoreStorage {
    db: FirestoreDb,
}

impl FirestoreStorage {
    /// Create a new Firestore store
    pub async fn new(project_id: &str) -> Result<Self> {
        info!("Connecting to Firestore project: {}", project_id);
        
        let db = FirestoreDb::new(project_id).await?;
        
        info!("Firestore connection established successfully");
        
        Ok(Self { db })
    }

    /// Initialize collections (Firestore collections are created on first write)
    #[instrument(skip(self))]
    pub async fn migrate(&self) -> Result<()> {
        info!("Initializing Firestore collections");
        
        // Firestore collections are created automatically on first write
        // No migration needed, but we can create index files if needed
        
        info!("Firestore collections initialized successfully");
        Ok(())
    }

    /// Fetch a group document by ID
    pub async fn fetch_group(&self, group_id: &str) -> Result<Option<GroupInfo>> {
        let docs = self.db
            .fluent()
            .select()
            .from("mls_groups")
            .filter(|f| f.field("group_id").eq(group_id))
            .limit(1)
            .query()
            .await?;

        let mut groups: Vec<GroupInfo> = docs
            .into_iter()
            .filter_map(|doc| {
                firestore::FirestoreDb::deserialize_doc_to::<GroupInfo>(&doc).ok()
            })
            .collect();

        Ok(groups.pop())
    }

    /// Upsert group information in the registry
    #[instrument(skip(self))]
    pub async fn upsert_group(
        &self,
        group_id: &str,
        display_name: Option<&str>,
        owner_pubkey: &str,
        last_epoch: i64,
    ) -> Result<()> {
        let now = Utc::now();

        // Preserve existing owner and created_at if the group already exists
        let existing = self.fetch_group(group_id).await?;
        let (owner_val, created_at_val, existing_admins, existing_display_name, existing_last_epoch, existing_service_member) = if let Some(g) = existing {
            (g.owner_pubkey, g.created_at, g.admin_pubkeys, g.display_name, g.last_epoch, g.service_member)
        } else {
            (owner_pubkey.to_string(), now, Vec::new(), None, None, false)
        };

        let group = GroupInfo {
            group_id: group_id.to_string(),
            display_name: display_name.map(|s| s.to_string()).or(existing_display_name),
            owner_pubkey: owner_val,
            last_epoch: Some(last_epoch).or(existing_last_epoch),
            admin_pubkeys: existing_admins,
            service_member: existing_service_member,
            created_at: created_at_val,
            updated_at: now,
        };

        // Insert or update the group
        self.db
            .fluent()
            .update()
            .fields(paths!(GroupInfo::{group_id, display_name, owner_pubkey, last_epoch, admin_pubkeys, service_member, created_at, updated_at}))
            .in_col("mls_groups")
            .document_id(group_id)
            .object(&group)
            .execute::<()>()
            .await?;

        info!("Updated group registry: {}", group_id);
        Ok(())
    }

    /// Get database health status
    pub async fn health_check(&self) -> Result<()> {
        // Simple health check - try to query the database
        let _result: Vec<GroupInfo> = self.db
            .fluent()
            .select()
            .from("mls_groups")
            .limit(1)
            .obj()
            .query()
            .await?;

        Ok(())
    }
    
    /// Returns true if the group is flagged to contain a service member
    pub async fn has_service_member(&self, group_id: &str) -> Result<bool> {
        Ok(self.fetch_group(group_id).await?.map(|g| g.service_member).unwrap_or(false))
    }
    
    /// Clean up expired keypackages - should be run daily
    pub async fn cleanup_expired_keypackages(&self) -> Result<u32> {
        let now = Utc::now();
        info!("Starting cleanup of expired keypackages");
        
        // Query for expired keypackages
        let expired_docs = self.db
            .fluent()
            .select()
            .from("mls_keypackages")
            .filter(|f| f.field("expires_at").less_than_or_equal(now))
            .query()
            .await?;
        
        let mut deleted = 0;
        for doc in expired_docs {
            if let Ok(kp) = firestore::FirestoreDb::deserialize_doc_to::<KeyPackageDoc>(&doc) {
                // Delete the expired keypackage
                if let Ok(_) = self.db
                    .fluent()
                    .delete()
                    .from("mls_keypackages")
                    .document_id(&kp.event_id)
                    .execute()
                    .await
                {
                    deleted += 1;
                    info!("Deleted expired keypackage {} for owner {}", kp.event_id, kp.owner_pubkey);
                }
            }
        }
        
        info!("Cleanup complete: deleted {} expired keypackages", deleted);
        Ok(deleted)
    }
}

#[async_trait]
impl MlsStorage for FirestoreStorage {
    async fn migrate(&self) -> anyhow::Result<()> {
        self.migrate().await
    }
    
    async fn upsert_group(
        &self,
        group_id: &str,
        display_name: Option<&str>,
        creator_pubkey: &str,
        epoch: Option<i64>,
    ) -> anyhow::Result<()> {
        self.upsert_group(group_id, display_name, creator_pubkey, epoch.unwrap_or(0)).await
    }
    
    async fn health_check(&self) -> anyhow::Result<()> {
        self.health_check().await
    }

    async fn group_exists(&self, group_id: &str) -> anyhow::Result<bool> {
        let docs = self.db
            .fluent()
            .select()
            .from("mls_groups")
            .filter(|f| f.field("group_id").eq(group_id))
            .limit(1)
            .query()
            .await?;
        Ok(!docs.is_empty())
    }

    async fn is_owner(&self, group_id: &str, pubkey: &str) -> anyhow::Result<bool> {
        let group = self.fetch_group(group_id).await?;
        Ok(group.map_or(false, |g| g.owner_pubkey == pubkey))
    }

    async fn is_admin(&self, group_id: &str, pubkey: &str) -> anyhow::Result<bool> {
        let group = self.fetch_group(group_id).await?;
        Ok(group.map_or(false, |g| g.admin_pubkeys.iter().any(|p| p == pubkey)))
    }

    async fn add_admins(&self, group_id: &str, admins: &[String]) -> anyhow::Result<()> {
        let now = Utc::now();
        let mut current = self.fetch_group(group_id).await?.map(|g| g.admin_pubkeys).unwrap_or_default();
        for a in admins {
            if !current.iter().any(|x| x == a) {
                current.push(a.clone());
            }
        }
        let patch = AdminsPatch { admin_pubkeys: current, updated_at: now };
        self.db
            .fluent()
            .update()
            .fields(paths!(AdminsPatch::{admin_pubkeys, updated_at}))
            .in_col("mls_groups")
            .document_id(group_id)
            .object(&patch)
            .execute::<()>()
            .await?;
        Ok(())
    }

    async fn remove_admins(&self, group_id: &str, admins: &[String]) -> anyhow::Result<()> {
        let now = Utc::now();
        let mut current = self.fetch_group(group_id).await?.map(|g| g.admin_pubkeys).unwrap_or_default();
        current.retain(|p| !admins.iter().any(|a| a == p));
        let patch = AdminsPatch { admin_pubkeys: current, updated_at: now };
        self.db
            .fluent()
            .update()
            .fields(paths!(AdminsPatch::{admin_pubkeys, updated_at}))
            .in_col("mls_groups")
            .document_id(group_id)
            .object(&patch)
            .execute::<()>()
            .await?;
        Ok(())
    }
    
    async fn get_last_roster_sequence(&self, group_id: &str) -> anyhow::Result<Option<u64>> {
        use firestore::*;
        
        let collection_name = "roster_policy";
        
        // Query for the latest sequence for this group
        let query = self.db
            .fluent()
            .select()
            .from(collection_name)
            .filter(|f| f.field("group_id").eq(group_id))
            .order_by([
                FirestoreQueryOrder::new("sequence".to_string(), FirestoreQueryDirection::Descending)
            ])
            .limit(1);

        let docs = query.query().await?;
        let roster_docs: Vec<RosterPolicyDocument> = docs
            .into_iter()
            .filter_map(|doc| {
                // Try to deserialize each document
                firestore::FirestoreDb::deserialize_doc_to::<RosterPolicyDocument>(&doc).ok()
            })
            .collect();
        
        Ok(roster_docs.first().map(|doc| doc.sequence))
    }
    
    async fn store_roster_policy(
        &self,
        group_id: &str,
        sequence: u64,
        operation: &str,
        member_pubkeys: &[String],
        admin_pubkey: &str,
        created_at: i64,
    ) -> anyhow::Result<()> {
        let collection = "roster_policy";
        
        // Check if sequence already exists for idempotency
        if let Ok(Some(last_seq)) = self.get_last_roster_sequence(group_id).await {
            if sequence <= last_seq {
                return Err(anyhow::anyhow!(
                    "Invalid sequence: {} <= last sequence {}",
                    sequence, last_seq
                ));
            }
        }
        
        let doc = RosterPolicyDocument {
            group_id: group_id.to_string(),
            sequence,
            operation: operation.to_string(),
            member_pubkeys: member_pubkeys.to_vec(),
            admin_pubkey: admin_pubkey.to_string(),
            created_at,
            updated_at: chrono::Utc::now().timestamp(),
        };
        
        let doc_id = format!("{}_{}", group_id, sequence);
        
        self.db
            .fluent()
            .insert()
            .into(collection)
            .document_id(&doc_id)
            .object(&doc)
            .execute::<()>()
            .await?;
            
        info!("Stored roster/policy event: group={}, seq={}, op={}", group_id, sequence, operation);
        Ok(())
    }

    async fn upsert_keypackage_relays(&self, owner_pubkey: &str, relays: &[String]) -> anyhow::Result<()> {
        let rec = KeypackageRelays {
            owner_pubkey: owner_pubkey.to_string(),
            relays: relays.to_vec(),
            updated_at: Utc::now(),
        };

        self.db
            .fluent()
            .update()
            .fields(paths!(KeypackageRelays::{owner_pubkey, relays, updated_at}))
            .in_col("keypackage_relays")
            .document_id(owner_pubkey)
            .object(&rec)
            .execute::<()>()
            .await?;

        info!("Upserted KeyPackage relays list for owner {}", owner_pubkey);
        Ok(())
    }

    async fn get_keypackage_relays(&self, owner_pubkey: &str) -> anyhow::Result<Vec<String>> {
        let docs = self.db
            .fluent()
            .select()
            .from("keypackage_relays")
            .filter(|f| f.field("owner_pubkey").eq(owner_pubkey))
            .limit(1)
            .query()
            .await?;

        let mut items: Vec<KeypackageRelays> = docs
            .into_iter()
            .filter_map(|doc| firestore::FirestoreDb::deserialize_doc_to::<KeypackageRelays>(&doc).ok())
            .collect();

        Ok(items.pop().map(|k| k.relays).unwrap_or_default())
    }

    async fn store_keypackage(
        &self,
        event_id: &str,
        owner_pubkey: &str,
        content: &str,
        ciphersuite: &str,
        extensions: &[String],
        relays: &[String],
        _has_last_resort: bool,
        created_at: i64,
        expires_at: i64,
    ) -> anyhow::Result<()> {
        // Note: has_last_resort parameter is now ignored since we use
        // "last remaining" approach instead of explicit last resort extension
        let doc = KeyPackageDoc {
            event_id: event_id.to_string(),
            owner_pubkey: owner_pubkey.to_string(),
            content: content.to_string(),
            ciphersuite: ciphersuite.to_string(),
            extensions: extensions.to_vec(),
            relays: relays.to_vec(),
            created_at: DateTime::from_timestamp(created_at, 0).unwrap_or_else(Utc::now),
            expires_at: DateTime::from_timestamp(expires_at, 0).unwrap_or_else(Utc::now),
        };

        self.db
            .fluent()
            .insert()
            .into("mls_keypackages")
            .document_id(event_id)
            .object(&doc)
            .execute::<()>()
            .await?;

        info!("Stored keypackage {} for owner {}", event_id, owner_pubkey);
        Ok(())
    }

    async fn query_keypackages(
        &self,
        authors: Option<&[String]>,
        _since: Option<i64>, // Ignored - not needed for keypackage queries
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

        // Simple query - no ordering, no expiration filtering
        // Expired keypackages are cleaned up by a separate daily job
        let docs = query.query().await?;
        let keypackages: Vec<(String, String, String, i64)> = docs
            .into_iter()
            .filter_map(|doc| {
                firestore::FirestoreDb::deserialize_doc_to::<KeyPackageDoc>(&doc).ok()
                    .map(|kp| (kp.event_id, kp.owner_pubkey, kp.content, kp.created_at.timestamp()))
            })
            .collect();

        Ok(keypackages)
    }

    async fn delete_consumed_keypackage(&self, event_id: &str) -> anyhow::Result<bool> {
        // First get the keypackage to find its owner
        let docs = self.db
            .fluent()
            .select()
            .from("mls_keypackages")
            .filter(|f| f.field("event_id").eq(event_id))
            .limit(1)
            .query()
            .await?;

        if let Some(doc) = docs.into_iter().next() {
            if let Ok(kp) = firestore::FirestoreDb::deserialize_doc_to::<KeyPackageDoc>(&doc) {
                // Count how many valid keypackages this user has
                let count = self.count_user_keypackages(&kp.owner_pubkey).await?;
                
                if count <= 1 {
                    // This is the last keypackage for the user - preserve it
                    info!("Preserving last remaining keypackage {} for user {}", event_id, kp.owner_pubkey);
                    return Ok(false);
                }
                
                // Safe to delete - user has other keypackages
                self.db
                    .fluent()
                    .delete()
                    .from("mls_keypackages")
                    .document_id(event_id)
                    .execute()
                    .await?;

                info!("Deleted consumed keypackage {} for user {} (remaining: {})",
                      event_id, kp.owner_pubkey, count - 1);
                return Ok(true);
            }
        }

        Ok(false)
    }

    async fn count_user_keypackages(&self, owner_pubkey: &str) -> anyhow::Result<u32> {
        let now = Utc::now();
        let docs = self.db
            .fluent()
            .select()
            .from("mls_keypackages")
            .filter(|f| f.field("owner_pubkey").eq(owner_pubkey))
            .filter(|f| f.field("expires_at").greater_than(now))
            .query()
            .await?;

        Ok(docs.len() as u32)
    }

    async fn cleanup_expired_keypackages(&self) -> anyhow::Result<u32> {
        let now = Utc::now();
        
        // Query for expired keypackages
        let expired_docs = self.db
            .fluent()
            .select()
            .from("mls_keypackages")
            .filter(|f| f.field("expires_at").less_than_or_equal(now))
            .query()
            .await?;

        let mut deleted_count = 0u32;

        // Group expired keypackages by owner
        let mut expired_by_owner: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
        
        for doc in expired_docs {
            if let Ok(kp) = firestore::FirestoreDb::deserialize_doc_to::<KeyPackageDoc>(&doc) {
                expired_by_owner.entry(kp.owner_pubkey.clone())
                    .or_insert_with(Vec::new)
                    .push(kp.event_id);
            }
        }

        // For each owner, delete expired keypackages but preserve at least one
        for (owner_pubkey, expired_ids) in expired_by_owner {
            // Count total valid keypackages for this user
            let total_count = self.count_user_keypackages(&owner_pubkey).await?;
            
            // Calculate how many we can safely delete while keeping at least one
            let deletable_count = if total_count > expired_ids.len() as u32 {
                // User has non-expired keypackages, can delete all expired
                expired_ids.len()
            } else {
                // All keypackages are expired, keep at least one
                expired_ids.len().saturating_sub(1)
            };
            
            // Delete the deletable expired keypackages
            for (i, event_id) in expired_ids.iter().enumerate() {
                if i < deletable_count {
                    if let Ok(_) = self.db
                        .fluent()
                        .delete()
                        .from("mls_keypackages")
                        .document_id(event_id)
                        .execute()
                        .await
                    {
                        deleted_count += 1;
                    }
                } else {
                    info!("Preserving expired keypackage {} as last remaining for user {}",
                          event_id, owner_pubkey);
                }
            }
        }

        if deleted_count > 0 {
            info!("Cleaned up {} expired keypackages", deleted_count);
        }

        Ok(deleted_count)
    }
}

/// Roster/Policy document structure for Firestore
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RosterPolicyDocument {
    pub group_id: String,
    pub sequence: u64,
    pub operation: String,
    pub member_pubkeys: Vec<String>,
    pub admin_pubkey: String,
    pub created_at: i64,
    pub updated_at: i64,
}
