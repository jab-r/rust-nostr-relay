//! REQ message interceptor for automatic KeyPackage consumption
//!
//! This module intercepts REQ messages for kind 443 (KeyPackages) and
//! automatically consumes them when delivered.

use crate::mls_gateway::MlsGateway;
use nostr_relay::db::{Event, Filter};
use std::collections::HashSet;
use tracing::{info, warn};
use metrics::counter;

impl MlsGateway {
    /// Check if a filter is requesting KeyPackages (kind 443)
    pub fn is_keypackage_query(filters: &[Filter]) -> bool {
        filters.iter().any(|f| {
            f.kinds.iter().any(|&k| k == 443)
        })
    }
    
    /// Extract authors from filters
    pub fn extract_authors(filters: &[Filter]) -> HashSet<String> {
        let mut authors = HashSet::new();
        for filter in filters {
            // Authors are byte arrays in the filter, convert to hex strings
            for author in filter.authors.iter() {
                authors.insert(hex::encode(author));
            }
        }
        authors
    }
    
    /// Process KeyPackage query with automatic consumption
    pub async fn process_keypackage_query(
        &self,
        requester_pubkey: &str,
        filters: &[Filter],
    ) -> anyhow::Result<Vec<Event>> {
        if !Self::is_keypackage_query(filters) {
            return Ok(vec![]);
        }
        
        let authors = Self::extract_authors(filters);
        if authors.is_empty() {
            return Ok(vec![]);
        }
        
        info!("Processing KeyPackage query from {} for {} authors", 
              requester_pubkey, authors.len());
        
        let mut all_events = Vec::new();
        
        for author in authors {
            match self.query_and_consume_keypackages(
                &author,
                requester_pubkey,
                2, // Max 2 per author as per spec
            ).await {
                Ok(events) => {
                    info!("Returning {} KeyPackages from {} to {}", 
                          events.len(), author, requester_pubkey);
                    all_events.extend(events);
                }
                Err(e) => {
                    warn!("Failed to get KeyPackages from {}: {}", author, e);
                }
            }
        }
        
        Ok(all_events)
    }
    
    /// Query and consume KeyPackages for delivery
    pub async fn query_and_consume_keypackages(
        &self,
        owner_pubkey: &str,
        requester_pubkey: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<Event>> {
        let store = self.store()?;
        
        // TODO: Add rate limiting here
        
        // Get total count
        let total_count = store.count_user_keypackages(owner_pubkey).await?;
        
        if total_count == 0 {
            info!("No KeyPackages available for {}", owner_pubkey);
            return Ok(vec![]);
        }
        
        // Query KeyPackages (oldest first)
        let kps = store.query_keypackages(
            Some(&[owner_pubkey.to_string()]),
            None,
            Some(limit as u32),
            Some("created_at_asc"),
        ).await?;
        
        let mut events_to_return: Vec<Event> = Vec::new();
        let mut ids_to_consume: Vec<String> = Vec::new();
        
        // Get access to the event database - need to find a way to access it
        // For now, return empty as we need to refactor to pass the DB reference
        warn!("Need database access to retrieve actual events");
        
        // Update metrics for what we would have done
        if kps.len() > 0 {
            info!("Would return {} KeyPackages from {} to {}",
                  kps.len(), owner_pubkey, requester_pubkey);
        }
        
        Ok(events_to_return)
    }
}

/// Rate limiter for KeyPackage queries
pub struct KeyPackageRateLimiter {
    // TODO: Implement rate limiting
    // For now, this is a placeholder
}

impl KeyPackageRateLimiter {
    pub fn new() -> Self {
        Self {}
    }
    
    pub async fn check_rate_limit(&self, _requester: &str, _target: &str) -> bool {
        // TODO: Implement actual rate limiting
        // For now, always allow
        true
    }
}