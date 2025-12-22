//! Cleanup command for expired keypackages
//!
//! This module provides a cleanup command that can be run as a Cloud Run Job
//! to remove expired keypackages from Firestore.

use anyhow::Result;
use tracing::{info, error};

/// Run cleanup of expired keypackages
#[cfg(feature = "mls_gateway_firestore")]
pub async fn run_cleanup() -> Result<()> {
    use nostr_extensions::mls_gateway::firestore::FirestoreStorage;
    
    info!("Starting keypackage cleanup job");
    
    // Get project ID from environment
    let project_id = if let Ok(pid) = std::env::var("MLS_FIRESTORE_PROJECT_ID") {
        pid
    } else if let Ok(pid) = std::env::var("GOOGLE_CLOUD_PROJECT") {
        pid
    } else if let Ok(pid) = std::env::var("GCP_PROJECT") {
        pid
    } else {
        error!("Firestore project ID not configured");
        return Err(anyhow::anyhow!("Firestore project ID not configured"));
    };
    
    info!("Connecting to Firestore project: {}", project_id);
    
    // Initialize Firestore storage
    let storage = FirestoreStorage::new(&project_id).await?;
    
    // Run cleanup
    match storage.cleanup_expired_keypackages().await {
        Ok(deleted_count) => {
            info!("Cleanup complete: deleted {} expired keypackages", deleted_count);
            Ok(())
        }
        Err(e) => {
            error!("Cleanup failed: {}", e);
            Err(e)
        }
    }
}

#[cfg(not(feature = "mls_gateway_firestore"))]
pub async fn run_cleanup() -> Result<()> {
    error!("Cleanup command requires mls_gateway_firestore feature");
    Err(anyhow::anyhow!("Cleanup command requires mls_gateway_firestore feature to be enabled"))
}