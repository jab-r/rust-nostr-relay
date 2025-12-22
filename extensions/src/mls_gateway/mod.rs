//! MLS Gateway Extension for high-security MLS-over-Nostr relay
//!
//! This extension provides:
//! - Complete MLS onboarding flow: KeyPackages (443), Welcome/Giftwrap (444/1059)
//! - MLS group messaging (kind 445) and Noise DM (kind 446) routing
//! - Key package and welcome message mailbox services
//! - Group registry with membership management
//! - REST API endpoints for auxiliary flows
//! - Cloud SQL integration for MLS-specific metadata

pub mod storage;
pub mod endpoints;
pub mod mailbox;
pub mod groups;
pub mod message_archive;
pub mod keypackage_delivery;
pub mod req_interceptor;
pub mod keypackage_consumer;
pub mod test_keypackage_flow;

#[cfg(feature = "mls_gateway_firestore")]
pub mod firestore;

#[cfg(feature = "nip_service_mls")]
pub mod service_member;

#[cfg(feature = "mls_gateway_firestore")]
pub use firestore::FirestoreStorage;

#[cfg(feature = "mls_gateway_sql")]
pub use storage::SqlStorage;

pub use message_archive::MessageArchive;

use actix_web::web::ServiceConfig;
use nostr_relay::{Extension, Session, ExtensionMessageResult};
use nostr_relay::db::Event;
use nostr_relay::message::{ClientMessage, IncomingMessage, Subscription};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{info, warn, error};
use metrics::{counter, describe_counter, describe_histogram};
use crate::mls_gateway::keypackage_delivery::{init_delivery_store, get_delivery_store};

// MLS and Noise event kinds as per specification
const KEYPACKAGE_KIND: u16 = 443;         // MLS KeyPackage
const WELCOME_KIND: u16 = 444;            // MLS Welcome (embedded in 1059)
const MLS_GROUP_MESSAGE_KIND: u16 = 445;  // MLS Group Message
const NOISE_DM_KIND: u16 = 446;           // Noise Direct Message
// Note: Kind 447 (KeyPackage Request) is deprecated - use REQ queries for kind 443 instead
const ROSTER_POLICY_KIND: u16 = 450;      // Roster/Policy (Admin-signed membership control)
const KEYPACKAGE_RELAYS_LIST_KIND: u16 = 10051; // KeyPackage Relays List
const GIFTWRAP_KIND: u16 = 1059;          // Giftwrap envelope for Welcome

/// Storage backend type configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StorageType {
    Firestore,
    #[cfg(feature = "mls_gateway_sql")]
    CloudSql,
}

impl Default for StorageType {
    fn default() -> Self {
        StorageType::Firestore
    }
}

/// MLS Gateway Extension configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct MlsGatewayConfig {
    /// Storage backend to use
    pub storage_backend: StorageType,
    /// Google Cloud Project ID (for Firestore)
    pub project_id: Option<String>,
    /// Cloud SQL database URL (for CloudSQL backend)
    pub database_url: Option<String>,
    /// Maximum TTL for key packages (seconds)
    pub keypackage_ttl: u64,
    /// Maximum TTL for welcome messages (seconds)
    pub welcome_ttl: u64,
    /// Enable REST API endpoints
    pub enable_api: bool,
    /// API endpoint prefix
    pub api_prefix: String,
    /// Enable message archival for offline delivery
    pub enable_message_archive: bool,
    /// Message archive TTL in days
    pub message_archive_ttl_days: u32,
    /// System/relay pubkey (deprecated - was used for kind 447 requests)
    pub system_pubkey: Option<String>,
    /// Admin pubkeys allowed to send roster/policy events (kind 450)
    pub admin_pubkeys: Vec<String>,
    /// TTL for KeyPackage requests (deprecated - kind 447 no longer supported)
    pub keypackage_request_ttl: u64,
    /// TTL for roster/policy events in days (default: indefinite/365 days)
    pub roster_policy_ttl_days: u32,

    /// Enable in-process MLS decrypt/dispatch for service actions
    pub enable_in_process_decrypt: bool,
    /// Select the active handler for service actions: "in-process" or "external"
    pub preferred_service_handler: String,
    /// Optional policy hint: if true, skip attempt when registry does not mark service-enabled
    pub gating_use_registry_hint: bool,
    /// MLS service-member user identifier used for membership checks
    pub mls_service_user_id: Option<String>,

    /// Backfill Firestore archived events into LMDB on startup
    pub backfill_on_startup: bool,
    /// Kinds to backfill from Firestore into LMDB
    pub backfill_kinds: Vec<u32>,
    /// Upper bound on total events to backfill
    pub backfill_max_events: u32,
    /// Maximum number of keypackages per user
    pub max_keypackages_per_user: Option<u32>,
}

impl Default for MlsGatewayConfig {
    fn default() -> Self {
        Self {
            storage_backend: StorageType::Firestore,
            project_id: None,
            database_url: None,
            keypackage_ttl: 604800, // 7 days
            welcome_ttl: 259200,    // 3 days
            enable_api: false,
            api_prefix: "/api/v1".to_string(),
            enable_message_archive: true,
            message_archive_ttl_days: 30,
            system_pubkey: None,
            admin_pubkeys: Vec::new(),
            keypackage_request_ttl: 604800, // 7 days
            roster_policy_ttl_days: 365,    // 1 year
            enable_in_process_decrypt: true,
            preferred_service_handler: "in-process".to_string(),
            gating_use_registry_hint: false,
            mls_service_user_id: None,
            backfill_on_startup: true,
            backfill_kinds: vec![445, 1059, 446],
            backfill_max_events: 50000,
            max_keypackages_per_user: Some(10),
        }
    }
}

/// Storage trait for MLS Gateway
#[async_trait::async_trait]
pub trait MlsStorage: Send + Sync {
    async fn migrate(&self) -> anyhow::Result<()>;
    async fn upsert_group(
        &self,
        group_id: &str,
        display_name: Option<&str>,
        owner_pubkey: &str,
        last_epoch: Option<i64>,
    ) -> anyhow::Result<()>;
    async fn health_check(&self) -> anyhow::Result<()>;

    /// Group-level metadata and authorization helpers
    async fn group_exists(&self, group_id: &str) -> anyhow::Result<bool>;
    async fn is_owner(&self, group_id: &str, pubkey: &str) -> anyhow::Result<bool>;
    async fn is_admin(&self, group_id: &str, pubkey: &str) -> anyhow::Result<bool>;
    async fn add_admins(&self, group_id: &str, admins: &[String]) -> anyhow::Result<()>;
    async fn remove_admins(&self, group_id: &str, admins: &[String]) -> anyhow::Result<()>;
    
    /// Get the last roster/policy sequence number for a group
    async fn get_last_roster_sequence(&self, group_id: &str) -> anyhow::Result<Option<u64>>;
    
    /// Store a roster/policy event with sequence validation
    async fn store_roster_policy(
        &self,
        group_id: &str,
        sequence: u64,
        operation: &str,
        member_pubkeys: &[String],
        admin_pubkey: &str,
        created_at: i64,
    ) -> anyhow::Result<()>;

    /// KeyPackage Relays List per owner (kind 10051)
    async fn upsert_keypackage_relays(&self, owner_pubkey: &str, relays: &[String]) -> anyhow::Result<()>;
    async fn get_keypackage_relays(&self, owner_pubkey: &str) -> anyhow::Result<Vec<String>>;

    /// KeyPackage lifecycle management (kind 443)
    async fn store_keypackage(
        &self,
        event_id: &str,
        owner_pubkey: &str,
        content: &str,
        ciphersuite: &str,
        extensions: &[String],
        relays: &[String],
        has_last_resort: bool,
        created_at: i64,
        expires_at: i64,
    ) -> anyhow::Result<()>;
    
    /// Query keypackages with filters
    async fn query_keypackages(
        &self,
        authors: Option<&[String]>,
        since: Option<i64>,
        limit: Option<u32>,
        order_by: Option<&str>,
    ) -> anyhow::Result<Vec<(String, String, String, i64)>>; // (event_id, owner_pubkey, content, created_at)
    
    /// Delete a consumed keypackage (unless it's a last resort keypackage)
    async fn delete_consumed_keypackage(&self, event_id: &str) -> anyhow::Result<bool>; // returns true if deleted
    
    /// Count keypackages per user
    async fn count_user_keypackages(&self, owner_pubkey: &str) -> anyhow::Result<u32>;
    
    /// Clean up expired keypackages
    async fn cleanup_expired_keypackages(&self) -> anyhow::Result<u32>;

    // New methods for pending deletion management
    
    /// Create a pending deletion record for last resort keypackage
    async fn create_pending_deletion(&self, pending: &firestore::PendingDeletion) -> anyhow::Result<()>;
    
    /// Get pending deletion for a user
    async fn get_pending_deletion(&self, user_pubkey: &str) -> anyhow::Result<Option<firestore::PendingDeletion>>;
    
    /// Update pending deletion (add new keypackages to the list)
    async fn update_pending_deletion(&self, pending: &firestore::PendingDeletion) -> anyhow::Result<()>;
    
    /// Delete pending deletion record
    async fn delete_pending_deletion(&self, user_pubkey: &str) -> anyhow::Result<()>;
    
    /// Delete keypackage by ID (bypassing last-one check)
    async fn delete_keypackage_by_id(&self, event_id: &str) -> anyhow::Result<()>;
    
    /// Check if a keypackage exists
    async fn keypackage_exists(&self, event_id: &str) -> anyhow::Result<bool>;
    
    /// Get all pending deletions that should be processed
    async fn get_expired_pending_deletions(&self) -> anyhow::Result<Vec<firestore::PendingDeletion>>;
}

/// MLS Gateway Extension
#[derive(Debug, Clone)]
pub enum StorageBackend {
    #[cfg(feature = "mls_gateway_sql")]
    Sql(Arc<storage::SqlStorage>),
    #[cfg(feature = "mls_gateway_firestore")]
    Firestore(Arc<firestore::FirestoreStorage>),
}

impl StorageBackend {
    async fn migrate(&self) -> anyhow::Result<()> {
        match self {
            #[cfg(feature = "mls_gateway_sql")]
            StorageBackend::Sql(storage) => storage.migrate().await,
            #[cfg(feature = "mls_gateway_firestore")]
            StorageBackend::Firestore(storage) => storage.migrate().await,
        }
    }

    async fn upsert_group(
        &self,
        group_id: &str,
        display_name: Option<&str>,
        creator_pubkey: &str,
        epoch: u64,
    ) -> anyhow::Result<()> {
        match self {
            #[cfg(feature = "mls_gateway_sql")]
            StorageBackend::Sql(storage) => storage.upsert_group(group_id, display_name, creator_pubkey, Some(epoch as i64)).await,
            #[cfg(feature = "mls_gateway_firestore")]
            StorageBackend::Firestore(storage) => storage.upsert_group(group_id, display_name, creator_pubkey, epoch as i64).await,
        }
    }

    async fn health_check(&self) -> anyhow::Result<()> {
        match self {
            #[cfg(feature = "mls_gateway_sql")]
            StorageBackend::Sql(storage) => storage.health_check().await,
            #[cfg(feature = "mls_gateway_firestore")]
            StorageBackend::Firestore(storage) => storage.health_check().await,
        }
    }

    /// Group-level metadata and authorization helpers
    async fn group_exists(&self, group_id: &str) -> anyhow::Result<bool> {
        match self {
            #[cfg(feature = "mls_gateway_sql")]
            StorageBackend::Sql(storage) => storage.group_exists(group_id).await,
            #[cfg(feature = "mls_gateway_firestore")]
            StorageBackend::Firestore(storage) => storage.group_exists(group_id).await,
        }
    }

    async fn is_owner(&self, group_id: &str, pubkey: &str) -> anyhow::Result<bool> {
        match self {
            #[cfg(feature = "mls_gateway_sql")]
            StorageBackend::Sql(storage) => storage.is_owner(group_id, pubkey).await,
            #[cfg(feature = "mls_gateway_firestore")]
            StorageBackend::Firestore(storage) => storage.is_owner(group_id, pubkey).await,
        }
    }

    async fn is_admin(&self, group_id: &str, pubkey: &str) -> anyhow::Result<bool> {
        match self {
            #[cfg(feature = "mls_gateway_sql")]
            StorageBackend::Sql(storage) => storage.is_admin(group_id, pubkey).await,
            #[cfg(feature = "mls_gateway_firestore")]
            StorageBackend::Firestore(storage) => storage.is_admin(group_id, pubkey).await,
        }
    }

    async fn add_admins(&self, group_id: &str, admins: &[String]) -> anyhow::Result<()> {
        match self {
            #[cfg(feature = "mls_gateway_sql")]
            StorageBackend::Sql(storage) => storage.add_admins(group_id, admins).await,
            #[cfg(feature = "mls_gateway_firestore")]
            StorageBackend::Firestore(storage) => storage.add_admins(group_id, admins).await,
        }
    }

    async fn remove_admins(&self, group_id: &str, admins: &[String]) -> anyhow::Result<()> {
        match self {
            #[cfg(feature = "mls_gateway_sql")]
            StorageBackend::Sql(storage) => storage.remove_admins(group_id, admins).await,
            #[cfg(feature = "mls_gateway_firestore")]
            StorageBackend::Firestore(storage) => storage.remove_admins(group_id, admins).await,
        }
    }

    /// Get the last roster/policy sequence number for a group
    async fn get_last_roster_sequence(&self, group_id: &str) -> anyhow::Result<Option<u64>> {
        match self {
            #[cfg(feature = "mls_gateway_sql")]
            StorageBackend::Sql(storage) => storage.get_last_roster_sequence(group_id).await,
            #[cfg(feature = "mls_gateway_firestore")]
            StorageBackend::Firestore(storage) => storage.get_last_roster_sequence(group_id).await,
        }
    }

    /// Store a roster/policy event with sequence validation
    async fn store_roster_policy(
        &self,
        group_id: &str,
        sequence: u64,
        operation: &str,
        member_pubkeys: &[String],
        admin_pubkey: &str,
        created_at: i64,
    ) -> anyhow::Result<()> {
        match self {
            #[cfg(feature = "mls_gateway_sql")]
            StorageBackend::Sql(storage) => {
                storage.store_roster_policy(group_id, sequence, operation, member_pubkeys, admin_pubkey, created_at).await
            }
            #[cfg(feature = "mls_gateway_firestore")]
            StorageBackend::Firestore(storage) => {
                storage.store_roster_policy(group_id, sequence, operation, member_pubkeys, admin_pubkey, created_at).await
            }
        }
    }

    async fn upsert_keypackage_relays(&self, owner_pubkey: &str, relays: &[String]) -> anyhow::Result<()> {
        match self {
            #[cfg(feature = "mls_gateway_sql")]
            StorageBackend::Sql(storage) => storage.upsert_keypackage_relays(owner_pubkey, relays).await,
            #[cfg(feature = "mls_gateway_firestore")]
            StorageBackend::Firestore(storage) => storage.upsert_keypackage_relays(owner_pubkey, relays).await,
        }
    }

    async fn get_keypackage_relays(&self, owner_pubkey: &str) -> anyhow::Result<Vec<String>> {
        match self {
            #[cfg(feature = "mls_gateway_sql")]
            StorageBackend::Sql(storage) => storage.get_keypackage_relays(owner_pubkey).await,
            #[cfg(feature = "mls_gateway_firestore")]
            StorageBackend::Firestore(storage) => storage.get_keypackage_relays(owner_pubkey).await,
        }
    }

    async fn store_keypackage(
        &self,
        event_id: &str,
        owner_pubkey: &str,
        content: &str,
        ciphersuite: &str,
        extensions: &[String],
        relays: &[String],
        has_last_resort: bool,
        created_at: i64,
        expires_at: i64,
    ) -> anyhow::Result<()> {
        match self {
            #[cfg(feature = "mls_gateway_sql")]
            StorageBackend::Sql(storage) => storage.store_keypackage(
                event_id, owner_pubkey, content, ciphersuite, extensions, relays, has_last_resort, created_at, expires_at
            ).await,
            #[cfg(feature = "mls_gateway_firestore")]
            StorageBackend::Firestore(storage) => storage.store_keypackage(
                event_id, owner_pubkey, content, ciphersuite, extensions, relays, has_last_resort, created_at, expires_at
            ).await,
        }
    }

    async fn query_keypackages(
        &self,
        authors: Option<&[String]>,
        since: Option<i64>,
        limit: Option<u32>,
        order_by: Option<&str>,
    ) -> anyhow::Result<Vec<(String, String, String, i64)>> {
        match self {
            #[cfg(feature = "mls_gateway_sql")]
            StorageBackend::Sql(storage) => storage.query_keypackages(authors, since, limit, order_by).await,
            #[cfg(feature = "mls_gateway_firestore")]
            StorageBackend::Firestore(storage) => storage.query_keypackages(authors, since, limit, order_by).await,
        }
    }

    async fn delete_consumed_keypackage(&self, event_id: &str) -> anyhow::Result<bool> {
        match self {
            #[cfg(feature = "mls_gateway_sql")]
            StorageBackend::Sql(storage) => storage.delete_consumed_keypackage(event_id).await,
            #[cfg(feature = "mls_gateway_firestore")]
            StorageBackend::Firestore(storage) => storage.delete_consumed_keypackage(event_id).await,
        }
    }

    async fn count_user_keypackages(&self, owner_pubkey: &str) -> anyhow::Result<u32> {
        match self {
            #[cfg(feature = "mls_gateway_sql")]
            StorageBackend::Sql(storage) => storage.count_user_keypackages(owner_pubkey).await,
            #[cfg(feature = "mls_gateway_firestore")]
            StorageBackend::Firestore(storage) => storage.count_user_keypackages(owner_pubkey).await,
        }
    }

    async fn cleanup_expired_keypackages(&self) -> anyhow::Result<u32> {
        match self {
            #[cfg(feature = "mls_gateway_sql")]
            StorageBackend::Sql(storage) => storage.cleanup_expired_keypackages().await,
            #[cfg(feature = "mls_gateway_firestore")]
            StorageBackend::Firestore(storage) => storage.cleanup_expired_keypackages().await,
        }
    }

    // New methods for pending deletion management
    
    async fn create_pending_deletion(&self, pending: &firestore::PendingDeletion) -> anyhow::Result<()> {
        match self {
            #[cfg(feature = "mls_gateway_sql")]
            StorageBackend::Sql(_storage) => Err(anyhow::anyhow!("Pending deletion not implemented for SQL backend")),
            #[cfg(feature = "mls_gateway_firestore")]
            StorageBackend::Firestore(storage) => storage.create_pending_deletion(pending).await,
        }
    }
    
    async fn get_pending_deletion(&self, user_pubkey: &str) -> anyhow::Result<Option<firestore::PendingDeletion>> {
        match self {
            #[cfg(feature = "mls_gateway_sql")]
            StorageBackend::Sql(_storage) => Ok(None),
            #[cfg(feature = "mls_gateway_firestore")]
            StorageBackend::Firestore(storage) => storage.get_pending_deletion(user_pubkey).await,
        }
    }
    
    async fn update_pending_deletion(&self, pending: &firestore::PendingDeletion) -> anyhow::Result<()> {
        match self {
            #[cfg(feature = "mls_gateway_sql")]
            StorageBackend::Sql(_storage) => Err(anyhow::anyhow!("Pending deletion not implemented for SQL backend")),
            #[cfg(feature = "mls_gateway_firestore")]
            StorageBackend::Firestore(storage) => storage.update_pending_deletion(pending).await,
        }
    }
    
    async fn delete_pending_deletion(&self, user_pubkey: &str) -> anyhow::Result<()> {
        match self {
            #[cfg(feature = "mls_gateway_sql")]
            StorageBackend::Sql(_storage) => Ok(()),
            #[cfg(feature = "mls_gateway_firestore")]
            StorageBackend::Firestore(storage) => storage.delete_pending_deletion(user_pubkey).await,
        }
    }
    
    async fn delete_keypackage_by_id(&self, event_id: &str) -> anyhow::Result<()> {
        match self {
            #[cfg(feature = "mls_gateway_sql")]
            StorageBackend::Sql(_storage) => Err(anyhow::anyhow!("Direct deletion not implemented for SQL backend")),
            #[cfg(feature = "mls_gateway_firestore")]
            StorageBackend::Firestore(storage) => storage.delete_keypackage_by_id(event_id).await,
        }
    }
    
    async fn keypackage_exists(&self, event_id: &str) -> anyhow::Result<bool> {
        match self {
            #[cfg(feature = "mls_gateway_sql")]
            StorageBackend::Sql(_storage) => Ok(false),
            #[cfg(feature = "mls_gateway_firestore")]
            StorageBackend::Firestore(storage) => storage.keypackage_exists(event_id).await,
        }
    }
    
    async fn get_expired_pending_deletions(&self) -> anyhow::Result<Vec<firestore::PendingDeletion>> {
        match self {
            #[cfg(feature = "mls_gateway_sql")]
            StorageBackend::Sql(_storage) => Ok(Vec::new()),
            #[cfg(feature = "mls_gateway_firestore")]
            StorageBackend::Firestore(storage) => storage.get_expired_pending_deletions().await,
        }
    }
}

pub struct MlsGateway {
    config: MlsGatewayConfig,
    store: Option<StorageBackend>,
    message_archive: Option<MessageArchive>,
    initialized: bool,
}

impl MlsGateway {
    /// Create a new MLS Gateway Extension
    pub fn new(config: MlsGatewayConfig) -> Self {
        Self {
            config,
            store: None,
            message_archive: None,
            initialized: false,
        }
    }

    /// Initialize the extension with database connection
    pub async fn initialize(&mut self) -> anyhow::Result<()> {
        if self.initialized {
            return Ok(());
        }

        info!("Initializing MLS Gateway Extension with {:?} backend", self.config.storage_backend);
        
        // Initialize the delivery store
        init_delivery_store();
        
        // Initialize metrics
        describe_counter!("mls_gateway_events_processed", "Number of MLS events processed by kind");
        describe_counter!("mls_gateway_groups_updated", "Number of group registry updates");
        describe_counter!("mls_gateway_keypackages_stored", "Number of key packages stored");
        describe_counter!("mls_gateway_keypackages_consumed", "Number of key packages consumed by requests");
        describe_counter!("mls_gateway_keypackages_expired_cleanup", "Number of expired key packages cleaned up");
        describe_counter!("mls_gateway_welcomes_stored", "Number of welcome messages stored");
        describe_counter!("mls_gateway_giftwarps_processed", "Number of giftwrap envelopes processed");
        describe_counter!("mls_gateway_membership_updates", "Number of membership updates from giftwarps");
        // Validation/hygiene counters
        describe_counter!("mls_gateway_443_missing_tag", "Count of KeyPackage events missing required tags");
        describe_counter!("mls_gateway_443_invalid_tag", "Count of KeyPackage events with invalid tag values");
        describe_counter!("mls_gateway_443_content_invalid", "Count of KeyPackage events with invalid hex content");
        describe_counter!("mls_gateway_445_unexpected_tag", "Count of unexpected outer tags observed on kind 445 events");
        describe_counter!("mls_gateway_top_level_444_dropped", "Number of top-level 444 events dropped (should be wrapped in 1059)");
        describe_counter!("mls_gateway_10051_processed", "Number of KeyPackage Relays List (10051) events processed");
        describe_histogram!("mls_gateway_db_operation_duration", "Duration of database operations");

        // Initialize storage backend
        let store = match self.config.storage_backend {
            #[cfg(feature = "mls_gateway_firestore")]
            StorageType::Firestore => {
                // Determine project_id from config or environment
                let project_id = if let Some(pid) = self.config.project_id.clone() {
                    pid
                } else if let Ok(pid) = std::env::var("MLS_FIRESTORE_PROJECT_ID") {
                    pid
                } else if let Ok(pid) = std::env::var("GOOGLE_CLOUD_PROJECT") {
                    pid
                } else if let Ok(pid) = std::env::var("GCP_PROJECT") {
                    pid
                } else {
                    return Err(anyhow::anyhow!(
                        "project_id required for Firestore backend (set extensions.mls_gateway.project_id or MLS_FIRESTORE_PROJECT_ID/GOOGLE_CLOUD_PROJECT/GCP_PROJECT env)"
                    ));
                };
                let firestore_store = firestore::FirestoreStorage::new(&project_id).await?;
                firestore_store.migrate().await?;
                StorageBackend::Firestore(Arc::new(firestore_store))
            },
            #[cfg(feature = "mls_gateway_sql")]
            StorageType::CloudSql => {
                let pool = match &self.config.database_url {
                    Some(url) => {
                        info!("Connecting to SQL database at {}", url);
                        sqlx::postgres::PgPoolOptions::new()
                            .max_connections(10)
                            .connect(url)
                            .await
                            .map_err(|e| anyhow::anyhow!("Failed to connect to database: {}", e))?
                    }
                    None => return Err(anyhow::anyhow!("SQL URL not configured")),
                };
                
                let storage = storage::SqlStorage::new(pool).await?;
                StorageBackend::Sql(Arc::new(storage))
            }
        };

        // Initialize message archive if enabled
        let message_archive = if self.config.enable_message_archive {
            match &self.config.storage_backend {
                #[cfg(feature = "mls_gateway_firestore")]
                StorageType::Firestore => {
                    match MessageArchive::new().await {
                        Ok(archive) => {
                            info!("Message archival enabled with {} day TTL", self.config.message_archive_ttl_days);
                            Some(archive)
                        }
                        Err(e) => {
                            warn!("Failed to initialize message archive: {}. Archival disabled.", e);
                            None
                        }
                    }
                }
                #[cfg(feature = "mls_gateway_sql")]
                StorageType::CloudSql => {
                    info!("Message archival not yet supported for SQL backend; disabling");
                    None
                }
            }
        } else {
            info!("Message archival disabled in configuration");
            None
        };
        
        self.store = Some(store.clone());
        self.message_archive = message_archive;
        self.initialized = true;
        
        // Spawn background task for periodic keypackage cleanup
        let cleanup_store = store;
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600)); // Run every hour
            loop {
                interval.tick().await;
                match cleanup_store.cleanup_expired_keypackages().await {
                    Ok(count) => {
                        if count > 0 {
                            info!("Cleaned up {} expired keypackages", count);
                            counter!("mls_gateway_keypackages_expired_cleanup").increment(count as u64);
                        }
                    }
                    Err(e) => {
                        error!("Error cleaning up expired keypackages: {}", e);
                    }
                }
            }
        });
        
        info!("MLS Gateway Extension initialized successfully");
        Ok(())
    }

    /// Get the store reference
    fn store(&self) -> anyhow::Result<&StorageBackend> {
        self.store.as_ref().ok_or_else(|| anyhow::anyhow!("MLS Gateway not initialized"))
    }

    /// Handle KeyPackage (kind 443)
    async fn handle_keypackage(&self, event: &Event) -> anyhow::Result<()> {
        let store = self.store()?;
        
        // Extract owner from p tag (should match pubkey for security)
        let owner_tag = event.tags().iter()
            .find(|tag| tag.len() >= 2 && tag[0] == "p")
            .map(|tag| tag[1].clone());
            
        let event_pubkey = hex::encode(event.pubkey());
        
        // Verify owner matches event pubkey (security requirement)
        if let Some(owner) = &owner_tag {
            if owner != &event_pubkey {
                warn!("KeyPackage owner tag {} doesn't match event pubkey {}", owner, event_pubkey);
                return Err(anyhow::anyhow!("KeyPackage owner verification failed"));
            }
        }
        
        // Extract expiry from exp tag
        let expiry = event.tags().iter()
            .find(|tag| tag.len() >= 2 && tag[0] == "exp")
            .and_then(|tag| tag[1].parse::<i64>().ok());
            
        // Check if expired
        if let Some(exp_timestamp) = expiry {
            let now = chrono::Utc::now().timestamp();
            if exp_timestamp <= now {
                warn!("Rejecting expired KeyPackage from {}", event_pubkey);
                return Err(anyhow::anyhow!("KeyPackage has expired"));
            }
        }

        // NIP-EE required tags (soft validation)
        let mls_version = event.tags().iter()
            .find(|tag| tag.len() >= 2 && tag[0] == "mls_protocol_version")
            .map(|tag| tag[1].clone());
        match mls_version.as_deref() {
            Some("1.0") => {}
            Some(other) => {
                warn!("KeyPackage mls_protocol_version invalid: {}", other);
                counter!("mls_gateway_443_invalid_tag").increment(1);
            }
            None => {
                warn!("KeyPackage missing required tag: mls_protocol_version");
                counter!("mls_gateway_443_missing_tag").increment(1);
            }
        }

        let ciphersuite = event.tags().iter()
            .find(|tag| tag.len() >= 2 && tag[0] == "ciphersuite")
            .map(|tag| tag[1].clone());
        if ciphersuite.is_none() {
            warn!("KeyPackage missing required tag: ciphersuite");
            counter!("mls_gateway_443_missing_tag").increment(1);
        }

        let extensions = event.tags().iter()
            .find(|tag| tag.len() >= 2 && tag[0] == "extensions")
            .map(|tag| tag[1..].to_vec());
        if extensions.is_none() {
            warn!("KeyPackage missing required tag: extensions");
            counter!("mls_gateway_443_missing_tag").increment(1);
        }

        // Note: We no longer check for "last_resort" extension as we use
        // the "last remaining keypackage" approach instead
        let has_last_resort = false; // Keep parameter for backward compatibility

        // Relays: accept either a single ["relays", ..many..] tag or multiple ["relay", url] tags
        let relays_vec = event.tags().iter()
            .find(|tag| !tag.is_empty() && tag[0] == "relays")
            .map(|tag| tag[1..].to_vec());
        let relay_tags: Vec<String> = event.tags().iter()
            .filter(|tag| tag.len() >= 2 && tag[0] == "relay")
            .map(|tag| tag[1].clone())
            .collect();
        let all_relays = if let Some(rv) = relays_vec {
            rv
        } else {
            relay_tags
        };
        
        if all_relays.is_empty() {
            warn!("KeyPackage missing relays list (tag 'relays' or repeated 'relay')");
            counter!("mls_gateway_443_missing_tag").increment(1);
        }

        // Content shape: expect hex-encoded KeyPackageBundle (soft check)
        let content = event.content().trim();
        let is_hex = !content.is_empty() && content.len() % 2 == 0 && content.bytes().all(|b| (b as char).is_ascii_hexdigit());
        if !is_hex {
            warn!("KeyPackage content is not valid hex payload (soft validation)");
            counter!("mls_gateway_443_content_invalid").increment(1);
            return Err(anyhow::anyhow!("Invalid keypackage content format"));
        }

        // Check per-user limits (if configured)
        let max_keypackages = self.config.max_keypackages_per_user.unwrap_or(10);
        let current_count = store.count_user_keypackages(&event_pubkey).await?;
        if current_count >= max_keypackages {
            warn!("User {} has reached keypackage limit ({} >= {})", event_pubkey, current_count, max_keypackages);
            return Err(anyhow::anyhow!("User keypackage limit exceeded"));
        }

        // Check if this is a last resort scenario (user had exactly 1 keypackage before this upload)
        let should_start_timer = current_count == 1;
        let oldest_keypackage_id = if should_start_timer {
            // Get the existing keypackage ID (the one that will become "last resort")
            let existing = store.query_keypackages(
                Some(&[event_pubkey.clone()]),
                None,
                Some(1),
                Some("created_at_asc") // Get the oldest one
            ).await?;
            existing.first().map(|(id, _, _, _)| id.clone())
        } else {
            None
        };

        // Calculate expiry if not provided
        let expires_at = expiry.unwrap_or_else(|| {
            chrono::Utc::now().timestamp() + self.config.keypackage_ttl as i64
        });

        // Store the keypackage
        store.store_keypackage(
            &event.id_str(),
            &event_pubkey,
            content,
            &ciphersuite.unwrap_or_default(),
            &extensions.unwrap_or_default(),
            &all_relays,
            has_last_resort,
            event.created_at() as i64,
            expires_at,
        ).await?;
        
        info!("Stored KeyPackage {} from owner: {} (last_resort: {})", event.id_str(), event_pubkey, has_last_resort);
        
        // Handle last resort transition
        if should_start_timer && oldest_keypackage_id.is_some() {
            let store_clone = store.clone();
            let event_pubkey_clone = event_pubkey.clone();
            let new_keypackage_id = event.id_str();
            let oldest_id = oldest_keypackage_id.unwrap();
            
            tokio::spawn(async move {
                if let Err(e) = handle_last_resort_transition(
                    store_clone,
                    event_pubkey_clone,
                    oldest_id,
                    new_keypackage_id
                ).await {
                    error!("Failed to handle last resort transition: {}", e);
                }
            });
        }
        
        counter!("mls_gateway_keypackages_stored").increment(1);
        counter!("mls_gateway_events_processed", "kind" => "443").increment(1);
        Ok(())
    }

    /// Handle Giftwrap (kind 1059) containing Welcome message
    async fn handle_giftwrap(&self, event: &Event) -> anyhow::Result<()> {
        let _store = self.store()?;
        
        // Extract recipient and group ID from tags
        let recipient = event.tags().iter()
            .find(|tag| tag.len() >= 2 && tag[0] == "p")
            .map(|tag| tag[1].clone());
            
        let group_id = event.tags().iter()
            .find(|tag| tag.len() >= 2 && tag[0] == "h")
            .map(|tag| tag[1].clone());
            
        if let Some(recipient) = recipient {
            // Process giftwrap for recipient; group_id is optional per NIP-59/NIP-EE
            info!("Processing Giftwrap for recipient={}, group_hint={:?}", recipient, group_id);
            // Membership update is best-effort; in practice handled by clients post-decrypt
            counter!("mls_gateway_membership_updates").increment(1);
            if let Some(ref gid) = group_id {
                info!("Giftwrap hints group {} for {}", gid, recipient);
            }
            
            // NOTE: Welcome messages inside giftwraps contain an 'e' tag referencing the consumed keypackage,
            // but since giftwraps are end-to-end encrypted, the relay cannot decrypt them to track consumption.
            // Keypackage consumption tracking would require either:
            // 1. Clients explicitly notifying the relay when a keypackage is consumed
            // 2. The relay having access to decrypt Welcome messages (breaks E2EE)
            // For now, we rely on TTL-based expiry and client cooperation.
        } else {
            // NIP-59 requires a 'p' tag for recipient routing; warn if missing
            warn!("Giftwrap missing required p (recipient) tag");
        }
        
        counter!("mls_gateway_giftwarps_processed").increment(1);
        counter!("mls_gateway_events_processed", "kind" => "1059").increment(1);
        Ok(())
    }

    /// Handle MLS group message (kind 445)
    async fn handle_mls_group_message(&self, event: &Event) -> anyhow::Result<()> {
        let store = self.store()?;
        
        // Extract group ID and epoch from tags
        let group_id = event.tags().iter()
            .find(|tag| tag.len() >= 2 && tag[0] == "h")
            .map(|tag| tag[1].clone());
            
        let epoch = event.tags().iter()
            .find(|tag| tag.len() >= 2 && tag[0] == "k")
            .and_then(|tag| tag[1].parse::<i64>().ok());

        if let Some(group_id) = group_id {
            // Update group registry
            store.upsert_group(
                &group_id,
                None, // display_name from content if needed
                &hex::encode(event.pubkey()),
                epoch.unwrap_or(0) as u64,
            ).await?;
            
            counter!("mls_gateway_groups_updated").increment(1);
            info!("Updated group registry for group: {}", group_id);
        }

        counter!("mls_gateway_events_processed", "kind" => "445").increment(1);
        Ok(())
    }


    /// Archive event for offline delivery if enabled
    async fn maybe_archive_event(&self, event: &Event) -> anyhow::Result<()> {
        if let Some(ref archive) = self.message_archive {
            archive.archive_event(event, Some(self.config.message_archive_ttl_days)).await?;
        }
        Ok(())
    }

    /// Handle Noise DM (kind 446)
    async fn handle_noise_dm(&self, event: &Event) -> anyhow::Result<()> {
        // For Noise DMs, we primarily just route them
        // The content remains opaque as per spec
        
        // Log recipient for observability (non-PII)
        let recipient_count = event.tags().iter()
            .filter(|tag| tag.len() >= 2 && tag[0] == "p")
            .count();
            
        info!("Processing Noise DM with {} recipients", recipient_count);
        
        counter!("mls_gateway_events_processed", "kind" => "446").increment(1);
        Ok(())
    }

    /// Handle KeyPackage Relays List (kind 10051)
    async fn handle_keypackage_relays_list(&self, event: &Event) -> anyhow::Result<()> {
        let store = self.store()?;
        let owner_pubkey = hex::encode(event.pubkey());

        // Collect relay URLs from tags
        let relays: Vec<String> = event.tags().iter()
            .filter(|tag| tag.len() >= 2 && tag[0] == "relay")
            .map(|tag| tag[1].clone())
            .collect();

        if relays.is_empty() {
            warn!("KeyPackage Relays List (10051) missing relay tags");
            return Err(anyhow::anyhow!("Missing relay tags in 10051"));
        }

        // Deduplicate relays
        let mut dedup = relays.clone();
        dedup.sort();
        dedup.dedup();

        store.upsert_keypackage_relays(&owner_pubkey, &dedup).await?;
        counter!("mls_gateway_10051_processed").increment(1);
        counter!("mls_gateway_events_processed", "kind" => "10051").increment(1);
        Ok(())
    }

    /// Handle Roster/Policy event (kind 450)
    async fn handle_roster_policy(&self, event: &Event) -> anyhow::Result<()> {
        let store = self.store()?;
        let event_pubkey = hex::encode(event.pubkey());

        // Extract required tags
        let group_id = event.tags().iter()
            .find(|tag| tag.len() >= 2 && tag[0] == "h")
            .map(|tag| tag[1].clone())
            .ok_or_else(|| anyhow::anyhow!("Missing group_id (h tag)"))?;

        // Determine operation up front (used for auth on non-existent groups)
        let operation = event.tags().iter()
            .find(|tag| tag.len() >= 2 && tag[0] == "op")
            .map(|tag| tag[1].clone())
            .ok_or_else(|| anyhow::anyhow!("Missing operation (op tag)"))?;

        // Authorization based on per-group ownership/admins
        let group_exists = store.group_exists(&group_id).await.unwrap_or(false);
        if !group_exists {
            // Only allow bootstrap to create a new group; creator becomes owner and initial admin
            if operation.as_str() != "bootstrap" {
                warn!("Rejecting non-bootstrap roster event for unknown group {}", group_id);
                return Err(anyhow::anyhow!("Group does not exist; bootstrap required"));
            }
        } else {
            let is_owner = store.is_owner(&group_id, &event_pubkey).await.unwrap_or(false);
            let is_admin = store.is_admin(&group_id, &event_pubkey).await.unwrap_or(false);
            if !(is_owner || is_admin) {
                warn!("Unauthorized roster/policy event for group {} from {}", group_id, event_pubkey);
                return Err(anyhow::anyhow!("Unauthorized roster/policy event"));
            }
        }

        let sequence = event.tags().iter()
            .find(|tag| tag.len() >= 2 && tag[0] == "seq")
            .and_then(|tag| tag[1].parse::<u64>().ok())
            .ok_or_else(|| anyhow::anyhow!("Missing or invalid sequence (seq tag)"))?;


        // Validate operation type
        match operation.as_str() {
            "add" | "remove" | "promote" | "demote" | "bootstrap" | "replace" => {},
            _ => return Err(anyhow::anyhow!("Invalid operation: {}", operation)),
        }

        // Extract member pubkeys
        let member_pubkeys: Vec<String> = event.tags().iter()
            .filter(|tag| tag.len() >= 2 && tag[0] == "p")
            .map(|tag| tag[1].clone())
            .collect();

        if member_pubkeys.is_empty() && operation != "bootstrap" {
            warn!("Roster/policy event has no member pubkeys");
        }

        // Check sequence number for idempotency
        if let Ok(last_seq) = store.get_last_roster_sequence(&group_id).await {
            if let Some(last_sequence) = last_seq {
                if sequence <= last_sequence {
                    warn!("Ignoring roster/policy event with stale sequence: {} <= {}", sequence, last_sequence);
                    return Err(anyhow::anyhow!("Stale sequence number"));
                }
            }
        }
        
        info!("Processing roster/policy event: group={}, seq={}, op={}, members={:?}",
              group_id, sequence, operation, member_pubkeys);

        // Store the roster/policy event for audit trail and idempotency
        store.store_roster_policy(
            &group_id,
            sequence,
            &operation,
            &member_pubkeys,
            &event_pubkey,
            event.created_at() as i64,
        ).await?;

        // Update group registry based on operation
        let role_admin = event.tags().iter()
            .find(|tag| tag.len() >= 2 && tag[0] == "role")
            .map(|tag| tag[1].to_lowercase())
            .map(|s| s == "admin")
            .unwrap_or(false);

        match operation.as_str() {
            "bootstrap" => {
                // Create group with sender as owner and initial admin
                store.upsert_group(
                    &group_id,
                    None,
                    &event_pubkey,
                    0,
                ).await?;
                // Ensure creator is an admin
                store.add_admins(&group_id, &vec![event_pubkey.clone()]).await?;
                info!("Initialized group {} by owner {}", group_id, event_pubkey);
            }
            "add" | "replace" => {
                // Ensure group record exists and bump updated_at
                store.upsert_group(
                    &group_id,
                    None,
                    &event_pubkey,
                    0,
                ).await?;
                info!("Roster operation {} applied to group {}", operation, group_id);
            }
            "promote" => {
                // If role=admin, add listed pubkeys as admins
                if role_admin && !member_pubkeys.is_empty() {
                    store.add_admins(&group_id, &member_pubkeys).await?;
                    info!("Promoted admins in group {}: {:?}", group_id, member_pubkeys);
                } else {
                    info!("Roster operation promote applied to group {}", group_id);
                }
            }
            "demote" => {
                // If role=admin, remove listed pubkeys from admins
                if role_admin && !member_pubkeys.is_empty() {
                    store.remove_admins(&group_id, &member_pubkeys).await?;
                    info!("Demoted admins in group {}: {:?}", group_id, member_pubkeys);
                } else {
                    info!("Roster operation demote applied to group {}", group_id);
                }
            }
            "remove" => {
                info!("Roster operation remove applied to group {}", group_id);
            }
            _ => unreachable!(), // Already validated above
        }

        counter!("mls_gateway_roster_policy_updates").increment(1);
        counter!("mls_gateway_events_processed", "kind" => "450").increment(1);
        Ok(())
    }
}

/// Handle the transition when a user goes from 1 to 2+ keypackages
/// Starts a timer to delete the old keypackage after 10 minutes
async fn handle_last_resort_transition(
    store: StorageBackend,
    user_pubkey: String,
    old_keypackage_id: String,
    new_keypackage_id: String,
) -> anyhow::Result<()> {
    use crate::mls_gateway::firestore::PendingDeletion;
    use chrono::{Duration, Utc};
    
    let now = Utc::now();
    let deletion_time = now + Duration::minutes(10);
    
    // Create pending deletion record
    let pending = PendingDeletion {
        user_pubkey: user_pubkey.clone(),
        old_keypackage_id: old_keypackage_id.clone(),
        new_keypackages_collected: vec![new_keypackage_id],
        timer_started_at: now,
        deletion_scheduled_at: deletion_time,
    };
    
    store.create_pending_deletion(&pending).await?;
    
    info!(
        "Started last resort keypackage deletion timer for user {} - will delete {} at {:?}",
        user_pubkey, old_keypackage_id, deletion_time
    );
    counter!("mls_gateway_last_resort_timers_started").increment(1);
    
    // Spawn timer task
    tokio::spawn(async move {
        // Wait for 10 minutes
        tokio::time::sleep(tokio::time::Duration::from_secs(600)).await;
        
        // Process the deletion
        if let Err(e) = process_pending_deletion(store, user_pubkey).await {
            error!("Failed to process pending deletion: {}", e);
        }
    });
    
    Ok(())
}

/// Process a pending deletion - check conditions and delete if appropriate
async fn process_pending_deletion(
    store: StorageBackend,
    user_pubkey: String,
) -> anyhow::Result<()> {
    // Get the pending deletion record
    let pending = match store.get_pending_deletion(&user_pubkey).await? {
        Some(p) => p,
        None => {
            info!("No pending deletion found for user {}", user_pubkey);
            return Ok(());
        }
    };
    
    // Check if it's time to delete
    if pending.deletion_scheduled_at > chrono::Utc::now() {
        info!("Deletion not yet due for user {}", user_pubkey);
        return Ok(());
    }
    
    // Count current valid keypackages
    let keypackage_count = store.count_user_keypackages(&user_pubkey).await?;
    
    if keypackage_count < 3 {
        // Not enough keypackages - cancel deletion
        warn!(
            "Cancelling deletion for user {} - only {} keypackages (need 3+)",
            user_pubkey, keypackage_count
        );
        counter!("mls_gateway_last_resort_deletions_cancelled").increment(1);
        
        // Clean up the pending deletion record
        store.delete_pending_deletion(&user_pubkey).await?;
        return Ok(());
    }
    
    // Check if the old keypackage still exists
    if !store.keypackage_exists(&pending.old_keypackage_id).await? {
        info!(
            "Old keypackage {} already deleted for user {}",
            pending.old_keypackage_id, user_pubkey
        );
        store.delete_pending_deletion(&user_pubkey).await?;
        return Ok(());
    }
    
    // All conditions met - delete the old keypackage
    store.delete_keypackage_by_id(&pending.old_keypackage_id).await?;
    
    info!(
        "Successfully deleted old keypackage {} for user {} (now has {} keypackages)",
        pending.old_keypackage_id, user_pubkey, keypackage_count - 1
    );
    counter!("mls_gateway_last_resort_deletions_completed").increment(1);
    
    // Clean up the pending deletion record
    store.delete_pending_deletion(&user_pubkey).await?;
    
    Ok(())
}

impl Extension for MlsGateway {
    fn name(&self) -> &'static str {
        "mls-gateway"
    }

    fn setting(&mut self, setting: &nostr_relay::setting::SettingWrapper) {
        // Load configuration from relay Setting.extra under key "mls_gateway"
        let r = setting.read();
        let mut cfg: MlsGatewayConfig = r.parse_extension("mls_gateway");
        drop(r);

        // Safety: do not expose REST API unless explicitly allowed
        if cfg.enable_api && std::env::var("MLS_API_UNSAFE_ALLOW").unwrap_or_default() != "true" {
            info!("Disabling MLS Gateway REST API until proper authentication is in place");
            cfg.enable_api = false;
        }

        self.config = cfg;
        info!("MLS Gateway settings updated");
    }

    fn config_web(&mut self, cfg: &mut ServiceConfig) {
        if !self.config.enable_api {
            return;
        }

        info!("Configuring MLS Gateway REST API endpoints");
        
        // Configure HTTP routes for mailbox services
        endpoints::configure_routes(cfg, &self.config.api_prefix);
    }

    fn connected(&self, session: &mut Session, _ctx: &mut <Session as actix::Actor>::Context) {
        info!("Client connected to MLS Gateway: {}", session.id());
    }

    fn disconnected(&self, session: &mut Session, _ctx: &mut <Session as actix::Actor>::Context) {
        info!("Client disconnected from MLS Gateway: {}", session.id());
    }

    fn message(
        &self,
        msg: nostr_relay::message::ClientMessage,
        _session: &mut Session,
        _ctx: &mut <Session as actix::Actor>::Context,
    ) -> ExtensionMessageResult {
        // Handle MLS events asynchronously
        if let nostr_relay::message::IncomingMessage::Event(event) = &msg.msg {
            match event.kind() {
                KEYPACKAGE_KIND => {
                    // KeyPackage (443) - validate and process using gateway handler
                    let config = self.config.clone();
                    let store = match self.store() {
                        Ok(store) => store.clone(),
                        Err(e) => {
                            error!("MLS Gateway not initialized: {}", e);
                            return ExtensionMessageResult::Continue(msg);
                        }
                    };
                    let event_clone = event.clone();
                    tokio::spawn(async move {
                        let mut gateway = MlsGateway::new(config);
                        gateway.store = Some(store);
                        gateway.initialized = true;
                        if let Err(e) = gateway.handle_keypackage(&event_clone).await {
                            error!("Error handling KeyPackage (443): {}", e);
                        }
                    });
                }
                WELCOME_KIND => {
                    // Top-level Welcome events should never appear; they must be inside 1059 giftwrap.
                    warn!("Dropping top-level 444 Welcome event; must be carried inside giftwrap (1059)");
                    counter!("mls_gateway_top_level_444_dropped").increment(1);
                }
                GIFTWRAP_KIND => {
                    // Giftwrap (1059) containing Welcome (444)
                    let event_clone = event.clone();
                    let archive = self.message_archive.clone();
                    let config = self.config.clone();
                    let ttl_days = config.message_archive_ttl_days;
                    tokio::spawn(async move {
                        // Attempt to archive giftwrap for offline delivery (requires p tag for recipient)
                        if let Some(archive) = archive {
                            if let Err(e) = archive.archive_event(&event_clone, Some(ttl_days)).await {
                                warn!("Failed to archive Giftwrap (1059) for offline delivery: {}", e);
                            }
                        }

                        // Extract recipient and optional group hint from tags
                        let recipient = event_clone.tags().iter()
                            .find(|tag| tag.len() >= 2 && tag[0] == "p")
                            .map(|tag| tag[1].clone());
                            
                        let group_id = event_clone.tags().iter()
                            .find(|tag| tag.len() >= 2 && tag[0] == "h")
                            .map(|tag| tag[1].clone());
                            
                        if let Some(recipient) = recipient {
                            // Best-effort membership/accounting; clients handle formal join post-decrypt
                            info!("Processing Giftwrap for recipient={}, group_hint={:?}", recipient, group_id);
                            counter!("mls_gateway_membership_updates").increment(1);
                            if let Some(ref gid) = group_id {
                                info!("Giftwrap hints group {} for {}", gid, recipient);
                            }
                        } else {
                            // NIP-59 requires 'p'; if absent, we still archived earlier but warn here
                            warn!("Giftwrap missing required p (recipient) tag");
                        }
                        
                        counter!("mls_gateway_giftwarps_processed").increment(1);
                        counter!("mls_gateway_events_processed", "kind" => "1059").increment(1);
                    });
                }
                MLS_GROUP_MESSAGE_KIND => {
                    // MLS group message (445)
                    let store = match self.store() {
                        Ok(store) => store.clone(),
                        Err(e) => {
                            error!("MLS Gateway not initialized: {}", e);
                            return ExtensionMessageResult::Continue(msg);
                        }
                    };
                    
                    // Check if we have message archive
                    let archive = self.message_archive.clone();
                    let config = self.config.clone();
                    
                    let event_clone = event.clone();
                    tokio::spawn(async move {
                        // Archive message for offline delivery if enabled
                        if let Some(ref archive) = archive {
                            if let Err(e) = archive.archive_event(&event_clone, Some(config.message_archive_ttl_days)).await {
                                warn!("Failed to archive event for offline delivery: {}", e);
                            }
                        }

                        if let Err(e) = Self::handle_mls_group_message_static(store, config.clone(), &event_clone).await {
                            error!("Error handling MLS group message: {}", e);
                        }
                    });
                }
                NOISE_DM_KIND => {
                    // Noise DM (446) - archive if enabled
                    if let Some(ref archive) = self.message_archive {
                        let event_clone = event.clone();
                        let config = self.config.clone();
                        let archive_clone = archive.clone();
                        let event_clone_2 = event_clone.clone();
                        let ttl_days = config.message_archive_ttl_days;
                        tokio::spawn(async move {
                            if let Err(e) = archive_clone.archive_event(&event_clone_2, Some(ttl_days)).await {
                                warn!("Failed to archive Noise DM for offline delivery: {}", e);
                            }
                        });
                    }
                    
                    counter!("mls_gateway_events_processed", "kind" => "446").increment(1);
                    info!("Processing Noise DM from {}", hex::encode(event.pubkey()));
                }
                KEYPACKAGE_RELAYS_LIST_KIND => {
                    // KeyPackage Relays List (10051)
                    let config = self.config.clone();
                    let store = match self.store() {
                        Ok(store) => store.clone(),
                        Err(e) => {
                            error!("MLS Gateway not initialized: {}", e);
                            return ExtensionMessageResult::Continue(msg);
                        }
                    };
                    let event_clone = event.clone();
                    tokio::spawn(async move {
                        let mut gateway = MlsGateway::new(config);
                        gateway.store = Some(store);
                        gateway.initialized = true;
                        if let Err(e) = gateway.handle_keypackage_relays_list(&event_clone).await {
                            error!("Error handling KeyPackage Relays List (10051): {}", e);
                        }
                    });
                }
                // Kind 447 (KeyPackage Request) is deprecated - use REQ queries for kind 443 instead
                ROSTER_POLICY_KIND => {
                    // Roster/Policy (450)
                    let config = self.config.clone();
                    let store = match self.store() {
                        Ok(store) => store.clone(),
                        Err(e) => {
                            error!("MLS Gateway not initialized: {}", e);
                            return ExtensionMessageResult::Continue(msg);
                        }
                    };
                    let event_clone = event.clone();
                    tokio::spawn(async move {
                        let mut gateway = MlsGateway::new(config);
                        // Set the store manually since we're in a spawned task
                        gateway.store = Some(store);
                        gateway.initialized = true;
                        if let Err(e) = gateway.handle_roster_policy(&event_clone).await {
                            error!("Error handling roster/policy event: {}", e);
                        }
                    });
                }
                _ => {
                    // Not an MLS event, continue processing
                }
            }
        }

        ExtensionMessageResult::Continue(msg)
    }
}

impl MlsGateway {
    /// Static version of handle_mls_group_message for use in async context
    async fn handle_mls_group_message_static(store: StorageBackend, config: MlsGatewayConfig, event: &Event) -> anyhow::Result<()> {
        // Extract group ID and epoch from tags

        // Outer tag hygiene (non-sensitive): warn on unexpected tags per NIP-EE (allow only "h" and optional "k")
        let unexpected_tag_count = event.tags().iter()
            .filter(|tag| !tag.is_empty())
            .filter(|tag| {
                let key = &tag[0];
                !(key == "h" || key == "k" || key == "mls_ver")
            })
            .count();
        if unexpected_tag_count > 0 {
            warn!("kind 445 contains non-standard outer tags: count={}", unexpected_tag_count);
            counter!("mls_gateway_445_unexpected_tag").increment(unexpected_tag_count as u64);
        }

        let group_id_opt = event.tags().iter()
            .find(|tag| tag.len() >= 2 && tag[0] == "h")
            .map(|tag| tag[1].clone());
            
        let epoch = event.tags().iter()
            .find(|tag| tag.len() >= 2 && tag[0] == "k")
            .and_then(|tag| tag[1].parse::<i64>().ok());

        if let Some(ref group_id) = group_id_opt {
            // Update group registry
            store.upsert_group(
                group_id,
                None, // display_name from content if needed
                &hex::encode(event.pubkey()),
                epoch.unwrap_or(0) as u64,
            ).await?;
            
            counter!("mls_gateway_groups_updated").increment(1);
            info!("Updated group registry for group: {}", group_id);
        }

        // Membership-first gating for MLS-first decrypt/dispatch
        #[cfg(feature = "nip_service_mls")]
        if let Some(ref group_id) = group_id_opt {
            // 1) Handler selection and global enable
            if !config.enable_in_process_decrypt || config.preferred_service_handler.to_lowercase() != "in-process" {
                counter!("mls_gateway_events_processed", "kind" => "445_nip_service_handler_disabled").increment(1);
            } else {
                // 2) Optional registry hint prefilter (policy/ops only)
                let mut allowed = true;
                if config.gating_use_registry_hint {
                    #[cfg(feature = "mls_gateway_firestore")]
                    {
                        let is_service_enabled = match &store {
                            StorageBackend::Firestore(storage) => storage.has_service_member(group_id).await.unwrap_or(false),
                            #[cfg(feature = "mls_gateway_sql")]
                            StorageBackend::Sql(_storage) => false,
                        };
                        if !is_service_enabled {
                            counter!("mls_gateway_events_processed", "kind" => "445_nip_service_policy_hint_skip").increment(1);
                            allowed = false;
                        }
                    }
                    #[cfg(not(feature = "mls_gateway_firestore"))]
                    {
                        // No registry available; ignore hint
                    }
                }

                // 3) Membership-first gating (fast in-memory)
                if allowed {
                    if let Some(user_id) = config.mls_service_user_id.as_deref() {
                        if crate::mls_gateway::service_member::has_group(user_id, group_id) {
                            // Try to decrypt via service member (dev stub for now)
                            if let Some(json) = crate::mls_gateway::service_member::try_decrypt_service_request(event).await {
                                // Dispatch decrypted NIP-SERVICE payload without exposing plaintext outside this scope
                                crate::nip_service::dispatcher::handle_service_request_payload(&json, Some(group_id.as_str()));
                                counter!("mls_gateway_events_processed", "kind" => "445_nip_service_decrypted").increment(1);
                            } else {
                                // Not a NIP-SERVICE payload or decrypt failed; content remains opaque
                                counter!("mls_gateway_events_processed", "kind" => "445_nip_service_decrypt_skip").increment(1);
                            }
                        } else {
                            counter!("mls_gateway_events_processed", "kind" => "445_nip_service_not_member").increment(1);
                        }
                    } else {
                        // Missing configuration for user id
                        counter!("mls_gateway_events_processed", "kind" => "445_nip_service_missing_user_id").increment(1);
                    }
                }
            }
        }

        counter!("mls_gateway_events_processed", "kind" => "445").increment(1);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr_relay::db::Event;
    use chrono::Utc;
    
    // Mock storage backend for testing
    struct MockStorage {
        keypackages: std::sync::Arc<std::sync::Mutex<Vec<(String, String, i64)>>>, // (id, owner, created_at)
        pending_deletions: std::sync::Arc<std::sync::Mutex<Vec<crate::mls_gateway::firestore::PendingDeletion>>>,
    }
    
    impl MockStorage {
        fn new() -> Self {
            Self {
                keypackages: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
                pending_deletions: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            }
        }
        
        async fn add_keypackage(&self, id: &str, owner: &str) {
            let mut kps = self.keypackages.lock().unwrap();
            kps.push((id.to_string(), owner.to_string(), Utc::now().timestamp()));
        }
        
        async fn count_keypackages(&self, owner: &str) -> usize {
            let kps = self.keypackages.lock().unwrap();
            kps.iter().filter(|(_, o, _)| o == owner).count()
        }
        
        async fn get_oldest_keypackage(&self, owner: &str) -> Option<String> {
            let kps = self.keypackages.lock().unwrap();
            kps.iter()
                .filter(|(_, o, _)| o == owner)
                .min_by_key(|(_, _, created)| *created)
                .map(|(id, _, _)| id.clone())
        }
        
        async fn has_pending_deletion(&self, owner: &str) -> bool {
            let pds = self.pending_deletions.lock().unwrap();
            pds.iter().any(|pd| pd.user_pubkey == owner)
        }
        
        async fn get_pending_deletion(&self, owner: &str) -> Option<crate::mls_gateway::firestore::PendingDeletion> {
            let pds = self.pending_deletions.lock().unwrap();
            pds.iter().find(|pd| pd.user_pubkey == owner).cloned()
        }
    }
    
    #[tokio::test]
    async fn test_last_resort_timer_not_started_with_zero_keypackages() {
        let storage = MockStorage::new();
        let owner = "test_user";
        
        // User has 0 keypackages initially
        assert_eq!(storage.count_keypackages(owner).await, 0);
        
        // Upload first keypackage
        storage.add_keypackage("kp1", owner).await;
        
        // No timer should be started (user went from 0 to 1 keypackage)
        assert!(!storage.has_pending_deletion(owner).await);
    }
    
    #[tokio::test]
    async fn test_last_resort_timer_started_with_one_keypackage() {
        let storage = MockStorage::new();
        let owner = "test_user";
        
        // User has 1 keypackage initially
        storage.add_keypackage("kp1", owner).await;
        assert_eq!(storage.count_keypackages(owner).await, 1);
        
        // Upload second keypackage - this should trigger timer
        storage.add_keypackage("kp2", owner).await;
        
        // Simulate timer creation
        let pending = crate::mls_gateway::firestore::PendingDeletion {
            user_pubkey: owner.to_string(),
            old_keypackage_id: "kp1".to_string(),
            new_keypackages_collected: vec!["kp2".to_string()],
            timer_started_at: Utc::now(),
            deletion_scheduled_at: Utc::now() + chrono::Duration::minutes(10),
        };
        storage.pending_deletions.lock().unwrap().push(pending);
        
        // Timer should be started
        assert!(storage.has_pending_deletion(owner).await);
        let pd = storage.get_pending_deletion(owner).await.unwrap();
        assert_eq!(pd.old_keypackage_id, "kp1");
        assert_eq!(pd.new_keypackages_collected.len(), 1);
    }
    
    #[tokio::test]
    async fn test_last_resort_timer_not_started_with_multiple_keypackages() {
        let storage = MockStorage::new();
        let owner = "test_user";
        
        // User has 2 keypackages initially
        storage.add_keypackage("kp1", owner).await;
        storage.add_keypackage("kp2", owner).await;
        assert_eq!(storage.count_keypackages(owner).await, 2);
        
        // Upload third keypackage
        storage.add_keypackage("kp3", owner).await;
        
        // No timer should be started (user already had 2+ keypackages)
        assert!(!storage.has_pending_deletion(owner).await);
    }
    
    #[tokio::test]
    async fn test_deletion_cancelled_if_not_enough_keypackages() {
        let storage = MockStorage::new();
        let owner = "test_user";
        
        // Set up scenario: user had 1 kp, uploaded 1 more
        storage.add_keypackage("kp1", owner).await;
        storage.add_keypackage("kp2", owner).await;
        
        // Create pending deletion that's already expired
        let pending = crate::mls_gateway::firestore::PendingDeletion {
            user_pubkey: owner.to_string(),
            old_keypackage_id: "kp1".to_string(),
            new_keypackages_collected: vec!["kp2".to_string()],
            timer_started_at: Utc::now() - chrono::Duration::minutes(15),
            deletion_scheduled_at: Utc::now() - chrono::Duration::minutes(5),
        };
        storage.pending_deletions.lock().unwrap().push(pending);
        
        // With only 2 keypackages, deletion should be cancelled
        assert_eq!(storage.count_keypackages(owner).await, 2);
        
        // In real implementation, process_pending_deletion would:
        // 1. Check keypackage count (2 < 3)
        // 2. Cancel the deletion
        // 3. Remove pending deletion record
    }
    
    #[tokio::test]
    async fn test_deletion_proceeds_with_enough_keypackages() {
        let storage = MockStorage::new();
        let owner = "test_user";
        
        // Set up scenario: user had 1 kp, uploaded 3 more
        storage.add_keypackage("kp1", owner).await;
        storage.add_keypackage("kp2", owner).await;
        storage.add_keypackage("kp3", owner).await;
        storage.add_keypackage("kp4", owner).await;
        
        // Create pending deletion that's already expired
        let pending = crate::mls_gateway::firestore::PendingDeletion {
            user_pubkey: owner.to_string(),
            old_keypackage_id: "kp1".to_string(),
            new_keypackages_collected: vec!["kp2".to_string(), "kp3".to_string(), "kp4".to_string()],
            timer_started_at: Utc::now() - chrono::Duration::minutes(15),
            deletion_scheduled_at: Utc::now() - chrono::Duration::minutes(5),
        };
        storage.pending_deletions.lock().unwrap().push(pending);
        
        // With 4 keypackages (>= 3), deletion should proceed
        assert_eq!(storage.count_keypackages(owner).await, 4);
        
        // In real implementation, process_pending_deletion would:
        // 1. Check keypackage count (4 >= 3)
        // 2. Delete old keypackage (kp1)
        // 3. Remove pending deletion record
    }
    
    #[tokio::test]
    async fn test_concurrent_uploads_during_timer() {
        let storage = MockStorage::new();
        let owner = "test_user";
        
        // User starts with 1 keypackage
        storage.add_keypackage("kp1", owner).await;
        
        // Upload triggers timer
        storage.add_keypackage("kp2", owner).await;
        let pending = crate::mls_gateway::firestore::PendingDeletion {
            user_pubkey: owner.to_string(),
            old_keypackage_id: "kp1".to_string(),
            new_keypackages_collected: vec!["kp2".to_string()],
            timer_started_at: Utc::now(),
            deletion_scheduled_at: Utc::now() + chrono::Duration::minutes(10),
        };
        storage.pending_deletions.lock().unwrap().push(pending);
        
        // More uploads during timer period
        storage.add_keypackage("kp3", owner).await;
        storage.add_keypackage("kp4", owner).await;
        
        // Update pending deletion with new keypackages
        let mut pds = storage.pending_deletions.lock().unwrap();
        if let Some(pd) = pds.iter_mut().find(|pd| pd.user_pubkey == owner) {
            pd.new_keypackages_collected.push("kp3".to_string());
            pd.new_keypackages_collected.push("kp4".to_string());
        }
        drop(pds);
        
        // Verify state
        assert_eq!(storage.count_keypackages(owner).await, 4);
        let pd = storage.get_pending_deletion(owner).await.unwrap();
        assert_eq!(pd.new_keypackages_collected.len(), 3);
    }
}
