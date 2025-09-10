# Executive Brief: Automated Secret Rotation via MLS and rust-nostr-relay

Purpose
Reduce cyber risk from leaked OAuth2/X-API secrets by automating rotation and secure distribution, minimizing downtime and human error. Build on our MLS (Messaging Layer Security) foundation and rust-nostr-relay integration to provide auditable, role-gated rotations that scale.

Business Problem
- Static secrets are a high-value target. If leaked, attackers can access sensitive interfaces like `/external-totp`.
- Manual rotations are slow, inconsistent, and often coordinated via insecure channels.
- Compliance frameworks (SOC 2, ISO 27001) expect regular rotation, least-privilege access, and strong audit trails.

Solution Overview
- A secure, automated rotation service hosted in `rust-nostr-relay`, leveraging MLS to distribute new secrets only to authorized “admin” group members.
- `loxation-server` validates client credentials against versioned secrets and honors a configurable grace window to ensure seamless cutovers.
- Firestore stores only hashed secrets and rotation metadata; plaintext secrets are never persisted server-side.

What Changes
- Administrators initiate rotations in a mobile app (MLS-enabled). The relay generates a new secret, updates Firestore with a hash, and securely distributes the plaintext secret via MLS.
- During a grace period, both current and previous secrets are accepted. After grace, old secrets are retired automatically.
- All events (who, what, when, why) are logged for audit, without exposing secret material.

Key Benefits
- Risk reduction: Leaked secrets become short-lived; rotation becomes routine and safe.
- Secure distribution: MLS ensures end-to-end encrypted delivery only to admins.
- Audit-ready: Complete rotation history supports compliance and forensics.
- Low disruption: Grace period allows clients to update without downtime.
- Extensible: Applicable to OAuth2 client_secrets and X-API keys across services.

Security Posture
- No plaintext secret storage server-side; logs contain no sensitive values.
- Authorization via MLS “admin” group; optional JWT proof ties actions to verified admins.
- Strong crypto hygiene: KMS-protected pepper for hashing; TLS everywhere; zero plaintext logging.

Operating Model
1) Admin authenticates with mobile TOTP and requests rotation for a client_id.
2) Relay verifies admin authorization, generates a new secret, and stores its hash/metadata in Firestore.
3) Relay distributes the plaintext secret to the MLS admin group; admins update downstream systems.
4) `loxation-server` accepts both current and previous secret during grace; retires previous when grace expires.
5) Optional rollback within grace by re-promoting the previous secret.

Governance and Compliance
- Enforces regular rotation with clear ownership and separation of duties.
- Evidence-based auditing via Firestore records (who, when, why), suitable for SOC 2/ISO 27001 controls.
- Policy controls: default/max grace windows, minimum rotation intervals, denial lists.

KPIs
- Mean Time To Rotate (MTTR-Secret): target < 15 minutes.
- Client switchover rate before grace expiry: > 95%.
- Incidents involving leaked secrets: trending to zero.
- Audit completeness: 100% rotations with traceable provenance.

Timeline and Effort (High-Level)
- 2–3 weeks total:
  - Relay rotation service: ~1 week
  - Server validation updates: 3–5 days
  - Admin app UX: 2–3 days
  - Testing and documentation: 3–5 days
- Leverages existing infrastructure (rust-nostr-relay, MLS library, Firestore).

Risks and Mitigations
- Operator mishandling of plaintext secrets: mitigate via MLS-only delivery, UI warnings, and training.
- Misconfigured grace windows: enforce sensible defaults and maximums; preflight validation.
- Delayed client updates: monitor usage telemetry; proactive outreach; rollback available within grace.
- Relay compromise: KMS isolation for pepper; least-privilege access; strong monitoring and alerting.

Decisions Requested
- Require JWT proof (in addition to MLS membership) for rotation requests? Recommendation: Yes.
- Default and maximum grace windows? Recommendation: 7-day default, 30-day maximum.
- Enable break-glass encrypted copy of the secret in Firestore (relay-side KMS)? Recommendation: Disabled initially.

Next Steps
- Approve decisions above (JWT requirement, grace policy, break-glass).
- Implement relay rotation service and server verification updates in staging.
- Pilot with a non-critical client_id; validate metrics and runbooks.
- Roll out broadly with monitoring and alerts.
