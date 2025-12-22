//! KeyPackage delivery mechanism for MLS Gateway
//!
//! This module handles the delivery of KeyPackages in response to kind 447 requests.
//! It stores pending deliveries that are picked up by the reader during normal queries.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use chrono::{DateTime, Utc, Duration};
use serde::{Serialize, Deserialize};
use tracing::{info, warn};

/// A pending KeyPackage delivery
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingKeyPackageDelivery {
    pub requester_pubkey: String,
    pub keypackage_event_ids: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

/// In-memory store for pending KeyPackage deliveries
/// This is a temporary solution - in production this should use persistent storage
#[derive(Debug, Clone)]
pub struct KeyPackageDeliveryStore {
    /// Map from requester pubkey to pending deliveries
    pending: Arc<RwLock<HashMap<String, Vec<PendingKeyPackageDelivery>>>>,
}

impl KeyPackageDeliveryStore {
    pub fn new() -> Self {
        Self {
            pending: Arc::new(RwLock::new(HashMap::new())),
        }
    }
    
    /// Add a pending delivery for a requester
    pub async fn add_pending_delivery(
        &self,
        requester_pubkey: String,
        keypackage_event_ids: Vec<String>,
    ) -> anyhow::Result<()> {
        let keypackage_count = keypackage_event_ids.len();
        let delivery = PendingKeyPackageDelivery {
            requester_pubkey: requester_pubkey.clone(),
            keypackage_event_ids,
            created_at: Utc::now(),
            expires_at: Utc::now() + Duration::minutes(5),
        };
        
        let mut pending = self.pending.write().await;
        pending
            .entry(requester_pubkey.clone())
            .or_insert_with(Vec::new)
            .push(delivery);
            
        info!("Added pending delivery for {} with {} KeyPackages",
              requester_pubkey, keypackage_count);
        
        Ok(())
    }
    
    /// Get and consume pending deliveries for a requester
    pub async fn get_pending_deliveries(
        &self,
        requester_pubkey: &str,
    ) -> Vec<PendingKeyPackageDelivery> {
        let mut pending = self.pending.write().await;
        
        // Take all deliveries for this requester
        if let Some(mut deliveries) = pending.remove(requester_pubkey) {
            // Filter out expired ones
            let now = Utc::now();
            deliveries.retain(|d| d.expires_at > now);
            
            if !deliveries.is_empty() {
                info!("Retrieved {} pending deliveries for {}", 
                      deliveries.len(), requester_pubkey);
            }
            
            deliveries
        } else {
            Vec::new()
        }
    }
    
    /// Clean up expired deliveries
    pub async fn cleanup_expired(&self) -> usize {
        let mut pending = self.pending.write().await;
        let now = Utc::now();
        let mut total_removed = 0;
        
        // Remove expired deliveries from all requesters
        pending.retain(|requester, deliveries| {
            let before = deliveries.len();
            deliveries.retain(|d| d.expires_at > now);
            let removed = before - deliveries.len();
            
            if removed > 0 {
                warn!("Cleaned up {} expired deliveries for {}", removed, requester);
                total_removed += removed;
            }
            
            // Keep the entry only if there are still deliveries
            !deliveries.is_empty()
        });
        
        total_removed
    }
    
    /// Check if a requester has pending deliveries
    pub async fn has_pending_deliveries(&self, requester_pubkey: &str) -> bool {
        let pending = self.pending.read().await;
        pending.contains_key(requester_pubkey)
    }
}

/// Global delivery store instance
/// This is initialized in the MLS Gateway extension
static mut DELIVERY_STORE: Option<KeyPackageDeliveryStore> = None;

/// Initialize the global delivery store
pub fn init_delivery_store() {
    unsafe {
        DELIVERY_STORE = Some(KeyPackageDeliveryStore::new());
    }
}

/// Get the global delivery store
pub fn get_delivery_store() -> Option<&'static KeyPackageDeliveryStore> {
    unsafe {
        DELIVERY_STORE.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_pending_delivery() {
        let store = KeyPackageDeliveryStore::new();
        
        // Add a delivery
        store.add_pending_delivery(
            "alice".to_string(),
            vec!["event1".to_string(), "event2".to_string()],
        ).await.unwrap();
        
        // Check it exists
        assert!(store.has_pending_deliveries("alice").await);
        assert!(!store.has_pending_deliveries("bob").await);
        
        // Retrieve it
        let deliveries = store.get_pending_deliveries("alice").await;
        assert_eq!(deliveries.len(), 1);
        assert_eq!(deliveries[0].keypackage_event_ids.len(), 2);
        
        // Should be consumed
        assert!(!store.has_pending_deliveries("alice").await);
    }
}