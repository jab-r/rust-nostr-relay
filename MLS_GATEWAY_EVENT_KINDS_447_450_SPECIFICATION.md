# MLS Gateway Event Kinds 447 & 450 Specification

This document provides the complete technical specification for the newly implemented Nostr event kinds 447 (KeyPackage Request) and 450 (Roster/Policy) in the MLS Gateway Extension.

## Overview

These event kinds extend the MLS Gateway to support:
- **Cross-relay interoperability** via Nostr-based KeyPackage requests (447)
- **Deterministic membership management** via admin-signed roster/policy events (450)

Both kinds are designed to enhance the robustness and consistency of MLS group management while maintaining the security model of the existing implementation.

---

## Kind 447: KeyPackage Request

### Purpose
Enable relay systems or administrators to request fresh KeyPackages from specific users via Nostr, facilitating:
- Commodity relay interoperability (not just gateway WebSocket/REST)
- Background replenishment across devices
- Per-group or generic requests with constraints

### Schema

#### Required Tags
- `["p", "<target_owner_pubkey_hex>"]` — Recipient to deliver request to (NIP fanout)

#### Optional Tags
- `["h", "<group_id>"]` — Scoped to a group if the request is for onboarding or rekey
- `["cs", "<ciphersuite_id>"]` — Ciphersuite hint (e.g., MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519)
- `["min", "<int>"]` — Optional minimum number of KeyPackages requested (human hint)
- `["ttl", "<seconds>"]` — Relay may drop after TTL (default: 7 days)

#### Content (JSON, optional)
```json
{
  "reason": "stock_low|onboarding|rekey|other",
  "note": "optional free-form note",
  "min": 5,
  "ciphersuite": "MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519"
}
```

### Relay Rules

#### Authentication
- Accept from relay-system key (`system_pubkey` config) or configured group admins
- Optionally allow group owners if `["h", group_id]` present
- Enforce NIP-42 auth on publishers if desired

#### Delivery
- **Recipient-only** (by p tag). Do not broadcast publicly.
- **No sensitive content** exposure

#### Storage & Indexing
- **Short retention** (e.g., 1–7 days). Keep minimal archive; don't index for public discovery
- Index by `p` (recipient) and optionally `h` (group) for scoped retrieval
- **Rate limiting** per publisher

#### TTL Handling
- Check `["ttl"]` tag against `created_at` timestamp
- Reject expired requests
- Default TTL from `keypackage_request_ttl` config (default: 7 days)

### Example Event
```json
{
  "id": "...",
  "kind": 447,
  "pubkey": "relay_system_key_hex",
  "created_at": 1724050000,
  "tags": [
    ["p", "target_owner_pubkey_hex"],
    ["h", "grp_abc"],
    ["cs", "MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519"],
    ["min", "5"],
    ["ttl", "604800"]
  ],
  "content": "{\"reason\":\"stock_low\"}",
  "sig": "..."
}
```

### Security Considerations
- **No sensitive data** in content or tags
- **Low-risk** message shape suitable for cross-relay transport
- **Authorization required** from system or admin keys
- **Short-lived** to minimize storage overhead

---

## Kind 450: Roster/Policy (Admin-signed Membership Control)

### Purpose
Provide an explicit, admin-signed policy stream for group membership updates outside MLS message traffic. This supplements (not replaces) implicit onboarding via 1059/444 and/or sidecar ACLs.

### Schema

#### Required Tags
- `["h", "<group_id>"]` — Group identifier
- `["seq", "<int>"]` — Strictly monotonic per group; relay enforces > last_seq
- `["op", "<operation>"]` — Operation type (see below)

#### Repeated Tags
- `["p", "<member_pubkey_hex>"]` — One or more member pubkeys, depending on operation

#### Optional Tags
- `["role", "<role>"]` — Role hints for promote/demote operations (e.g., "admin")
- `["note", "..."]` — Human-readable context

#### Content (JSON, optional)
Opaque to relay, can contain arbitrary metadata:
```json
{
  "reason": "onboarding sync",
  "batch_id": "batch_2024_01_15",
  "external_ref": "ticket_12345"
}
```

### Operations

| Operation | Description | P Tags Usage |
|-----------|-------------|--------------|
| `add` | Add members in `["p"]` tags to group | Required: member pubkeys to add |
| `remove` | Remove members in `["p"]` tags from group | Required: member pubkeys to remove |
| `promote` | Adjust roles upward (with optional `["role"]` hint) | Required: member pubkeys to promote |
| `demote` | Adjust roles downward (with optional `["role"]` hint) | Required: member pubkeys to demote |
| `bootstrap` | Create group record with initial members | Required: initial member list |
| `replace` | Replace member list atomically with `["p"]` set | Required: complete new member list |

### Relay Rules

#### Authentication
- **Only accept** from configured group admin(s) or owner(s) for that specific group
- Verify admin authorization per group via `admin_pubkeys` configuration

#### Sequence Validation
- **Enforce strictly increasing** sequence numbers per `group_id`
- Drop events if `["seq"] <= last_seen_sequence` for the group
- **Idempotency guarantee**: duplicate sequences are rejected

#### Delivery
- **Fanout to current members** of the group
- **Archive indefinitely** (or per policy) to support backfill and audit

#### Registry Effects
Based on operation type:
- `add`: Add members in `["p"]` tags to group registry
- `remove`: Remove members in `["p"]` tags from group registry
- `promote/demote`: Adjust roles if server models roles
- `bootstrap`: Create group record with initial members (`["p"]` list)
- `replace`: Replace member list atomically with `["p"]` set

#### Storage & Indexing
- Index by `(h, seq)`, `h`, and `op`
- **Long-term archival** for audit trail and backfill support
- Sequence tracking per group for validation

### Example Event
```json
{
  "id": "...",
  "kind": 450,
  "pubkey": "admin_pubkey_hex",
  "created_at": 1724050000,
  "tags": [
    ["h", "grp_abc"],
    ["seq", "17"],
    ["op", "add"],
    ["p", "member1_pubkey_hex"],
    ["p", "member2_pubkey_hex"],
    ["note", "Onboard two users from batch"]
  ],
  "content": "{\"note\":\"onboard two users\"}",
  "sig": "..."
}
```

### Security Considerations
- **Admin-only publishing** with per-group authorization
- **Monotonic sequence numbers** prevent replay attacks
- **Event existence** may reveal group_id and membership changes
- **Audit trail** for compliance and debugging
- **Deterministic processing** via sequence ordering

---

## Implementation Notes

### Configuration Requirements

Both event kinds require additional configuration in `config/rnostr.toml`:

```toml
[extensions.mls_gateway]
# ... existing config ...

# System pubkey for KeyPackage requests (optional)
# If not set, only admin_pubkeys can send requests
system_pubkey = "relay_system_pubkey_hex"

# Admin pubkeys allowed to send roster/policy events and KeyPackage requests
admin_pubkeys = [
    "admin_pubkey_1_hex",
    "admin_pubkey_2_hex"
]

# TTL for KeyPackage requests in seconds (default: 7 days)
keypackage_request_ttl = 604800

# TTL for roster/policy events in days (default: 365 days)
roster_policy_ttl_days = 365
```

### Storage Schema Extensions

#### Firestore Collection: `roster_policy`
```json
{
  "id": "group_id_sequence",
  "group_id": "grp_abc",
  "sequence": 17,
  "operation": "add",
  "member_pubkeys": ["member1", "member2"],
  "admin_pubkey": "admin_hex",
  "created_at": 1724050000,
  "updated_at": 1724050000
}
```

#### SQL Table: `mls_roster_policy`
```sql
CREATE TABLE mls_roster_policy (
    id TEXT PRIMARY KEY,
    group_id TEXT NOT NULL,
    sequence BIGINT NOT NULL,
    operation TEXT NOT NULL,
    member_pubkeys TEXT[] NOT NULL,
    admin_pubkey TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(group_id, sequence)
);

CREATE INDEX idx_mls_roster_policy_group ON mls_roster_policy(group_id);
CREATE INDEX idx_mls_roster_policy_sequence ON mls_roster_policy(group_id, sequence);
```

### Metrics

New metrics added for monitoring:

```rust
// KeyPackage requests
mls_gateway_events_processed_total{kind="447"}
mls_gateway_keypackage_requests_processed

// Roster/policy events  
mls_gateway_events_processed_total{kind="450"}
mls_gateway_roster_policy_updates
mls_gateway_membership_updates_total
```

---

## Backward Compatibility

- **No conflicts** with existing kinds 443, 444, 445, 446, 1059
- **Additive functionality** - existing mechanisms continue to work
- **Optional features** - can be disabled if not needed
- **Configuration-driven** - enable via admin_pubkeys configuration

## Client Integration

### For KeyPackage Requests (447)
1. **Receive request** via WebSocket subscription to your pubkey
2. **Validate request** (check sender authorization, TTL)
3. **Generate and upload** requested KeyPackages via existing 443 flow
4. **Optional**: Respond with confirmation or error

### For Roster/Policy Events (450)
1. **Admin clients** can publish membership changes
2. **All group members** receive membership updates
3. **Sequence tracking** enables reliable ordering
4. **Audit trail** for compliance and debugging

---

## Migration from Existing Deployments

1. **Deploy updated service** with kinds 447/450 support
2. **Configure admin_pubkeys** for roster management
3. **Optional**: Set system_pubkey for automated requests
4. **Clients update** to handle new event kinds
5. **Gradual rollout** - old and new mechanisms coexist

The implementation is production-ready and deployed at `https://loxation-messaging-4dygmq5xta-uc.a.run.app` with full support for all event kinds including the new 447 and 450.