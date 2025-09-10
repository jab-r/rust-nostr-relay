# Competitive Analysis: Automated Static API Key / OAuth2 Client Secret Rotation

Scope
Evaluate approaches and products that address rotation of static API credentials used by external services (e.g., OAuth2 client_credentials), against our requirements:
- Secure generation and distribution to human operators and/or services
- Zero plaintext secret persistence on servers
- End-to-end encryption during distribution
- Overlapped cutover (grace window) for zero downtime
- Strong operator identity assurance and auditability
- Fit with our stack (Firestore, rust-nostr-relay, MLS, React Native admin app)

Primary candidates evaluated
- Our approach: NIP-KR over rust-nostr-relay + MLS + Firestore (this repo family)
- HashiCorp Vault (KV v2, Dynamic Secrets, Response Wrapping, Control Groups)
- AWS Secrets Manager (+KMS/Parameter Store)
- GCP Secret Manager
- Azure Key Vault
- Cloud IAM short-lived identity (AWS STS, GCP Workload Identity, Azure Managed Identity)
- SPIFFE/SPIRE (x509/JWT SVID)
- mTLS client certificates (cert-manager/ACME, internal CA)
- Kubernetes External Secrets Operator (ESO) + Secret Stores
- “Do nothing”: ad-hoc/manual secret distribution over tickets/email/chats

Evaluation criteria
1) E2EE distribution to human operators
2) Zero plaintext secret persistence on servers (at rest and in logs)
3) Automated rotation orchestration and policy (grace windows, rate limits)
4) Strong operator identity (device attest, TOTP, MLS membership, PoP)
5) Overlapped cutover for external integrators (current+previous)
6) Audit depth and tamper-evident logging
7) Ecosystem fit (Firestore, Nostr/MLS, RN mobile app)
8) Operational complexity and TCO
9) Vendor lock-in and portability
10) Support for machine-to-machine (M2M) vs human-in-the-loop distribution
11) Scalability across many integrators and administrators
12) Offline/edge distribution capability

Decision matrix

| Solution | E2EE to humans | Zero plaintext at rest | Auto rotation & policy | Strong operator identity | Overlap grace at verifier | Audit depth | Ecosystem fit (Firestore/Nostr/MLS) | Ops complexity | Lock-in | M2M strength | Human distribution strength | Notes |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| NIP-KR (relay+MLS) | Yes (MLS) | Yes (hash-only; optional KMS break-glass off) | Yes (relay policy; idempotency; rate limits) | Yes (App Attest+TOTP+PoP+MLS) | Yes (current+previous) | High (Firestore audit) | Excellent | Medium | Low/Med (custom infra) | Medium | Excellent | Tailored to external integrators with human operators |
| Vault (KV v2 + Response Wrapping) | Partial (wrapping reduces exposure; not group E2EE) | No by default (encrypted at rest on server) | Yes (rotation via engines/workflows) | Medium (RBAC; MFA; no device attest) | Not built-in at verifier | High | Medium | Medium/High | Med/High | Excellent | Medium | Strong for M2M; human E2EE not native |
| Vault Dynamic Secrets | N/A (no human distribution) | N/A | Yes | Medium | N/A | High | Medium | Medium/High | Med/High | Excellent | Low | Best when clients are code that can fetch ephemeral creds; less fit for OAuth2 client_secret to third-parties |
| AWS Secrets Manager | No | No (encrypted at rest) | Yes (rotation lambdas) | Medium (IAM + optional MFA) | Not at verifier | Medium | Low | Low/Med | High (AWS) | High (AWS) | Low | Great inside AWS; human distribution out-of-band |
| GCP Secret Manager | No | No (encrypted at rest) | Limited (versions; rotation via Cloud Functions) | Medium | Not at verifier | Medium | Low | Low/Med | High (GCP) | High (GCP) | Low | Similar story as AWS |
| Azure Key Vault | No | No (encrypted at rest) | Limited (versions; rotation policies) | Medium | Not at verifier | Medium | Low | Low/Med | High (Azure) | High (Azure) | Low | Similar story as AWS/GCP |
| Cloud IAM Short-Lived Identity (STS etc.) | N/A | N/A | Yes (automatic) | High (workload identity) | N/A | High | Low | Medium | High | Excellent | Low | Changes protocol from static secrets → tokens/roles; great internally, hard for external integrators |
| SPIFFE/SPIRE | N/A | N/A | Yes (cert rotation) | High (attested workloads) | N/A | High | Low | High (adoption) | Low | Excellent | Low | Best-in-class for in-house workloads; not a fit for 3rd-party OAuth2 |
| mTLS client certs (cert-manager) | No (human distribution is manual) | N/A | Yes (cert rotation) | Medium (can add MFA) | At TLS layer, not OAuth2 | Medium | Low | Medium | Low/Med | High | Low | Improves security; integration burden for external partners |
| K8s ESO + Secret Stores | No | No (encrypted at rest) | Yes (controller-sync) | Medium | Not at verifier | Medium | Low | Medium | Medium | High | Low | Good for cluster-internal; not for human E2EE |
| Ad-hoc/manual | No | No | No | Low | No | Low | High risk | Low | Low | Low | Low | Not acceptable |

Findings

- Centralized secret managers excel for M2M, not human operators:
  - Vault/AWS/GCP/Azure securely store secrets and can automate rotation for services that pull secrets programmatically.
  - They do not natively provide E2EE distribution to a set of human administrators; console/download flows or API reads leave plaintext server-side or on endpoints and in logs.
  - “Response wrapping” improves point-to-point handoff but is not group messaging or MLS-grade E2EE, nor does it eliminate plaintext on backend storage.

- Identity-based approaches (SPIFFE/SPIRE, Cloud IAM with STS) eliminate static secrets but require broad changes:
  - Ideal for in-house workloads where you can adopt workload identities and mTLS/JWT-SVID.
  - Poor fit for third-party integrators using OAuth2 client_credentials, where static client_secret remains expected.

- mTLS client certs are powerful but heavy for external integrators:
  - Operational overhead of PKI, client cert issuance, revocation, and cross-org trust can be prohibitive.
  - Shifts the problem from secret rotation to cert lifecycle management for each partner.

- Our MLS-based approach uniquely targets human-in-the-loop distribution without server plaintext:
  - Plaintext secrets exist only in MLS messages to the admin group.
  - No server logs or Firestore documents contain the plaintext; only HMAC hashes with KMS-protected pepper.
  - Strong operator identity: MLS membership + App Attest (device integrity) + TOTP (user presence) + PoP (npub).
  - Verifier (loxation-server) supports current+previous grace, enabling zero-downtime cutover.

Cost/Complexity summary

- NIP-KR (relay+MLS): Medium complexity to implement and operate; low vendor lock-in; good TCO once in place; fits our stack perfectly.
- Vault: Medium/High complexity; licensing (Enterprise) for advanced workflows; excellent ecosystem if primarily machine-oriented and you can centralize retrieval.
- Cloud Secrets Managers: Low/Medium complexity; strong if everything is inside one cloud; weakest for cross-org human distribution and zero-plaintext constraint.
- SPIFFE/SPIRE/STS: High initial adoption cost; best if you can eliminate static secrets entirely (internal services).
- mTLS client certs: Medium complexity; hard for cross-org at scale.

When to choose what

- Choose NIP-KR (relay+MLS) when:
  - You must distribute new secrets to human operators across organizations securely and quickly.
  - Plaintext secrets must not persist on servers or logs.
  - You need dual-secret overlap at the verifier for zero downtime.
  - You want strong admin identity guarantees and MLS-based acknowledgments.
  - You integrate with Firestore and Nostr/MLS already.

- Choose Vault/Dynamic Secrets when:
  - Consumers are code (services) that can fetch on-demand short-lived credentials.
  - Human operators are not in the loop or can use alternative secure channels.
  - Secrets do not need to be E2EE-delivered to many admins simultaneously.

- Choose Cloud Secrets Manager when:
  - All parties and workloads are inside one cloud boundary.
  - Rotation and distribution can be automated entirely to services (not human admins).
  - You accept secrets encrypted at rest on provider infra and can tolerate console/API retrieval.

- Choose SPIFFE/SPIRE or Cloud IAM STS when:
  - You can fundamentally avoid static secrets in favor of workload identity.
  - External integrators are not a factor, or you can federate identity across orgs.

- Consider mTLS client certs when:
  - External partners are capable of mTLS and certificate lifecycle management.
  - You can give up OAuth2 client_credentials in favor of cert-based auth.

Risks and mitigations (NIP-KR)

- Custom control plane risk → Mitigate with clear spec (NIP-KR), tests, and runbooks; limit scope.
- MLS admin group management → Document membership procedures; implement admin UIs and audit.
- Mobile dependency → Provide backup admin paths (e.g., quorum approvals; disaster recovery runbooks).
- Adoption by external integrators → Use grace windows, proactive comms, and telemetry to track cutover progress.

Conclusion

For the specific problem of rotating OAuth2 client_secrets and X-API keys used by external integrators—and doing so with zero plaintext persistence, E2EE human distribution, and verifiable admin identity—the NIP-KR (relay+MLS) approach best matches the requirements with the least compromise. For internal-only systems, identity-based approaches (SPIFFE/STS) or dynamic secrets (Vault) provide stronger posture by eliminating static secrets entirely, but they do not generalize well to third-party integrators expecting OAuth2 / X-API header client_credentials.

Appendix A: Glossary
- MLS: Messaging Layer Security (RFC 9420)
- PoP: Proof of Possession (binding token to key)
- E2EE: End-to-end encryption
- SVID: SPIFFE Verifiable Identity Document
- STS: Security Token Service (short-lived credentials)
- KMS: Key Management Service

References
- NIP-KR: docs/nips/nip-kr.md
- High-level architecture: docs/architecture/high-level-architecture-automatic-api-key-rotation.md
- Implementation plan: docs/architecture/oauth2-mls-key-rotation-implementation-plan.md
- Vault: https://www.vaultproject.io/
- AWS Secrets Manager: https://aws.amazon.com/secrets-manager/
- GCP Secret Manager: https://cloud.google.com/secret-manager
- Azure Key Vault: https://azure.microsoft.com/products/key-vault/
- SPIFFE/SPIRE: https://spiffe.io/
