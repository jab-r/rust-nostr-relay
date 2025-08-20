//! SQL storage backend for MLS Gateway Extension (disabled)
//!
//! This module provides PostgreSQL-based storage for MLS group metadata,
//! key packages, welcome messages, and user epoch tracking.
//! Currently disabled to avoid compilation issues when only using Firestore.

#[cfg(feature = "mls_gateway_sql")]
mod sql_storage {
    use sqlx::{PgPool, Row, postgres::PgRow};
    use chrono::{DateTime, Utc, Duration};
    use serde::{Deserialize, Serialize};
    use tracing::{info, warn, instrument};
    use uuid::Uuid;
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
        pub created_at: DateTime<Utc>,
        pub updated_at: DateTime<Utc>,
    }

    /// Key package stored in mailbox
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct KeyPackage {
        pub id: String,
        pub recipient_pubkey: String,
        pub sender_pubkey: String,
        pub content_b64: String,
        pub created_at: DateTime<Utc>,
        pub expires_at: DateTime<Utc>,
        pub picked_up_at: Option<DateTime<Utc>>,
    }

    /// Welcome message stored in mailbox
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct WelcomeMessage {
        pub id: String,
        pub recipient_pubkey: String,
        pub sender_pubkey: String,
        pub group_id: String,
        pub welcome_b64: String,
        pub created_at: DateTime<Utc>,
        pub expires_at: DateTime<Utc>,
        pub picked_up_at: Option<DateTime<Utc>>,
    }

    /// SQL storage implementation
    pub struct SqlStorage {
        pool: PgPool,
    }

    impl SqlStorage {
        /// Create new SQL storage instance
        pub async fn new(pool: PgPool) -> Result<Self> {
            let storage = Self { pool };
            storage.migrate().await?;
            Ok(storage)
        }

        /// Run database migrations
        async fn migrate(&self) -> Result<()> {
            info!("Running SQL database migrations...");
            
            // Create groups table
            sqlx::query(r#"
                CREATE TABLE IF NOT EXISTS mls_groups (
                    group_id TEXT PRIMARY KEY,
                    display_name TEXT,
                    owner_pubkey TEXT NOT NULL,
                    last_epoch BIGINT,
                    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
                )
            "#).execute(&self.pool).await?;

            // Create key packages table
            sqlx::query(r#"
                CREATE TABLE IF NOT EXISTS mls_keypackages (
                    id TEXT PRIMARY KEY,
                    recipient_pubkey TEXT NOT NULL,
                    sender_pubkey TEXT NOT NULL,
                    content_b64 TEXT NOT NULL,
                    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                    expires_at TIMESTAMPTZ NOT NULL,
                    picked_up_at TIMESTAMPTZ
                )
            "#).execute(&self.pool).await?;

            // Create welcome messages table
            sqlx::query(r#"
                CREATE TABLE IF NOT EXISTS mls_welcomes (
                    id TEXT PRIMARY KEY,
                    recipient_pubkey TEXT NOT NULL,
                    sender_pubkey TEXT NOT NULL,
                    group_id TEXT NOT NULL,
                    welcome_b64 TEXT NOT NULL,
                    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                    expires_at TIMESTAMPTZ NOT NULL,
                    picked_up_at TIMESTAMPTZ
                )
            "#).execute(&self.pool).await?;

            // Create roster/policy events table
            sqlx::query(r#"
                CREATE TABLE IF NOT EXISTS mls_roster_policy (
                    id TEXT PRIMARY KEY,
                    group_id TEXT NOT NULL,
                    sequence BIGINT NOT NULL,
                    operation TEXT NOT NULL,
                    member_pubkeys TEXT[] NOT NULL,
                    admin_pubkey TEXT NOT NULL,
                    created_at TIMESTAMPTZ NOT NULL,
                    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                    UNIQUE(group_id, sequence)
                )
            "#).execute(&self.pool).await?;

            // Create indexes for performance
            let indexes = [
                "CREATE INDEX IF NOT EXISTS idx_mls_keypackages_recipient ON mls_keypackages(recipient_pubkey)",
                "CREATE INDEX IF NOT EXISTS idx_mls_keypackages_expires ON mls_keypackages(expires_at)",
                "CREATE INDEX IF NOT EXISTS idx_mls_welcomes_recipient ON mls_welcomes(recipient_pubkey)",
                "CREATE INDEX IF NOT EXISTS idx_mls_welcomes_expires ON mls_welcomes(expires_at)",
                "CREATE INDEX IF NOT EXISTS idx_mls_groups_owner ON mls_groups(owner_pubkey)",
                "CREATE INDEX IF NOT EXISTS idx_mls_roster_policy_group ON mls_roster_policy(group_id)",
                "CREATE INDEX IF NOT EXISTS idx_mls_roster_policy_sequence ON mls_roster_policy(group_id, sequence)",
            ];

            for index_sql in indexes.iter() {
                sqlx::query(index_sql).execute(&self.pool).await?;
            }

            info!("SQL database migrations completed successfully");
            Ok(())
        }
    }

    #[async_trait]
    impl MlsStorage for SqlStorage {
        async fn migrate(&self) -> anyhow::Result<()> {
            self.migrate().await
        }

        async fn upsert_group(
            &self,
            group_id: &str,
            display_name: Option<&str>,
            creator_pubkey: &str,
            last_epoch: Option<i64>,
        ) -> anyhow::Result<()> {
            let result = sqlx::query(r#"
                INSERT INTO mls_groups (group_id, display_name, owner_pubkey, last_epoch, created_at, updated_at)
                VALUES ($1, $2, $3, $4, NOW(), NOW())
                ON CONFLICT (group_id) DO UPDATE SET
                    display_name = EXCLUDED.display_name,
                    last_epoch = EXCLUDED.last_epoch,
                    updated_at = NOW()
            "#)
            .bind(group_id)
            .bind(display_name)
            .bind(creator_pubkey)
            .bind(last_epoch)
            .execute(&self.pool)
            .await?;

            info!("Upserted group {} (rows affected: {})", group_id, result.rows_affected());
            Ok(())
        }

        async fn health_check(&self) -> anyhow::Result<()> {
            sqlx::query("SELECT 1").fetch_one(&self.pool).await?;
            Ok(())
        }
        
        async fn get_last_roster_sequence(&self, group_id: &str) -> anyhow::Result<Option<u64>> {
            let result = sqlx::query!(
                "SELECT sequence FROM mls_roster_policy WHERE group_id = $1 ORDER BY sequence DESC LIMIT 1",
                group_id
            )
            .fetch_optional(&self.pool)
            .await?;
            
            Ok(result.map(|row| row.sequence as u64))
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
            let id = format!("{}_{}", group_id, sequence);
            let created_at_ts = chrono::DateTime::from_timestamp(created_at, 0)
                .ok_or_else(|| anyhow::anyhow!("Invalid timestamp"))?;
            
            let result = sqlx::query!(
                r#"
                INSERT INTO mls_roster_policy (id, group_id, sequence, operation, member_pubkeys, admin_pubkey, created_at, updated_at)
                VALUES ($1, $2, $3, $4, $5, $6, $7, NOW())
                "#,
                id,
                group_id,
                sequence as i64,
                operation,
                member_pubkeys,
                admin_pubkey,
                created_at_ts
            )
            .execute(&self.pool)
            .await?;
            
            info!("Stored roster/policy event: group={}, seq={}, op={} (rows affected: {})",
                  group_id, sequence, operation, result.rows_affected());
            Ok(())
        }
    }
}

#[cfg(feature = "mls_gateway_sql")]
pub use sql_storage::*;

#[cfg(not(feature = "mls_gateway_sql"))]
pub struct SqlStorage;