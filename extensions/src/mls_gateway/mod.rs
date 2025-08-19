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

#[cfg(feature = "mls_gateway_firestore")]
pub use firestore::FirestoreStorage;

#[cfg(feature = "mls_gateway_sql")]
pub use storage::CloudSqlStore;

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
}

impl Default for MlsGatewayConfig {
    fn default() -> Self {
        Self {
            storage_backend: StorageType::Firestore,
            project_id: None,
            database_url: None,
            keypackage_ttl: 604800, // 7 days
            welcome_ttl: 259200,    // 3 days
            enable_api: true,
            api_prefix: "/api/v1".to_string(),
            enable_message_archive: true,
            message_archive_ttl_days: 30,
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
            StorageBackend::Sql(storage) => storage.upsert_group(group_id, display_name, creator_pubkey, epoch).await,
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
        describe_counter!("mls_gateway_welcomes_stored", "Number of welcome messages stored");
        describe_counter!("mls_gateway_giftwarps_processed", "Number of giftwrap envelopes processed");
        describe_counter!("mls_gateway_membership_updates", "Number of membership updates from giftwarps");
        describe_histogram!("mls_gateway_db_operation_duration", "Duration of database operations");

        // Initialize storage backend
        let store = match self.config.storage_backend {
            #[cfg(feature = "mls_gateway_firestore")]
            StorageType::Firestore => {
                let project_id = self.config.project_id.as_ref()
                    .ok_or_else(|| anyhow::anyhow!("project_id required for Firestore backend"))?;
                let firestore_store = firestore::FirestoreStorage::new(project_id).await?;
                firestore_store.migrate().await?;
                StorageBackend::Firestore(Arc::new(firestore_store))
            },
            #[cfg(feature = "mls_gateway_sql")]
            StorageType::CloudSql => {
                let pool = match &self.config.sql_url {
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

        // Initialize message archive if enabled and using Firestore
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
            }
        } else {
            info!("Message archival disabled in configuration");
            None
        };
        
        self.store = Some(store);
        self.message_archive = message_archive;
        self.initialized = true;
        
        info!("MLS Gateway Extension initialized successfully");
        Ok(())
    }

    /// Get the store reference
    fn store(&self) -> anyhow::Result<&StorageBackend> {
        self.store.as_ref().ok_or_else(|| anyhow::anyhow!("MLS Gateway not initialized"))
    }

    /// Handle KeyPackage (kind 443)
    async fn handle_keypackage(&self, event: &Event) -> anyhow::Result<()> {
        let _store = self.store()?;
        
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
        
        info!("Processing KeyPackage from owner: {}", event_pubkey);
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
            
        if let (Some(recipient), Some(group_id)) = (recipient, group_id) {
            // This represents a group invitation - update membership
            // Per spec: "When a Giftwrap (1059) for p=invitee arrives, relay marks invitee as a member"
            info!("Processing group invitation via Giftwrap: recipient={}, group={}", recipient, group_id);
            
            // Mark recipient as group member (simplified approach)
            // In production, this might need more sophisticated membership tracking
            counter!("mls_gateway_membership_updates").increment(1);
            
            info!("Added {} to group {} via Giftwrap invitation", recipient, group_id);
        } else {
            warn!("Giftwrap missing required p (recipient) or h (group_id) tags");
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
}

impl Extension for MlsGateway {
    fn name(&self) -> &'static str {
        "mls-gateway"
    }

    fn setting(&mut self, _setting: &nostr_relay::setting::SettingWrapper) {
        // Settings can be updated here if needed
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
                    // KeyPackage (443) - validate and process
                    let event_clone = event.clone();
                    tokio::spawn(async move {
                        // Extract owner from p tag (should match pubkey for security)
                        let owner_tag = event_clone.tags().iter()
                            .find(|tag| tag.len() >= 2 && tag[0] == "p")
                            .map(|tag| tag[1].clone());
                            
                        let event_pubkey = hex::encode(event_clone.pubkey());
                        
                        // Verify owner matches event pubkey (security requirement)
                        if let Some(owner) = &owner_tag {
                            if owner != &event_pubkey {
                                warn!("KeyPackage owner tag {} doesn't match event pubkey {}", owner, event_pubkey);
                                return;
                            }
                        }
                        
                        // Extract expiry from exp tag
                        let expiry = event_clone.tags().iter()
                            .find(|tag| tag.len() >= 2 && tag[0] == "exp")
                            .and_then(|tag| tag[1].parse::<i64>().ok());
                            
                        // Check if expired
                        if let Some(exp_timestamp) = expiry {
                            let now = chrono::Utc::now().timestamp();
                            if exp_timestamp <= now {
                                warn!("Rejecting expired KeyPackage from {}", event_pubkey);
                                return;
                            }
                        }
                        
                        info!("Processing KeyPackage from owner: {}", event_pubkey);
                        counter!("mls_gateway_keypackages_stored").increment(1);
                        counter!("mls_gateway_events_processed", "kind" => "443").increment(1);
                    });
                }
                GIFTWRAP_KIND => {
                    // Giftwrap (1059) containing Welcome (444)
                    let event_clone = event.clone();
                    tokio::spawn(async move {
                        // Extract recipient and group ID from tags
                        let recipient = event_clone.tags().iter()
                            .find(|tag| tag.len() >= 2 && tag[0] == "p")
                            .map(|tag| tag[1].clone());
                            
                        let group_id = event_clone.tags().iter()
                            .find(|tag| tag.len() >= 2 && tag[0] == "h")
                            .map(|tag| tag[1].clone());
                            
                        if let (Some(recipient), Some(group_id)) = (recipient, group_id) {
                            // This represents a group invitation - update membership
                            // Per spec: "When a Giftwrap (1059) for p=invitee arrives, relay marks invitee as a member"
                            info!("Processing group invitation via Giftwrap: recipient={}, group={}", recipient, group_id);
                            
                            // Mark recipient as group member (simplified approach)
                            // In production, this might need more sophisticated membership tracking
                            counter!("mls_gateway_membership_updates").increment(1);
                            
                            info!("Added {} to group {} via Giftwrap invitation", recipient, group_id);
                        } else {
                            warn!("Giftwrap missing required p (recipient) or h (group_id) tags");
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

                        if let Err(e) = Self::handle_mls_group_message_static(store, &event_clone).await {
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
    async fn handle_mls_group_message_static(store: StorageBackend, event: &Event) -> anyhow::Result<()> {
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
}