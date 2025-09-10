# Implementing KMS MACVerify for OAuth2/X-API Client Secret Validation in this Project

Goal
Migrate from bcrypt-hashed client secrets to KMS-backed HMAC (MACSign/MACVerify) for versioned OAuth2 client_secrets, enabling secure rotation with zero plaintext at rest and shared verification between relay and server.

Current state (this repo)
- File: src/services/oauth2Service.ts
  - Uses bcrypt to hash secrets at registration and bcrypt.compare for validation.
  - No versioned secret pointers; no KMS integration.
  - Token payloads do not carry client_version_id.
- Firestore
  - oauth2_clients doc stores a single bcrypt hash per client.
  - No subcollection for versioned secrets or rotation audit trails.

Target state (phased)
- Versioned secrets stored as KMS HMACs (not plaintext or bcrypt) with metadata (algo, mac_key_ref, version_id, state).
- loxation-server validates client secrets using KMS MACVerify based on canonical input encoded as length‑prefixed fields: len(client_id)||client_id||len(version_id)||version_id||len(presented_secret)||presented_secret (32‑bit big‑endian lengths; UTF‑8; no Unicode normalization).
- Dual-secret acceptance during grace (current + previous).
- Optional: immediate revoke requires token version stamping (client_version_id) and revocation of retired versions.

Phased implementation plan

Phase 0 — KMS and IAM setup (GCP assumed due to Firestore/Firebase)
- Create HMAC-SHA-256 key in Cloud KMS:
  - Key purpose: MAC
  - Algorithm: HMAC_SHA256
  - Key ring: kr-oauth-rotation
  - Key name: kr-mac
- Service accounts and IAM:
  - loxation-server SA: roles/cloudkms.signerVerifier (MAC use) on the MAC key (MACVerify required; MACSign optional if server signs test vectors)
  - rust-nostr-relay SA: roles/cloudkms.signerVerifier on the MAC key (MACSign needed to compute secret_hash; MACVerify optional)
  - Deny any get/export of key material (non-exportable; Cloud KMS guarantees this)
- Record the KMS resource name:
  - projects/{PROJECT_ID}/locations/{LOCATION}/keyRings/kr-oauth-rotation/cryptoKeys/kr-mac
- Decide pepper_version label (e.g., kr-mac-v1) and rotate via KMS key versions when needed.

Phase 1 — Firestore schema (additive, non-breaking)
- New structure to support versioned secrets:
  - oauth2_clients/{clientId}
    - current_version: string
    - previous_version: string | null
    - updated_at: timestamp
    - status: "active" | "suspended" | "revoked"
  - oauth2_clients/{clientId}/secrets/{versionId}
    - secret_hash: base64url_no_padding(HMAC_SHA256(kms, length_prefixed(client_id, version_id, secret)))
    - algo: "HMAC-SHA-256"
    - mac_key_ref: "projects/.../cryptoKeyVersions/..." (exact KMS key version reference or logical label)
    - created_at: timestamp
    - not_before: timestamp
    - not_after: timestamp | null
    - state: "pending" | "current" | "grace" | "retired"
    - rotated_by: userId
    - rotation_reason: string
  - oauth2_rotations/{rotationId} (audit)
    - client_id, requested_by, new_version, old_version, not_before, grace_until, distribution_message_id, completed_at
- Keep legacy fields for compatibility:
  - Existing bcrypt-hashed clientSecret in oauth2_clients doc stays for legacy clients until rotated.

Phase 2 — Node KMS integration (new service)
Create a reusable KMS MAC service.

File: src/services/kmsMacService.ts (new)
```ts
import { KeyManagementServiceClient } from '@google-cloud/kms';

export interface MacConfig {
  keyResourceName: string;    // projects/.../cryptoKeys/kr-mac
  keyVersion?: string;        // optional: projects/.../cryptoKeyVersions/#
}

export class KmsMacService {
  private static instance: KmsMacService;
  private client: KeyManagementServiceClient;
  private keyResourceName: string;

  private constructor(cfg: MacConfig) {
    this.client = new KeyManagementServiceClient();
    this.keyResourceName = cfg.keyVersion ?? cfg.keyResourceName;
  }

  static init(cfg: MacConfig) {
    KmsMacService.instance = new KmsMacService(cfg);
    return KmsMacService.instance;
  }

  static getInstance() {
    if (!KmsMacService.instance) {
      throw new Error('KmsMacService not initialized');
    }
    return KmsMacService.instance;
  }

  // Canonical input builder (UTF-8, no Unicode normalization), length-prefixed fields
  static buildMacInput(clientId: string, versionId: string, secret: string): Buffer {
    const parts = [clientId, versionId, secret].map(s => Buffer.from(s, 'utf8'));
    const lenBufs = parts.map(b => {
      const lb = Buffer.alloc(4);
      lb.writeUInt32BE(b.length, 0);
      return lb;
    });
    return Buffer.concat([lenBufs[0], parts[0], lenBufs[1], parts[1], lenBufs[2], parts[2]]);
  }

  async macSign(data: Buffer): Promise<string> {
    const [resp] = await this.client.macSign({ name: this.keyResourceName, data });
    if (!resp.mac) throw new Error('KMS macSign returned no mac');
    return resp.mac.toString('base64url');
  }

  async macVerify(data: Buffer, macBase64Url: string): Promise<boolean> {
    const [resp] = await this.client.macVerify({
      name: this.keyResourceName,
      data,
      mac: Buffer.from(macBase64Url, 'base64url'),
    });
    return resp.success ?? false;
  }
}
```

Environment/config (set in your deployment):
- KMS_MAC_KEY: projects/{PROJECT}/locations/{LOC}/keyRings/kr-oauth-rotation/cryptoKeys/kr-mac
- Optional KMS_MAC_KEY_VERSION: projects/{PROJECT}/locations/{LOC}/keyRings/kr-oauth-rotation/cryptoKeys/kr-mac/cryptoKeyVersions/{N}
- MAC_KEY_REF: kr-mac-v1

Phase 3 — Update OAuth2Service to support dual validation path
- Keep legacy bcrypt for existing single-hash records.
- Add version-aware KMS validation for rotated clients.

Pseudocode changes (do not apply yet; for reference):
```ts
// During app bootstrap
KmsMacService.init({
  keyResourceName: process.env.KMS_MAC_KEY!,
  keyVersion: process.env.KMS_MAC_KEY_VERSION, // optional pin
});

// In validateClientCredentials(clientId, presentedSecret)
const clientDoc = await db.collection('oauth2_clients').doc(clientId).get();
if (!clientDoc.exists) return null;
const client = clientDoc.data() as OAuth2Client;

// Legacy path fallback:
if (!client.current_version) {
  // bcrypt-validate as today
  const ok = await bcrypt.compare(presentedSecret, client.clientSecret);
  return ok ? client : null;
}

// KMS path (versioned):
const pointers = {
  current: client.current_version,
  previous: client.previous_version,
};
const now = Date.now();

// Helper to try a version
const tryVersion = async (versionId: string) => {
  const vDoc = await db.collection('oauth2_clients').doc(clientId).collection('secrets').doc(versionId).get();
  if (!vDoc.exists) return false;
  const v = vDoc.data()!;
  if (v.state !== 'current' && v.state !== 'grace') return false;
  if (v.not_before && now < v.not_before.toMillis()) return false;
  if (v.not_after && now > v.not_after.toMillis()) return false;

  const data = KmsMacService.buildMacInput(clientId, versionId, presentedSecret);
  const ok = await KmsMacService.getInstance().macVerify(data, v.secret_hash);
  return ok;
};

// Try current, then previous if within grace
if (pointers.current && await tryVersion(pointers.current)) return client;
if (pointers.previous && await tryVersion(pointers.previous)) return client;

return null;
```

Phase 4 — Token version stamping (for immediate revoke)
- In generateAccessToken, add client_version_id to the JWT payload for clients with versioned secrets:
```ts
// when client.current_version exists
payload.client_version_id = client.current_version;
```
- In validateAccessToken, if immediate revoke was executed (grace=0) for a version, reject tokens where payload.client_version_id equals the retired version_id. For durable grace policies, rely on not_after enforcement in secret metadata.

Phase 5 — Rotation write path (relay) expectations
- rust-nostr-relay (separate repo) should:
  - Compute secret_hash with KmsMacService.macSign(buildMacInput(client_id, version_id, secret)).
  - Write oauth2_clients/{clientId}/secrets/{versionId} with metadata, state=pending, not_before=now+Δ.
  - After MLS notify and ack quorum, promote: set oauth2_clients/{clientId}.current_version=versionId, move previous to grace with not_after=not_before+grace.
  - Write oauth2_rotations audit record.

Phase 6 — Caching and invalidation
- Add a tiny TTL cache (e.g., 60s) for client pointers and metadata to reduce Firestore reads under load.
- Invalidate cache on changes:
  - Firestore listeners on oauth2_clients/{clientId} to observe pointer flips (promotion).
  - Optional: a relay-published control event via WebSocket to force cache clear after promotions.
- Add a small acceptance margin near not_before/not_after (e.g., ±2s) to mitigate skew; ensure NTP sync.

Phase 7 — Backward compatibility and migration flow
- Existing clients:
  - Continue to validate via bcrypt until rotated.
  - When you run /v1/oauth2/rotate (server’s existing API) or use the relay-based rotation, create a new KMS-backed version and start using the versioned path. Keep legacy bcrypt hash intact for historical records but not used post-rotation.
- Soft rollout:
  - Implement KMS path behind a feature flag (e.g., OAUTH_KMS_VERIFY=true).
  - Pilot with a non-critical client_id, observe metrics (latency, success rate).
  - Expand to all rotated clients.

Phase 8 — Observability and alerts
- Metrics:
  - MACVerify latency p50/p95/p99 and error rate
  - Validation success split by current vs previous
  - Invalid attempts by reason (unknown client, window violation, retired version)
  - Token acceptance/rejection by client_version_id (if immediate revoke used)
- Alerts:
  - KMS MAC error spikes
  - Retired-secret usage post-grace
  - Pending promotions exceeding SLA (if relay exposes)
- Logs:
  - No plaintext; include client_id, version used, rotation_id (if available), correlation id.

Phase 9 — Firestore Rules (outline)
- Restrict writes to:
  - oauth2_clients/* pointers and secrets subcollection to relay SA only
  - oauth2_rotations/* to relay SA only
- loxation-server:
  - Read-only access to oauth2_clients/* and oauth2_rotations/*
  - No writes to versions or pointers in normal operation

Phase 10 — Tests
- Unit tests:
  - KmsMacService.buildMacInput canonical format
  - KMS macVerify success/failure (use KMS emulator or mock)
  - Window enforcement
  - Token stamping logic
- Integration:
  - Version promotion path + validation transitions
  - Legacy bcrypt fallback path
- Load:
  - Issue many validateClientCredentials calls and measure KMS latency impact

Package/dependency changes
- Add @google-cloud/kms
  - npm install @google-cloud/kms
- Ensure GOOGLE_APPLICATION_CREDENTIALS is configured for server environments.

Security notes
- Do not log secrets or MACs.
- Limit KMS permissions to “use” on the specific key for loxation-server.
- Consider pinning a specific cryptoKeyVersion short-term during rollout; then relax to the key to permit rotation.

Appendix: Minimal code diff sketches (for future PRs)
- New file: src/services/kmsMacService.ts (see above)
- src/services/oauth2Service.ts
  - Inject/use KmsMacService in validateClientCredentials
  - Add version-aware branch as pseudocode above
  - In generateAccessToken, add client_version_id when current_version exists

Outcome
This plan integrates KMS MACVerify into the current project with minimal disruption:
- Legacy bcrypt validation continues until clients are rotated.
- New rotations produce KMS-backed versioned secrets and server-side MACVerify without exposing pepper material.
- The system becomes compatible with relay-driven MLS rotations and future immediate revoke semantics.
