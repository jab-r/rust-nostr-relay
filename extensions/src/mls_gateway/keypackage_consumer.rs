//! KeyPackage automatic consumption on REQ queries
//!
//! This module implements automatic consumption of KeyPackages when they are
//! queried via standard REQ messages. No special kind 447 requests are needed.

use crate::mls_gateway::StorageBackend;
use nostr_relay::db::{Event, Filter};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use chrono::{DateTime, Utc};
use tracing::{info, error};
use metrics::counter;

/// Tracks which events have been delivered to which requesters
/// This helps us consume KeyPackages after they've been sent
#[derive(Debug, Clone)]
pub struct ConsumptionTracker {
    /// Map from event_id to list of requesters who received it
    delivered: Arc<RwLock<HashMap<String, Vec<DeliveryRecord>>>>,
}

#[derive(Debug, Clone)]
struct DeliveryRecord {
    requester_pubkey: String,
    delivered_at: DateTime<Utc>,
}

impl ConsumptionTracker {
    pub fn new() -> Self {
        Self {
            delivered: Arc::new(RwLock::new(HashMap::new())),
        }
    }
    
    /// Record that an event was delivered to a requester
    pub async fn record_delivery(
        &self,
        event_id: &str,
        requester_pubkey: &str,
    ) {
        let mut delivered = self.delivered.write().await;
        let record = DeliveryRecord {
            requester_pubkey: requester_pubkey.to_string(),
            delivered_at: Utc::now(),
        };
        
        delivered
            .entry(event_id.to_string())
            .or_insert_with(Vec::new)
            .push(record);
    }
    
    /// Get all event IDs that were delivered to a requester
    pub async fn get_delivered_to(&self, requester_pubkey: &str) -> Vec<String> {
        let delivered = self.delivered.read().await;
        let mut event_ids = Vec::new();
        
        for (event_id, records) in delivered.iter() {
            if records.iter().any(|r| r.requester_pubkey == requester_pubkey) {
                event_ids.push(event_id.clone());
            }
        }
        
        event_ids
    }
}

/// Rate limiter for KeyPackage queries
#[derive(Debug, Clone)]
pub struct KeyPackageRateLimiter {
    /// Map from (requester, author) to query timestamps
    queries: Arc<RwLock<HashMap<(String, String), Vec<DateTime<Utc>>>>>,
    /// Max queries per hour per requester-author pair
    max_queries_per_hour: u32,
    /// Max KeyPackages per query
    max_keypackages_per_query: u32,
}

impl KeyPackageRateLimiter {
    pub fn new() -> Self {
        Self {
            queries: Arc::new(RwLock::new(HashMap::new())),
            max_queries_per_hour: 10,
            max_keypackages_per_query: 2,
        }
    }
    
    /// Check if a query is allowed
    pub async fn check_rate_limit(
        &self,
        requester: &str,
        author: &str,
    ) -> Result<bool, String> {
        let now = Utc::now();
        let hour_ago = now - chrono::Duration::hours(1);
        
        let mut queries = self.queries.write().await;
        let key = (requester.to_string(), author.to_string());
        
        // Get or create query list
        let query_list = queries.entry(key).or_insert_with(Vec::new);
        
        // Remove old queries
        query_list.retain(|&t| t > hour_ago);
        
        // Check limit
        if query_list.len() >= self.max_queries_per_hour as usize {
            counter!("mls_gateway_rate_limit_exceeded", 
                     "requester" => requester.to_string(),
                     "author" => author.to_string())
                .increment(1);
                
            let minutes_until_reset = 60 - query_list[0].signed_duration_since(hour_ago).num_minutes();
            return Err(format!(
                "Rate limit exceeded. Try again in {} minutes.", 
                minutes_until_reset
            ));
        }
        
        // Record this query
        query_list.push(now);
        Ok(true)
    }
}

/// Helper to check if a filter is querying for KeyPackages
pub fn is_keypackage_query(filter: &Filter) -> bool {
    filter.kinds.iter().any(|&k| k == 443)
}

/// Extract authors from a KeyPackage query filter
pub fn extract_keypackage_authors(filter: &Filter) -> Vec<String> {
    if !is_keypackage_query(filter) {
        return vec![];
    }
    
    filter.authors.iter()
        .map(|author| hex::encode(author))
        .collect()
}

/// Process KeyPackage query results for consumption
pub async fn process_keypackage_delivery(
    storage: &StorageBackend,
    events: &[Event],
    requester_pubkey: &str,
    author_pubkey: &str,
) -> anyhow::Result<()> {
    // Only process KeyPackage events
    let keypackage_events: Vec<_> = events.iter()
        .filter(|e| e.kind() == 443)
        .collect();
    
    if keypackage_events.is_empty() {
        return Ok(());
    }
    
    info!("Processing delivery of {} KeyPackages from {} to {}",
          keypackage_events.len(), author_pubkey, requester_pubkey);
    
    // Get total count for this author
    let total_count = storage.count_user_keypackages(author_pubkey).await?;
    
    // Determine which KeyPackages to consume
    let mut to_consume = Vec::new();
    for (idx, event) in keypackage_events.iter().enumerate() {
        // Never consume the last KeyPackage
        let would_be_last = (total_count as usize) - to_consume.len() <= 1;
        
        if !would_be_last {
            to_consume.push(event.id_str());
            info!("Marking KeyPackage {} for consumption", event.id_str());
        } else {
            info!("Preserving last KeyPackage {} for {}", event.id_str(), author_pubkey);
        }
    }
    
    // Consume the KeyPackages
    for event_id in &to_consume {
        match storage.delete_consumed_keypackage(event_id).await {
            Ok(deleted) => {
                if deleted {
                    info!("Consumed KeyPackage {} after delivery to {}", event_id, requester_pubkey);
                    counter!("mls_gateway_keypackages_consumed",
                             "owner" => author_pubkey.to_string())
                        .increment(1);
                }
            }
            Err(e) => {
                error!("Failed to consume KeyPackage {}: {}", event_id, e);
            }
        }
    }
    
    // Update delivery metrics
    counter!("mls_gateway_keypackages_served",
             "requester" => requester_pubkey.to_string(),
             "owner" => author_pubkey.to_string())
        .increment(keypackage_events.len() as u64);
    
    info!("KeyPackage delivery complete: {} delivered, {} consumed, {} remaining",
          keypackage_events.len(),
          to_consume.len(),
          total_count - to_consume.len() as u32);
    
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr_relay::db::SortList;
    
    #[tokio::test]
    async fn test_rate_limiter() {
        let limiter = KeyPackageRateLimiter::new();
        
        // First 10 queries should be allowed
        for i in 0..10 {
            let allowed = limiter.check_rate_limit("alice", "bob").await;
            assert!(allowed.is_ok(), "Query {} should be allowed", i);
        }
        
        // 11th query should be rate limited
        let allowed = limiter.check_rate_limit("alice", "bob").await;
        assert!(allowed.is_err(), "11th query should be rate limited");
        
        // Different author should still be allowed
        let allowed = limiter.check_rate_limit("alice", "carol").await;
        assert!(allowed.is_ok(), "Different author should have separate limit");
    }
    
    #[test]
    fn test_keypackage_filter_detection() {
        // Filter with kind 443 should be detected
        let mut filter = Filter::default();
        filter.kinds = SortList::from(vec![443]);
        assert!(is_keypackage_query(&filter));
        
        // Filter without kinds should not be detected
        let filter = Filter::default();
        assert!(!is_keypackage_query(&filter));
        
        // Filter with different kind should not be detected
        let mut filter = Filter::default();
        filter.kinds = SortList::from(vec![1, 2, 3]);
        assert!(!is_keypackage_query(&filter));
    }
}