//! SQL storage backend for MLS Gateway Extension (disabled)
//!
//! This module provides PostgreSQL-based storage for MLS group metadata,
//! key packages, welcome messages, and user epoch tracking.
//! Currently disabled to avoid compilation issues when only using Firestore.

#[cfg(feature = "mls_gateway_sql")]
mod sql_storage {
    use sqlx::PgPool;
    use chrono::{DateTime, Utc};
    use serde::{Deserialize, Serialize};
    use tracing::{info, warn};
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
            storage.run_migrations().await?;
            Ok(storage)
        }

        /// Run database migrations
        async fn run_migrations(&self) -> Result<()> {
            info!("Running SQL database migrations...");
            
            // Create groups table
            sqlx::query(r#"
                CREATE TABLE IF NOT EXISTS mls_groups (
                    group_id TEXT PRIMARY KEY,
                    display_name TEXT,
                    owner_pubkey TEXT NOT NULL,
                    last_epoch BIGINT,
                    admin_pubkeys TEXT[] NOT NULL DEFAULT ARRAY[]::TEXT[],
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
            self.run_migrations().await
        }

        async fn upsert_group(
            &self,
            group_id: &str,
            display_name: Option<&str>,
            creator_pubkey: &str,
            last_epoch: Option<i64>,
        ) -> anyhow::Result<()> {
            // Preserve existing owner_pubkey, created_at, and admin_pubkeys on update.
            // Only update display_name/last_epoch when provided (COALESCE to retain existing when NULL).
            let result = sqlx::query(r#"
                INSERT INTO mls_groups (group_id, display_name, owner_pubkey, last_epoch)
                VALUES ($1, $2, $3, $4)
                ON CONFLICT (group_id) DO UPDATE SET
                    display_name = COALESCE(EXCLUDED.display_name, mls_groups.display_name),
                    last_epoch = COALESCE(EXCLUDED.last_epoch, mls_groups.last_epoch),
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

        async fn group_exists(&self, group_id: &str) -> anyhow::Result<bool> {
            let exists = sqlx::query_scalar::<_, i64>(
                "SELECT 1 FROM mls_groups WHERE group_id = $1 LIMIT 1"
            )
            .bind(group_id)
            .fetch_optional(&self.pool)
            .await?
            .is_some();
            Ok(exists)
        }

        async fn is_owner(&self, group_id: &str, pubkey: &str) -> anyhow::Result<bool> {
            let owner: Option<String> = sqlx::query_scalar(
                "SELECT owner_pubkey FROM mls_groups WHERE group_id = $1"
            )
            .bind(group_id)
            .fetch_optional(&self.pool)
            .await?;
            Ok(owner.map_or(false, |o| o == pubkey))
        }

        async fn is_admin(&self, group_id: &str, pubkey: &str) -> anyhow::Result<bool> {
            let is_admin: Option<bool> = sqlx::query_scalar(
                "SELECT $2 = ANY(admin_pubkeys) FROM mls_groups WHERE group_id = $1"
            )
            .bind(group_id)
            .bind(pubkey)
            .fetch_optional(&self.pool)
            .await?;
            Ok(is_admin.unwrap_or(false))
        }

        async fn add_admins(&self, group_id: &str, admins: &[String]) -> anyhow::Result<()> {
            let mut tx = self.pool.begin().await?;
            let current: Option<Vec<String>> = sqlx::query_scalar(
                "SELECT admin_pubkeys FROM mls_groups WHERE group_id = $1 FOR UPDATE"
            )
            .bind(group_id)
            .fetch_optional(&mut *tx)
            .await?;

            let mut new_list = current.unwrap_or_default();
            for a in admins {
                if !new_list.iter().any(|x| x == a) {
                    new_list.push(a.clone());
                }
            }

            sqlx::query(
                "UPDATE mls_groups SET admin_pubkeys = $2, updated_at = NOW() WHERE group_id = $1"
            )
            .bind(group_id)
            .bind(&new_list)
            .execute(&mut *tx)
            .await?;

            tx.commit().await?;
            Ok(())
        }

        async fn remove_admins(&self, group_id: &str, admins: &[String]) -> anyhow::Result<()> {
            let mut tx = self.pool.begin().await?;
            let current: Option<Vec<String>> = sqlx::query_scalar(
                "SELECT admin_pubkeys FROM mls_groups WHERE group_id = $1 FOR UPDATE"
            )
            .bind(group_id)
            .fetch_optional(&mut *tx)
            .await?;

            let mut new_list = current.unwrap_or_default();
            new_list.retain(|p| !admins.iter().any(|a| a == p));

            sqlx::query(
                "UPDATE mls_groups SET admin_pubkeys = $2, updated_at = NOW() WHERE group_id = $1"
            )
            .bind(group_id)
            .bind(&new_list)
            .execute(&mut *tx)
            .await?;

            tx.commit().await?;
            Ok(())
        }
        
        async fn get_last_roster_sequence(&self, group_id: &str) -> anyhow::Result<Option<u64>> {
            let seq_opt: Option<i64> = sqlx::query_scalar(
                "SELECT sequence FROM mls_roster_policy WHERE group_id = $1 ORDER BY sequence DESC LIMIT 1"
            )
            .bind(group_id)
            .fetch_optional(&self.pool)
            .await?;

            Ok(seq_opt.map(|s| s as u64))
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
            
            let result = sqlx::query(
                r#"
                INSERT INTO mls_roster_policy (id, group_id, sequence, operation, member_pubkeys, admin_pubkey, created_at, updated_at)
                VALUES ($1, $2, $3, $4, $5, $6, $7, NOW())
                "#
            )
            .bind(&id)
            .bind(group_id)
            .bind(sequence as i64)
            .bind(operation)
            .bind(member_pubkeys)
            .bind(admin_pubkey)
            .bind(created_at_ts)
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
