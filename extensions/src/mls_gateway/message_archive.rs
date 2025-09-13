//! Message Archive System for Offline Delivery
//! 
//! This module provides message archival functionality to ensure users can retrieve
//! messages they missed while offline. When the Cloud Run service restarts frequently,
//! LMDB storage is ephemeral, so we need persistent storage for offline message delivery.
//!
//! Features:
//! - Archive Nostr events (kinds 445, 446) to Firestore
//! - Retrieve missed messages since a timestamp
//! - Automatic cleanup of expired messages
//! - Query by recipient pubkey for efficient delivery

use anyhow::Result;
use chrono::Utc;
use nostr_relay::db::Event;
use reqwest::Client as HttpClient;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::env;
use std::collections::HashSet;
use tracing::{debug, info, warn, instrument};

/// Archived event data structure for Firestore storage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchivedEvent {
    /// Nostr event ID
    pub id: String,
    /// Nostr event kind (445, 446, 1059)
    pub kind: u32,
    /// Event content
    pub content: String,
    /// Event tags
    pub tags: Vec<Vec<String>>,
    /// Event creation timestamp
    pub created_at: i64,
    /// Event author pubkey
    pub pubkey: String,
    /// Event signature
    pub sig: String,
    /// List of recipient pubkeys extracted from 'p' tags
    pub recipients: Vec<String>,
    /// Optional Nostr group id (from 'h' tag) for MLS group events
    pub group_id: Option<String>,
    /// Optional group epoch (from 'k' tag)
    pub group_epoch: Option<i64>,
    /// When this event was archived
    pub archived_at: i64,
    /// When this archived event expires
    pub expires_at: i64,
}

/// Message Archive client for Firestore operations
#[derive(Clone)]
pub struct MessageArchive {
    http_client: HttpClient,
    project_id: String,
    base_url: String,
}

impl MessageArchive {
    /// Create a new message archive instance
    pub async fn new() -> Result<Self> {
        let project_id = env::var("GOOGLE_CLOUD_PROJECT")
            .or_else(|_| env::var("GCP_PROJECT"))
            .unwrap_or_else(|_| "loxation-f8e1c".to_string());

        let http_client = HttpClient::new();
        let base_url = format!("https://firestore.googleapis.com/v1/projects/{}/databases/(default)/documents", project_id);

        info!("Message archive initialized for project: {}", project_id);
        Ok(Self {
            http_client,
            project_id,
            base_url,
        })
    }

    /// Get Google Cloud access token using metadata service (for Cloud Run)
    async fn get_access_token(&self) -> Result<String> {
        let metadata_url = "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token";
        
        let response = self.http_client
            .get(metadata_url)
            .header("Metadata-Flavor", "Google")
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!("Failed to get access token from metadata service"));
        }

        let token_response: Value = response.json().await?;
        let access_token = token_response
            .get("access_token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Invalid token response"))?;

        Ok(access_token.to_string())
    }

    /// Archive a Nostr event for offline delivery
    #[instrument(skip(self, event))]
    pub async fn archive_event(&self, event: &Event, ttl_days: Option<u32>) -> Result<()> {
        let now = Utc::now();
        let ttl_days = ttl_days.unwrap_or(7); // Default 7 days
        let expires_at = now + chrono::Duration::days(ttl_days as i64);
        
        // Extract recipient pubkeys from 'p' tags
        let recipients: Vec<String> = event.tags().iter()
            .filter(|tag| tag.len() >= 2 && tag[0] == "p")
            .map(|tag| tag[1].clone())
            .collect();

        // Extract group id and epoch from tags (for kind 445 MLS group messages or giftwrap scoped to a group)
        let group_id: Option<String> = event.tags().iter()
            .find(|tag| tag.len() >= 2 && tag[0] == "h")
            .map(|tag| tag[1].clone());

        let group_epoch: Option<i64> = event.tags().iter()
            .find(|tag| tag.len() >= 2 && tag[0] == "k")
            .and_then(|tag| tag[1].parse::<i64>().ok());

        // Skip archiving only if we have neither recipients nor group id.
        // This allows archiving 445 events keyed by group_id even when there are no 'p' recipients.
        if recipients.is_empty() && group_id.is_none() {
            debug!(
                "Skipping archive for event {} - no recipients and no group_id",
                hex::encode(event.id())
            );
            return Ok(());
        }

        let archived_event = ArchivedEvent {
            id: hex::encode(event.id()),
            kind: event.kind() as u32,
            content: event.content().to_string(),
            tags: event.tags().iter().map(|tag| {
                tag.iter().map(|s| s.to_string()).collect()
            }).collect(),
            created_at: event.created_at() as i64,
            pubkey: hex::encode(event.pubkey()),
            sig: hex::encode(event.sig()),
            recipients: recipients.clone(),
            group_id,
            group_epoch,
            archived_at: now.timestamp(),
            expires_at: expires_at.timestamp(),
        };

        // Store in Firestore
        let access_token = self.get_access_token().await?;
        let doc_id = format!("{}-{}", event.kind(), hex::encode(event.id()));
        let url = format!("{}/archived_events/{}", self.base_url, doc_id);
        
        let firestore_doc = self.to_firestore_document(&archived_event)?;
        
        let response = self.http_client
            .patch(&url)
            .header("Authorization", format!("Bearer {}", access_token))
            .header("Content-Type", "application/json")
            .json(&firestore_doc)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("Failed to archive event ({}): {}", status, error_text));
        }

        debug!("Archived event {} with {} recipients, expires at {}",
               hex::encode(event.id()), recipients.len(), expires_at);
        Ok(())
    }

    /// Get missed messages for a user since a timestamp
    #[instrument(skip(self))]
    pub async fn get_missed_messages(&self, pubkey: &str, since: i64, limit: u32) -> Result<Vec<Event>> {
        let access_token = self.get_access_token().await?;
        let now = Utc::now().timestamp();
        
        // Build Firestore structured query
        let query = json!({
            "structuredQuery": {
                "from": [{"collectionId": "archived_events"}],
                "where": {
                    "compositeFilter": {
                        "op": "AND",
                        "filters": [
                            {
                                "fieldFilter": {
                                    "field": {"fieldPath": "recipients"},
                                    "op": "ARRAY_CONTAINS",
                                    "value": {"stringValue": pubkey}
                                }
                            },
                            {
                                "fieldFilter": {
                                    "field": {"fieldPath": "created_at"},
                                    "op": "GREATER_THAN",
                                    "value": {"integerValue": since.to_string()}
                                }
                            },
                            {
                                "fieldFilter": {
                                    "field": {"fieldPath": "expires_at"},
                                    "op": "GREATER_THAN",
                                    "value": {"integerValue": now.to_string()}
                                }
                            }
                        ]
                    }
                },
                "orderBy": [{"field": {"fieldPath": "created_at"}, "direction": "ASCENDING"}],
                "limit": limit
            }
        });

        let url = format!("{}:runQuery", self.base_url);
        let response = self.http_client
            .post(&url)
            .header("Authorization", format!("Bearer {}", access_token))
            .header("Content-Type", "application/json")
            .json(&query)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("Failed to query missed messages ({}): {}", status, error_text));
        }

        let response_json: Value = response.json().await?;
        let mut events = Vec::new();

        if let Some(documents) = response_json.as_array() {
            for doc in documents {
                if let Some(document) = doc.get("document") {
                    if let Some(fields) = document.get("fields") {
                        match self.from_firestore_fields(fields) {
                            Ok(archived_event) => {
                                match self.archived_event_to_nostr_event(&archived_event) {
                                    Ok(event) => events.push(event),
                                    Err(e) => warn!("Failed to convert archived event to Nostr event: {}", e),
                                }
                            }
                            Err(e) => warn!("Failed to parse archived event: {}", e),
                        }
                    }
                }
            }
        }

        info!("Retrieved {} missed messages for pubkey {} since {}", events.len(), pubkey, since);
        Ok(events)
    }

    /// Get MLS group messages by group_id since a timestamp
    #[instrument(skip(self))]
    pub async fn get_group_messages(&self, group_id: &str, since: i64, limit: u32) -> Result<Vec<Event>> {
        let access_token = self.get_access_token().await?;
        let now = Utc::now().timestamp();

        // Build Firestore structured query for group-based retrieval
        let query = json!({
            "structuredQuery": {
                "from": [{"collectionId": "archived_events"}],
                "where": {
                    "compositeFilter": {
                        "op": "AND",
                        "filters": [
                            {
                                "fieldFilter": {
                                    "field": {"fieldPath": "group_id"},
                                    "op": "EQUAL",
                                    "value": {"stringValue": group_id}
                                }
                            },
                            {
                                "fieldFilter": {
                                    "field": {"fieldPath": "created_at"},
                                    "op": "GREATER_THAN",
                                    "value": {"integerValue": since.to_string()}
                                }
                            },
                            {
                                "fieldFilter": {
                                    "field": {"fieldPath": "expires_at"},
                                    "op": "GREATER_THAN",
                                    "value": {"integerValue": now.to_string()}
                                }
                            }
                        ]
                    }
                },
                "orderBy": [{"field": {"fieldPath": "created_at"}, "direction": "ASCENDING"}],
                "limit": limit
            }
        });

        let url = format!("{}:runQuery", self.base_url);
        let response = self.http_client
            .post(&url)
            .header("Authorization", format!("Bearer {}", access_token))
            .header("Content-Type", "application/json")
            .json(&query)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("Failed to query group messages ({}): {}", status, error_text));
        }

        let response_json: Value = response.json().await?;
        let mut events = Vec::new();

        if let Some(documents) = response_json.as_array() {
            for doc in documents {
                if let Some(document) = doc.get("document") {
                    if let Some(fields) = document.get("fields") {
                        match self.from_firestore_fields(fields) {
                            Ok(archived_event) => {
                                match self.archived_event_to_nostr_event(&archived_event) {
                                    Ok(event) => events.push(event),
                                    Err(e) => warn!("Failed to convert archived event to Nostr event: {}", e),
                                }
                            }
                            Err(e) => warn!("Failed to parse archived group event: {}", e),
                        }
                    }
                }
            }
        }

        info!("Retrieved {} group messages for group {} since {}", events.len(), group_id, since);
        Ok(events)
    }

    /// List recent archived events by kinds, ordered by created_at ASC, TTL-respecting
    /// This is used at relay startup to reconstitute LMDB so clients can use pure Nostr REQ.
    pub async fn list_recent_events_by_kinds(
        &self,
        kinds: &[u32],
        since: i64,
        total_limit: u32,
    ) -> Result<Vec<Event>> {
        let access_token = self.get_access_token().await?;
        let now = Utc::now().timestamp();

        let mut collected: Vec<Event> = Vec::new();
        let mut seen_ids: HashSet<String> = HashSet::new();

        for kind in kinds {
            if collected.len() as u32 >= total_limit {
                break;
            }
            // Limit per kind to avoid huge reads; Firestore hard-caps at 500 per page here.
            let per_kind_limit = (total_limit.saturating_sub(collected.len() as u32)).min(500);

            let query = json!({
                "structuredQuery": {
                    "from": [{"collectionId": "archived_events"}],
                    "where": {
                        "compositeFilter": {
                            "op": "AND",
                            "filters": [
                                {
                                    "fieldFilter": {
                                        "field": {"fieldPath": "kind"},
                                        "op": "EQUAL",
                                        "value": {"integerValue": kind.to_string()}
                                    }
                                },
                                {
                                    "fieldFilter": {
                                        "field": {"fieldPath": "created_at"},
                                        "op": "GREATER_THAN",
                                        "value": {"integerValue": since.to_string()}
                                    }
                                },
                                {
                                    "fieldFilter": {
                                        "field": {"fieldPath": "expires_at"},
                                        "op": "GREATER_THAN",
                                        "value": {"integerValue": now.to_string()}
                                    }
                                }
                            ]
                        }
                    },
                    "orderBy": [{"field": {"fieldPath": "created_at"}, "direction": "ASCENDING"}],
                    "limit": per_kind_limit
                }
            });

            let url = format!("{}:runQuery", self.base_url);
            let response = self.http_client
                .post(&url)
                .header("Authorization", format!("Bearer {}", access_token))
                .header("Content-Type", "application/json")
                .json(&query)
                .send()
                .await?;

            if !response.status().is_success() {
                let status = response.status();
                let error_text = response.text().await.unwrap_or_default();
                return Err(anyhow::anyhow!("Failed to query recent events ({}): {}", status, error_text));
            }

            let response_json: Value = response.json().await?;
            if let Some(documents) = response_json.as_array() {
                for doc in documents {
                    if let Some(document) = doc.get("document") {
                        if let Some(fields) = document.get("fields") {
                            if let Ok(archived_event) = self.from_firestore_fields(fields) {
                                if seen_ids.insert(archived_event.id.clone()) {
                                    if let Ok(event) = self.archived_event_to_nostr_event(&archived_event) {
                                        collected.push(event);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        collected.sort_by_key(|e| e.created_at() as i64);
        Ok(collected)
    }

    /// Clean up expired archived events
    #[instrument(skip(self))]
    pub async fn cleanup_expired(&self) -> Result<u64> {
        let access_token = self.get_access_token().await?;
        let now = Utc::now().timestamp();
        
        // Query for expired documents
        let query = json!({
            "structuredQuery": {
                "from": [{"collectionId": "archived_events"}],
                "where": {
                    "fieldFilter": {
                        "field": {"fieldPath": "expires_at"},
                        "op": "LESS_THAN",
                        "value": {"integerValue": now.to_string()}
                    }
                },
                "limit": 100
            }
        });

        let url = format!("{}:runQuery", self.base_url);
        let response = self.http_client
            .post(&url)
            .header("Authorization", format!("Bearer {}", access_token))
            .header("Content-Type", "application/json")
            .json(&query)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("Failed to query expired events ({}): {}", status, error_text));
        }

        let response_json: Value = response.json().await?;
        let mut deleted_count = 0;

        if let Some(documents) = response_json.as_array() {
            for doc in documents {
                if let Some(document) = doc.get("document") {
                    if let Some(name) = document.get("name").and_then(|v| v.as_str()) {
                        let delete_response = self.http_client
                            .delete(&format!("https://firestore.googleapis.com/v1/{}", name))
                            .header("Authorization", format!("Bearer {}", access_token))
                            .send()
                            .await?;

                        if delete_response.status().is_success() {
                            deleted_count += 1;
                        } else {
                            warn!("Failed to delete expired archived event: {}", name);
                        }
                    }
                }
            }
        }

        if deleted_count > 0 {
            info!("Cleaned up {} expired archived events", deleted_count);
        }

        Ok(deleted_count)
    }

    /// Convert archived event back to Nostr event
    fn archived_event_to_nostr_event(&self, archived: &ArchivedEvent) -> Result<Event> {
        let event_json = json!({
            "id": archived.id,
            "kind": archived.kind,
            "content": archived.content,
            "tags": archived.tags,
            "created_at": archived.created_at,
            "pubkey": archived.pubkey,
            "sig": archived.sig
        });

        let event: Event = serde_json::from_value(event_json)?;
        Ok(event)
    }

    /// Convert ArchivedEvent to Firestore document format
    fn to_firestore_document(&self, event: &ArchivedEvent) -> Result<Value> {
        // Base fields
        let mut doc = json!({
            "fields": {
                "id": {"stringValue": event.id},
                "kind": {"integerValue": event.kind.to_string()},
                "content": {"stringValue": event.content},
                "tags": {
                    "arrayValue": {
                        "values": event.tags.iter().map(|tag| {
                            json!({
                                "arrayValue": {
                                    "values": tag.iter().map(|s| json!({"stringValue": s})).collect::<Vec<_>>()
                                }
                            })
                        }).collect::<Vec<_>>()
                    }
                },
                "created_at": {"integerValue": event.created_at.to_string()},
                "pubkey": {"stringValue": event.pubkey},
                "sig": {"stringValue": event.sig},
                "recipients": {
                    "arrayValue": {
                        "values": event.recipients.iter().map(|r| json!({"stringValue": r})).collect::<Vec<_>>()
                    }
                },
                "archived_at": {"integerValue": event.archived_at.to_string()},
                "expires_at": {"integerValue": event.expires_at.to_string()}
            }
        });

        // Optionally include group_id and group_epoch for MLS group catch-up
        if let Some(ref gid) = event.group_id {
            doc["fields"]["group_id"] = json!({"stringValue": gid});
        }
        if let Some(epoch) = event.group_epoch {
            doc["fields"]["group_epoch"] = json!({"integerValue": epoch.to_string()});
        }

        Ok(doc)
    }

    /// Convert Firestore fields to ArchivedEvent
    fn from_firestore_fields(&self, fields: &Value) -> Result<ArchivedEvent> {
        let get_string = |field: &str| -> Result<String> {
            fields.get(field)
                .and_then(|v| v.get("stringValue"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .ok_or_else(|| anyhow::anyhow!("Missing string field: {}", field))
        };

        let get_int = |field: &str| -> Result<i64> {
            fields.get(field)
                .and_then(|v| v.get("integerValue"))
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| anyhow::anyhow!("Missing integer field: {}", field))
        };

        let get_string_array = |field: &str| -> Result<Vec<String>> {
            let array = fields.get(field)
                .and_then(|v| v.get("arrayValue"))
                .and_then(|v| v.get("values"))
                .and_then(|v| v.as_array())
                .ok_or_else(|| anyhow::anyhow!("Missing array field: {}", field))?;

            let mut result = Vec::new();
            for item in array {
                if let Some(s) = item.get("stringValue").and_then(|v| v.as_str()) {
                    result.push(s.to_string());
                }
            }
            Ok(result)
        };

        // Parse tags (array of arrays)
        let tags = if let Some(tags_value) = fields.get("tags").and_then(|v| v.get("arrayValue")).and_then(|v| v.get("values")) {
            let mut result = Vec::new();
            if let Some(tags_array) = tags_value.as_array() {
                for tag_value in tags_array {
                    if let Some(tag_array) = tag_value.get("arrayValue").and_then(|v| v.get("values")).and_then(|v| v.as_array()) {
                        let mut tag = Vec::new();
                        for item in tag_array {
                            if let Some(s) = item.get("stringValue").and_then(|v| v.as_str()) {
                                tag.push(s.to_string());
                            }
                        }
                        result.push(tag);
                    }
                }
            }
            result
        } else {
            Vec::new()
        };

        // Optional fields for group catch-up
        let group_id = fields.get("group_id")
            .and_then(|v| v.get("stringValue"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let group_epoch = fields.get("group_epoch")
            .and_then(|v| v.get("integerValue"))
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<i64>().ok());

        Ok(ArchivedEvent {
            id: get_string("id")?,
            kind: get_int("kind")? as u32,
            content: get_string("content")?,
            tags,
            created_at: get_int("created_at")?,
            pubkey: get_string("pubkey")?,
            sig: get_string("sig")?,
            recipients: get_string_array("recipients")?,
            group_id,
            group_epoch,
            archived_at: get_int("archived_at")?,
            expires_at: get_int("expires_at")?,
        })
    }
}
