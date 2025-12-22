#![allow(dead_code)]
//! Service Member adapter for MLS-first NIP-SERVICE path.
//!
//! This module is compiled only when the `nip_service_mls` feature is enabled.
//! It owns the RN MLS client (loxation_mls_rust::api::MlsClient) and
//! provides decrypt/encrypt helpers for MLS application messages.
//!
//! This module handles:
//! - Decrypting MLS application messages (kind 445) that contain NIP-SERVICE payloads
//! - Validating membership in MLS groups
//! - Processing decrypted NIP-SERVICE service-request JSON payloads

use nostr_relay::db::Event;
use serde_json::Value as JsonValue;
use tracing::{info, warn, error};
use std::sync::OnceLock;
use loxation_mls_rust::api::{MlsClient, Result as MLSResult, KeyPackage};

/// Global MLS client instance
static MLS_CLIENT: OnceLock<MlsClient> = OnceLock::new();

/// Get or initialize the global MLS client
pub fn get_mls_client() -> &'static MlsClient {
    MLS_CLIENT.get_or_init(|| {
        info!("Initializing MLS client for service member operations");
        MlsClient::new()
    })
}

/// Initialize MLS client with storage configuration
pub fn initialize_mls_client(storage_path: Option<&str>, user_id: &str, encryption_key: &str) -> MLSResult<()> {
    let client = get_mls_client();

    // Set storage path if provided
    if let Some(path) = storage_path {
        client.set_storage_path(path)?;
    }

    // Set storage encryption key for the service user
    client.set_storage_key(user_id, encryption_key)?;

    info!("MLS client initialized for service member with user_id: {}", user_id);
    Ok(())
}

/// Check if a user is a member of an MLS group
///
/// # Arguments
/// * `user_id` - The user ID to check
/// * `group_id` - The group ID to check membership in
///
/// # Returns
/// true if the user is a member of the group, false otherwise
pub fn has_group(user_id: &str, group_id: &str) -> bool {
    let client = get_mls_client();

    match client.group_members(group_id, user_id) {
        Ok(members) => {
            // Check if user_id is in the members list
            members.contains(&user_id.to_string())
        }
        Err(e) => {
            warn!("Failed to check group membership for user {} in group {}: {}", user_id, group_id, e);
            false
        }
    }
}

/// Create a new MLS group for a client
///
/// # Arguments
/// * `group_id` - Unique identifier for the group
/// * `creator_id` - Identifier of the group creator
///
/// # Returns
/// The group handle ID
pub fn create_client_group(group_id: &str, creator_id: &str) -> Result<usize, String> {
    let client = get_mls_client();

    match client.create_group(group_id, creator_id) {
        Ok(handle_id) => {
            info!("Created MLS group {} for client {}", group_id, creator_id);
            Ok(handle_id)
        }
        Err(e) => {
            error!("Failed to create MLS group {} for client {}: {}", group_id, creator_id, e);
            Err(format!("Failed to create group: {}", e))
        }
    }
}

/// Add a member to an MLS group
///
/// # Arguments
/// * `group_id` - The group to add the member to
/// * `creator_id` - The group creator/member performing the action
/// * `new_member_id` - The new member to add
/// * `key_package_data` - The key package bytes for the new member
///
/// # Returns
/// Result indicating success or failure
pub fn add_group_member(
    group_id: &str,
    creator_id: &str,
    new_member_id: &str,
    key_package_data: &[u8]
) -> Result<(), String> {
    let client = get_mls_client();

    // Create a KeyPackage from the provided data
    let key_package = KeyPackage::from_bytes(key_package_data.to_vec());

    match client.add_member(group_id, creator_id, new_member_id, &key_package) {
        Ok(_result) => {
            info!("Added member {} to group {} by {}", new_member_id, group_id, creator_id);
            Ok(())
        }
        Err(e) => {
            error!("Failed to add member {} to group {}: {}", new_member_id, group_id, e);
            Err(format!("Failed to add member: {}", e))
        }
    }
}

/// Encrypt a NIP-SERVICE payload for a specific group
///
/// # Arguments
/// * `group_id` - The target group
/// * `sender_id` - The sender of the message
/// * `payload` - The JSON payload to encrypt
///
/// # Returns
/// The encrypted message bytes
pub fn encrypt_service_payload(
    group_id: &str,
    sender_id: &str,
    payload: JsonValue
) -> Result<Vec<u8>, String> {
    let client = get_mls_client();

    // Serialize the JSON payload to string
    let payload_str = match serde_json::to_string(&payload) {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to serialize payload to JSON: {}", e);
            return Err(format!("JSON serialization failed: {}", e));
        }
    };

    // Use the MLS client to encrypt the message
    match client.encrypt_message(group_id, sender_id, &payload_str) {
        Ok(encrypted_bytes) => {
            info!("Encrypted service payload for group {} by {}", group_id, sender_id);
            Ok(encrypted_bytes)
        }
        Err(e) => {
            error!("Failed to encrypt service payload for group {}: {}", group_id, e);
            Err(format!("Encryption failed: {}", e))
        }
    }
}

/// Get the current epoch for a group
///
/// # Arguments
/// * `group_id` - The group ID
/// * `user_id` - The user ID
///
/// # Returns
/// The current epoch number
pub fn get_group_epoch(group_id: &str, user_id: &str) -> Result<u64, String> {
    let client = get_mls_client();

    match client.get_current_epoch(group_id, user_id) {
        Ok(epoch) => Ok(epoch),
        Err(e) => {
            error!("Failed to get epoch for group {}: {}", group_id, e);
            Err(format!("Failed to get epoch: {}", e))
        }
    }
}

/// Attempt to decrypt an incoming MLS group message (kind 445) into a NIP-SERVICE JSON payload.
///
/// This function:
/// 1. Extracts group_id from event tags (["h", group_id])
/// 2. Uses the MLS client to decrypt the message content
/// 3. Validates that the decrypted content is a NIP-SERVICE service-request
/// 4. Returns the parsed JSON payload if valid
///
/// Returns Some(JsonValue) if the payload is a valid NIP-SERVICE service-request; otherwise None.
///
/// NEVER logs plaintext payloads; only logs non-sensitive summary for observability.
pub async fn try_decrypt_service_request(event: &Event) -> Option<JsonValue> {
    if event.kind() != 445 {
        return None;
    }

    // Extract group_id from event tags
    let group_id = match event.tags()
        .iter()
        .find(|tag| tag.len() >= 2 && tag[0] == "h")
        .map(|tag| tag[1].as_str()) {
        Some(id) => id,
        None => {
            warn!("MLS message (kind 445) missing group_id tag");
            return None;
        }
    };

    // Get the raw encrypted content
    let encrypted_content = event.content().as_bytes();

    if encrypted_content.is_empty() {
        warn!("Empty content in MLS message");
        return None;
    }

    let client = get_mls_client();

    // Try to decrypt the message using MLS
    // Note: We need to determine the user_id for decryption. For service operations,
    // this could be a service account ID. For now, we'll use a default service user.
    let service_user_id = "nip_service";

    match client.decrypt_message(group_id, service_user_id, encrypted_content) {
        Ok(decrypted_json) => {
            // Parse the decrypted content as JSON
            match serde_json::from_str::<JsonValue>(&decrypted_json) {
                Ok(json_payload) => {
                    // Validate that this looks like a NIP-SERVICE service-request
                    let action_type = json_payload.get("action_type").and_then(|x| x.as_str());
                    let action_id = json_payload.get("action_id").and_then(|x| x.as_str());
                    let client_id = json_payload.get("client_id").and_then(|x| x.as_str());
                    let profile = json_payload.get("profile").and_then(|x| x.as_str());

                    // Check for required NIP-SERVICE fields
                    let is_valid_service_request =
                        action_type.is_some() && action_id.is_some() &&
                        client_id.is_some() && profile.is_some();

                    if is_valid_service_request {
                        info!(
                            target: "nip_service",
                            "try_decrypt_service_request: successfully decrypted MLS message (profile={:?}, action_type={:?}, group_id={})",
                            profile, action_type, group_id
                        );
                        Some(json_payload)
                    } else {
                        warn!(
                            target: "nip_service",
                            "try_decrypt_service_request: decrypted content missing required NIP-SERVICE fields (action_type={:?}, action_id={:?}, client_id={:?}, profile={:?})",
                            action_type, action_id, client_id, profile
                        );
                        None
                    }
                }
                Err(e) => {
                    warn!(
                        target: "nip_service",
                        "try_decrypt_service_request: decrypted content is not valid JSON: {}",
                        e
                    );
                    None
                }
            }
        }
        Err(e) => {
            // This could be:
            // - Not an MLS message (normal Nostr content)
            // - Not a member of the group
            // - Corrupted ciphertext
            // - Other decryption failures
            // We log at debug level since this is expected for non-MLS messages
            warn!(
                target: "nip_service",
                "try_decrypt_service_request: failed to decrypt MLS message for group {}: {}",
                group_id, e
            );
            None
        }
    }
}
