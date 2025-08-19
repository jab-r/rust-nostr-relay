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
    #[serde(with = "chrono::serde::ts_seconds")]
    pub created_at: DateTime<Utc>,
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
        
        let group = GroupInfo {
            group_id: group_id.to_string(),
            display_name: display_name.map(|s| s.to_string()),
            owner_pubkey: owner_pubkey.to_string(),
            last_epoch: Some(last_epoch),
            created_at: now,
            updated_at: now,
        };

        // Insert or update the group
        self.db
            .fluent()
            .update()
            .fields(paths!(GroupInfo::{group_id, display_name, owner_pubkey, last_epoch, created_at, updated_at}))
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
}