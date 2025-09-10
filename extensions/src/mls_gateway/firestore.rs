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
            .execute()
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
            .execute()
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
            .execute()
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
            .execute()
            .await?;
            
        info!("Stored roster/policy event: group={}, seq={}, op={}", group_id, sequence, operation);
        Ok(())
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
