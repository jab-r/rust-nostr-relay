//! NIP-SERVICE storage scaffolding for NIP-KR (Rotation) flows.
//!
//! This module provides a minimal, compilable storage abstraction with an
//! in-memory implementation to unblock wiring. A Firestore-backed version can
//! be added later following the same trait.
//!
//! Policy: Do NOT store plaintext secrets. Only hashes and metadata.

use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretState {
    Pending,
    Current,
    Grace,
    Retired,
}

#[derive(Debug, Clone)]
pub struct SecretVersionRecord {
    pub client_id: String,
    pub version_id: String,
    pub secret_hash: String,
    pub mac_key_ref: String,
    pub not_before_ms: i64,
    pub not_after_ms: Option<i64>,
    pub state: SecretState,
    pub rotated_by: Option<String>,
    pub rotation_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RotationOutcome {
    None,
    Promoted,
    Canceled,
    Expired,
    RolledBack,
}

#[derive(Debug, Clone)]
pub struct RotationRecord {
    pub action_id: String, // rotation_id
    pub client_id: String,
    pub new_version: String,
    pub old_version: Option<String>,
    pub not_before_ms: i64,
    pub grace_until_ms: Option<i64>,
    pub quorum_required: u32,
    pub quorum_acks: u32,
    pub outcome: RotationOutcome,
}

#[async_trait]
pub trait NipKrStore: Send + Sync + 'static {
    /// Prepare rotation: write version record as pending and rotation audit entry.
    async fn prepare_rotation(
        &self,
        client_id: &str,
        version_id: &str,
        secret_hash: &str,
        mac_key_ref: &str,
        not_before_ms: i64,
        grace_duration_ms: Option<i64>,
        rotation_id: &str,
        rotation_reason: Option<&str>,
        quorum_required: u32,
    ) -> Result<()>;

    /// Promote rotation: atomically set current_version=new and old to grace (skeleton).
    async fn promote_rotation(
        &self,
        client_id: &str,
        rotation_id: &str,
    ) -> Result<()>;

    /// Record an ack (increments quorum_acks).
    async fn record_ack(&self, rotation_id: &str) -> Result<()>;
}

// ---------------- In-memory store (dev only) ----------------

#[derive(Default)]
struct InMemoryInner {
    // Keyed by (client_id, version_id)
    versions: HashMap<(String, String), SecretVersionRecord>,
    // Keyed by rotation_id
    rotations: HashMap<String, RotationRecord>,
    // Current pointer per client
    current_version: HashMap<String, String>,
    // Previous pointer per client
    previous_version: HashMap<String, String>,
}

pub struct InMemoryStore {
    inner: Mutex<InMemoryInner>,
}

impl InMemoryStore {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(InMemoryInner::default()),
        }
    }
}

static GLOBAL_STORE: OnceLock<InMemoryStore> = OnceLock::new();

/// Get a global in-memory store (dev-only; replace with Firestore in prod).
pub fn get_global_store() -> &'static InMemoryStore {
    GLOBAL_STORE.get_or_init(InMemoryStore::new)
}

#[async_trait]
impl NipKrStore for InMemoryStore {
    async fn prepare_rotation(
        &self,
        client_id: &str,
        version_id: &str,
        secret_hash: &str,
        mac_key_ref: &str,
        not_before_ms: i64,
        grace_duration_ms: Option<i64>,
        rotation_id: &str,
        rotation_reason: Option<&str>,
        quorum_required: u32,
    ) -> Result<()> {
        let mut g = self.inner.lock().unwrap();

        // Create pending version record
        let rec = SecretVersionRecord {
            client_id: client_id.to_string(),
            version_id: version_id.to_string(),
            secret_hash: secret_hash.to_string(),
            mac_key_ref: mac_key_ref.to_string(),
            not_before_ms,
            not_after_ms: grace_duration_ms.map(|gms| not_before_ms + gms),
            state: SecretState::Pending,
            rotated_by: None,
            rotation_reason: rotation_reason.map(|s| s.to_string()),
        };
        g.versions
            .insert((client_id.to_string(), version_id.to_string()), rec);

        // Create rotation audit entry
        let rot = RotationRecord {
            action_id: rotation_id.to_string(),
            client_id: client_id.to_string(),
            new_version: version_id.to_string(),
            old_version: g.current_version.get(client_id).cloned(),
            not_before_ms,
            grace_until_ms: grace_duration_ms.map(|gms| not_before_ms + gms),
            quorum_required,
            quorum_acks: 0,
            outcome: RotationOutcome::None,
        };
        g.rotations.insert(rotation_id.to_string(), rot);

        Ok(())
    }

    async fn promote_rotation(&self, client_id: &str, rotation_id: &str) -> Result<()> {
        let mut g = self.inner.lock().unwrap();

        // First, read the new_version without holding a mutable borrow across further ops
        let new_version = match g.rotations.get(rotation_id) {
            Some(r) => r.new_version.clone(),
            None => return Ok(()), // no-op
        };

        // Move current -> previous, and set previous state to Grace
        if let Some(cur) = g.current_version.get(client_id).cloned() {
            g.previous_version.insert(client_id.to_string(), cur.clone());
            if let Some(prev_rec) = g
                .versions
                .get_mut(&(client_id.to_string(), cur.clone()))
            {
                prev_rec.state = SecretState::Grace;
            }
        }

        // Set new current
        g.current_version
            .insert(client_id.to_string(), new_version.clone());
        if let Some(new_rec) = g
            .versions
            .get_mut(&(client_id.to_string(), new_version.clone()))
        {
            new_rec.state = SecretState::Current;
        }

        // Finally, update the rotation outcome in a separate mutable borrow
        if let Some(rot) = g.rotations.get_mut(rotation_id) {
            rot.outcome = RotationOutcome::Promoted;
        }

        Ok(())
    }

    async fn record_ack(&self, rotation_id: &str) -> Result<()> {
        let mut g = self.inner.lock().unwrap();
        if let Some(rot) = g.rotations.get_mut(rotation_id) {
            rot.quorum_acks = rot.quorum_acks.saturating_add(1);
        }
        Ok(())
    }
}
