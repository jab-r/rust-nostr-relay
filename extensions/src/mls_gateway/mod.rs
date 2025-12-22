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
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{info, warn, error};
use metrics::{counter, describe_counter, describe_histogram};

// MLS and Noise event kinds as per specification
const KEYPACKAGE_KIND: u16 = 443;         // MLS KeyPackage
const WELCOME_KIND: u16 = 444;            // MLS Welcome (embedded in 1059)
const MLS_GROUP_MESSAGE_KIND: u16 = 445;  // MLS Group Message
const NOISE_DM_KIND: u16 = 446;           // Noise Direct Message
const KEYPACKAGE_REQUEST_KIND: u16 = 447; // KeyPackage Request (Nostr-based)
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
    /// System/relay pubkey for KeyPackage requests (kind 447)
    pub system_pubkey: Option<String>,
    /// Admin pubkeys allowed to send roster/policy events (kind 450)
    pub admin_pubkeys: Vec<String>,
    /// TTL for KeyPackage requests in seconds (default: 7 days)
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
    ) -> anyhow::Result<Vec<(String, String, String, i64)>>; // (event_id, owner_pubkey, content, created_at)
    
    /// Delete a consumed keypackage (unless it's a last resort keypackage)
    async fn delete_consumed_keypackage(&self, event_id: &str) -> anyhow::Result<bool>; // returns true if deleted
    
    /// Count keypackages per user
    async fn count_user_keypackages(&self, owner_pubkey: &str) -> anyhow::Result<u32>;
    
    /// Clean up expired keypackages
    async fn cleanup_expired_keypackages(&self) -> anyhow::Result<u32>;
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
    ) -> anyhow::Result<Vec<(String, String, String, i64)>> {
        match self {
            #[cfg(feature = "mls_gateway_sql")]
            StorageBackend::Sql(storage) => storage.query_keypackages(authors, since, limit).await,
            #[cfg(feature = "mls_gateway_firestore")]
            StorageBackend::Firestore(storage) => storage.query_keypackages(authors, since, limit).await,
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
        describe_counter!("mls_gateway_keypackage_requests_processed", "Number of KeyPackage requests (447) processed");
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

    /// Handle KeyPackage Request (kind 447)
    async fn handle_keypackage_request(&self, event: &Event) -> anyhow::Result<()> {
        let event_pubkey = hex::encode(event.pubkey());

        // Authorization:
        // For MLS to work, any authenticated user must be able to request keypackages
        // to create groups and add members. This is a core user functionality, not admin-only.
        
        // Log the request for visibility
        info!("KeyPackage request received from authenticated user: {}", event_pubkey);
        
        // If scoped to a group (h tag present), optionally verify membership
        // But for now, allow any authenticated user to request keypackages
        let scoped_group = event.tags().iter()
            .find(|tag| tag.len() >= 2 && tag[0] == "h")
            .map(|tag| tag[1].clone());
            
        if let Some(group_id) = &scoped_group {
            info!("KeyPackage request scoped to group: {}", group_id);
            // Note: We're allowing any authenticated user to request keypackages
            // even for group-scoped requests, as they may need them to join the group
        }

        // Extract recipient from p tag
        let recipient = event.tags().iter()
            .find(|tag| tag.len() >= 2 && tag[0] == "p")
            .map(|tag| tag[1].clone());

        if let Some(recipient_pubkey) = recipient {
            // Extract optional parameters
            let group_id = event.tags().iter()
                .find(|tag| tag.len() >= 2 && tag[0] == "h")
                .map(|tag| tag[1].clone());

            let ciphersuite = event.tags().iter()
                .find(|tag| tag.len() >= 2 && tag[0] == "cs")
                .map(|tag| tag[1].clone());

            let min_count = event.tags().iter()
                .find(|tag| tag.len() >= 2 && tag[0] == "min")
                .and_then(|tag| tag[1].parse::<u32>().ok())
                .unwrap_or(1);

            let ttl = event.tags().iter()
                .find(|tag| tag.len() >= 2 && tag[0] == "ttl")
                .and_then(|tag| tag[1].parse::<u64>().ok())
                .unwrap_or(self.config.keypackage_request_ttl);

            // Check if request has expired
            let now = chrono::Utc::now().timestamp() as u64;
            let created_at = event.created_at() as u64;
            if created_at + ttl < now {
                warn!("KeyPackage request has expired");
                return Err(anyhow::anyhow!("KeyPackage request has expired"));
            }

            info!("Processing KeyPackage request: recipient={}, group={:?}, ciphersuite={:?}, min={}",
                  recipient_pubkey, group_id, ciphersuite, min_count);

            // Query available keypackages for the recipient
            let store = self.store()?;
            let keypackages = store.query_keypackages(
                Some(&[recipient_pubkey.clone()]),
                None, // No since filter
                Some(min_count.max(10)) // Limit to reasonable number
            ).await?;

            info!("Found {} keypackages for recipient {}", keypackages.len(), recipient_pubkey);

            // Mark keypackages as consumed (except the last remaining one)
            // As per user suggestion: "we could consider a keypackage that has been delivered by request to a client as 'consumed'"
            let mut consumed_count = 0;
            for (event_id, _owner, _content, _created_at) in &keypackages {
                // Try to delete (which preserves the last remaining keypackage internally)
                if store.delete_consumed_keypackage(event_id).await? {
                    consumed_count += 1;
                }
            }

            if consumed_count > 0 {
                info!("Marked {} keypackages as consumed for recipient {} (preserving last remaining)",
                      consumed_count, recipient_pubkey);
                counter!("mls_gateway_keypackages_consumed").increment(consumed_count);
            }

            // Note: The actual delivery of keypackages to the requester would happen through
            // the normal Nostr REQ/EVENT flow. The requester can query for kind 443 events
            // authored by the recipient after making this request.

            counter!("mls_gateway_keypackage_requests_processed").increment(1);
            counter!("mls_gateway_events_processed", "kind" => "447").increment(1);
            Ok(())
        } else {
            warn!("KeyPackage request missing recipient (p tag)");
            Err(anyhow::anyhow!("Missing recipient in KeyPackage request"))
        }
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
                KEYPACKAGE_REQUEST_KIND => {
                    // KeyPackage Request (447)
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
                        if let Err(e) = gateway.handle_keypackage_request(&event_clone).await {
                            error!("Error handling KeyPackage request: {}", e);
                        }
                    });
                }
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
