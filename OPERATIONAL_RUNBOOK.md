# MLS Gateway Extension - Operational Runbook

## Table of Contents

1. [Daily Operations](#daily-operations)
2. [Monitoring & Alerting](#monitoring--alerting)
3. [Incident Response](#incident-response)
4. [Maintenance Procedures](#maintenance-procedures)
5. [Deployment Procedures](#deployment-procedures)
6. [Backup & Recovery](#backup--recovery)
7. [Security Operations](#security-operations)
8. [Performance Management](#performance-management)
9. [Troubleshooting Playbooks](#troubleshooting-playbooks)
10. [Emergency Procedures](#emergency-procedures)

---

## Daily Operations

### Morning Health Check

**Frequency**: Daily at 9:00 AM UTC  
**Duration**: ~10 minutes  
**Owner**: Platform Team

#### Checklist
```bash
# 1. Service Status Check
gcloud run services describe loxation-messaging \
  --region us-central1 \
  --format="value(status.conditions[0].status)"
# Expected: True

# 2. Health Endpoint Verification  
curl -f https://loxation-messaging-4dygmq5xta-uc.a.run.app/health
# Expected: {"status":"healthy","version":"0.4.8"}

# 3. WebSocket Connectivity Test
node test-websocket.js
# Expected: ✅ WebSocket connection established

# 4. Recent Error Rate Check
gcloud logging read "resource.type=cloud_run_revision AND severity>=ERROR" \
  --limit=10 --freshness=1h --project=loxation-f8e1c
# Expected: No critical errors

# 5. Firestore Health Check
gcloud firestore operations list --limit=5
# Expected: No failed operations
```

#### Key Metrics to Review
- **Event Processing Rate**: Should be within normal range (varies by usage)
- **Error Rate**: Should be < 1%
- **Response Time P95**: Should be < 500ms
- **Active Connections**: Monitor for unusual spikes
- **Memory Usage**: Should be < 80% of allocated

#### Actions Required
- ✅ **Green**: All checks pass → Continue monitoring
- ⚠️ **Yellow**: Minor issues detected → Investigate within 2 hours
- 🚨 **Red**: Critical issues → Initiate incident response immediately

---

## Monitoring & Alerting

### Critical Alerts

#### High Error Rate
```yaml
Alert: MLS_Gateway_High_Error_Rate
Condition: error_rate > 5% for 5 minutes
Severity: CRITICAL
Response Time: 15 minutes
```

**Response Steps:**
1. Check Cloud Run logs for error patterns
2. Verify Firestore connectivity and permissions
3. Check if related to specific event kinds
4. Scale up instances if capacity issue
5. Contact on-call engineer if unresolved

#### Service Down
```yaml
Alert: MLS_Gateway_Service_Down  
Condition: health_check_failures > 3 consecutive
Severity: CRITICAL
Response Time: 5 minutes
```

**Response Steps:**
1. Check Cloud Run service status
2. Verify network connectivity
3. Review recent deployments
4. Check Cloud Run logs for startup errors
5. Rollback if recent deployment caused issue

#### Database Connection Issues
```yaml
Alert: MLS_Gateway_Database_Errors
Condition: firestore_errors > 10% for 5 minutes  
Severity: HIGH
Response Time: 30 minutes
```

**Response Steps:**
1. Check Firestore service status
2. Verify IAM permissions
3. Check for quota limits
4. Review connection pool settings
5. Consider read replicas if read-heavy

### Warning Alerts

#### High Response Time
```yaml
Alert: MLS_Gateway_Slow_Response
Condition: p95_response_time > 1000ms for 10 minutes
Severity: WARNING  
Response Time: 1 hour
```

#### High Memory Usage
```yaml
Alert: MLS_Gateway_Memory_High
Condition: memory_usage > 85% for 15 minutes
Severity: WARNING
Response Time: 2 hours
```

#### Unusual Traffic Patterns
```yaml
Alert: MLS_Gateway_Traffic_Spike
Condition: event_rate > 150% of baseline for 20 minutes
Severity: INFO
Response Time: 4 hours
```

### Monitoring Dashboards

#### Primary Dashboard Metrics
```
Row 1: Service Health
- Service Status (Up/Down)
- Active Connections
- Request Rate (per second)
- Error Rate (%)

Row 2: Performance  
- Response Time (P50, P95, P99)
- CPU Usage (%)
- Memory Usage (%)
- Database Connection Pool

Row 3: MLS-Specific Metrics
- Events by Kind (443, 445, 446, 1059)
- Group Operations
- KeyPackage Operations  
- Message Archive Rate

Row 4: Infrastructure
- Cloud Run Instances
- Firestore Operations
- Network I/O
- Log Volume
```

---

## Incident Response

### Severity Levels

#### SEV1 - Critical
**Definition**: Complete service outage or data loss  
**Response Time**: 15 minutes  
**Escalation**: Immediate to on-call engineer and management

**Examples:**
- Service completely unreachable
- Data corruption detected
- Security breach confirmed

#### SEV2 - High  
**Definition**: Significant service degradation  
**Response Time**: 1 hour  
**Escalation**: To on-call engineer within 30 minutes

**Examples:**
- High error rates (>10%)
- Performance severely degraded
- Partial service outage

#### SEV3 - Medium
**Definition**: Minor service impact  
**Response Time**: 4 hours  
**Escalation**: During business hours

**Examples:**
- Warning alerts triggered
- Non-critical features affected
- Performance slightly degraded

### Incident Response Process

#### 1. Detection & Assessment (0-5 minutes)
```bash
# Immediate assessment commands
gcloud run services describe loxation-messaging --region us-central1
curl -I https://loxation-messaging-4dygmq5xta-uc.a.run.app/health  
gcloud logging read "resource.type=cloud_run_revision AND severity>=ERROR" --limit=20
```

#### 2. Initial Response (5-15 minutes)
- Create incident ticket
- Notify relevant stakeholders
- Gather initial diagnostic information
- Determine severity level

#### 3. Investigation & Mitigation (15+ minutes)
- Deep dive into root cause
- Implement temporary workarounds
- Apply fixes as appropriate
- Monitor for resolution

#### 4. Recovery & Validation (Variable)
- Verify service restoration
- Run validation tests
- Monitor for any recurring issues
- Update stakeholders

#### 5. Post-Incident Review (Within 48 hours)
- Document root cause
- Identify prevention measures
- Update runbooks/monitoring
- Share learnings with team

### Communication Templates

#### Initial Incident Notification
```
Subject: [SEV{X}] MLS Gateway Service Issue - {Brief Description}

Issue: {Description of the problem}
Impact: {Who/what is affected}  
Start Time: {When issue began}
Current Status: {Investigation/Mitigation/Resolved}
Next Update: {When next update will be provided}
Incident Commander: {Name and contact}
```

#### Resolution Notification
```
Subject: [RESOLVED] MLS Gateway Service Issue - {Brief Description}

The incident has been resolved as of {timestamp}.

Root Cause: {Brief explanation}
Resolution: {What was done to fix it}
Preventive Actions: {What will be done to prevent recurrence}
Post-Incident Review: {When/where it will happen}
```

---

## Maintenance Procedures

### Weekly Maintenance

#### Log Cleanup (Every Sunday 02:00 UTC)
```bash
# Clean up old Cloud Run logs (older than 30 days)
gcloud logging sinks delete old-logs-sink --quiet 2>/dev/null || true

# Archive message data older than retention period
# This should be automated via Firestore TTL policies
```

#### Security Updates (Every Sunday 03:00 UTC)
```bash
# Check for security updates
cargo audit

# Review dependency updates
cargo outdated

# Update base Docker image if needed
docker pull rust:1.75-slim
```

#### Performance Review (Every Sunday 04:00 UTC)
```bash
# Generate weekly performance report
gcloud monitoring metrics list --filter="metric.type:custom.googleapis.com/mls_gateway"

# Review slow queries and operations
# Check Firestore performance metrics in Console

# Analyze traffic patterns and plan capacity
```

### Monthly Maintenance

#### Certificate Renewal (1st Sunday of month)
```bash
# Cloud Run automatically handles TLS certificates
# Verify certificate validity
openssl s_client -connect loxation-messaging-4dygmq5xta-uc.a.run.app:443 -servername loxation-messaging-4dygmq5xta-uc.a.run.app | openssl x509 -noout -dates
```

#### Capacity Planning Review (1st Sunday of month)
```bash
# Review resource utilization trends
gcloud run services describe loxation-messaging --region us-central1 --format="value(spec.template.spec.containerConcurrency)"

# Analyze scaling patterns
gcloud logging read "resource.type=cloud_run_revision" --filter="jsonPayload.message:scaling" --limit=100

# Update resource limits if needed
```

#### Security Audit (1st Sunday of month)
```bash
# Review access logs for anomalies
gcloud logging read "resource.type=cloud_run_revision AND httpRequest.status>=400" --limit=100

# Check IAM permissions
gcloud projects get-iam-policy loxation-f8e1c

# Review pubkey allowlist
grep -A 10 "pubkey_whitelist" config/rnostr.toml
```

### Quarterly Maintenance

#### Dependency Updates (1st week of quarter)
```bash
# Update Rust dependencies
cargo update

# Update base Docker image
docker pull rust:latest

# Test thoroughly in staging environment
cargo test --all
cargo bench
```

#### Disaster Recovery Test (2nd week of quarter)
```bash
# Test backup restoration procedures
# Verify geo-redundancy setup
# Practice incident response scenarios
# Update emergency contact information
```

---

## Deployment Procedures

### Standard Deployment

#### Pre-Deployment Checklist
- [ ] Code reviewed and approved
- [ ] All tests passing
- [ ] Security scan completed
- [ ] Staging deployment successful
- [ ] Rollback plan prepared
- [ ] Deployment window scheduled
- [ ] Stakeholders notified

#### Deployment Steps
```bash
# 1. Build and test locally
cargo test --all
cargo build --release

# 2. Build Docker image
docker build -t gcr.io/loxation-f8e1c/loxation-messaging:v$(date +%Y%m%d_%H%M%S) .

# 3. Push to registry  
docker push gcr.io/loxation-f8e1c/loxation-messaging:v$(date +%Y%m%d_%H%M%S)

# 4. Update Cloud Run service
gcloud run deploy loxation-messaging \
  --image gcr.io/loxation-f8e1c/loxation-messaging:v$(date +%Y%m%d_%H%M%S) \
  --region us-central1 \
  --platform managed

# 5. Verify deployment
curl https://loxation-messaging-4dygmq5xta-uc.a.run.app/health
node test-websocket.js

# 6. Monitor for 30 minutes
watch -n 30 'gcloud run services describe loxation-messaging --region us-central1 --format="value(status.conditions[0].status)"'
```

#### Post-Deployment Checklist
- [ ] Health checks passing
- [ ] WebSocket functionality verified
- [ ] Error rates normal
- [ ] Performance metrics stable
- [ ] Logs free of errors
- [ ] Rollback plan tested (if critical deployment)

### Emergency Deployment

#### Hotfix Process
```bash
# For critical security fixes or major outages

# 1. Create hotfix branch
git checkout -b hotfix/critical-fix

# 2. Apply minimal necessary changes
# ... make changes ...

# 3. Test quickly but thoroughly
cargo test --package nostr-extensions

# 4. Fast-track deployment
./scripts/deploy.sh --emergency

# 5. Monitor closely
# Watch logs and metrics for 1 hour minimum
```

### Rollback Procedures

#### Automatic Rollback Triggers
- Error rate > 25% for 5 minutes
- Health checks failing for 3 consecutive checks
- Memory usage > 95% consistently

#### Manual Rollback
```bash
# 1. Get previous revision
PREVIOUS_REVISION=$(gcloud run revisions list --service=loxation-messaging --region=us-central1 --limit=2 --format="value(metadata.name)" | tail -1)

# 2. Rollback to previous revision
gcloud run services update-traffic loxation-messaging \
  --to-revisions=$PREVIOUS_REVISION=100 \
  --region=us-central1

# 3. Verify rollback success
curl https://loxation-messaging-4dygmq5xta-uc.a.run.app/health

# 4. Investigate original issue
gcloud logging read "resource.type=cloud_run_revision" --limit=50
```

---

## Backup & Recovery

### Backup Strategy

#### Firestore Backups
```bash
# Automated daily backups (configured in GCP)
gcloud firestore import gs://loxation-f8e1c-backups/$(date +%Y-%m-%d)

# Verify backup integrity weekly
gcloud firestore export gs://loxation-f8e1c-backups/verify-$(date +%Y-%m-%d)
```

#### Configuration Backups
```bash
# Store configurations in version control
git add config/
git commit -m "Backup configuration $(date)"
git push origin main

# Backup Cloud Run service configuration
gcloud run services describe loxation-messaging --region us-central1 > backup/cloud-run-config-$(date +%Y%m%d).yaml
```

#### Container Image Backups
```bash
# Images are automatically retained in GCR
# Verify critical images are present
gcloud container images list --repository=gcr.io/loxation-f8e1c/loxation-messaging
```

### Recovery Procedures

#### Data Recovery
```bash
# 1. Stop service to prevent data corruption
gcloud run services update loxation-messaging --min-instances=0 --region=us-central1

# 2. Restore from backup
gcloud firestore import gs://loxation-f8e1c-backups/YYYY-MM-DD

# 3. Verify data integrity
# Run data validation queries

# 4. Restart service
gcloud run services update loxation-messaging --min-instances=1 --region=us-central1
```

#### Service Recovery
```bash
# 1. Deploy known good image
gcloud run deploy loxation-messaging \
  --image gcr.io/loxation-f8e1c/loxation-messaging:known-good-tag \
  --region us-central1

# 2. Verify functionality
curl https://loxation-messaging-4dygmq5xta-uc.a.run.app/health
node test-websocket.js

# 3. Monitor recovery
# Watch metrics for 2 hours minimum
```

### Recovery Time Objectives (RTO)

| Scenario | RTO | RPO |
|----------|-----|-----|
| Service restart | 5 minutes | 0 |
| Rollback deployment | 10 minutes | 0 |
| Restore from backup | 2 hours | 24 hours |
| Full disaster recovery | 4 hours | 24 hours |

---

## Security Operations

### Daily Security Tasks

#### Access Review
```bash
# Review unusual access patterns
gcloud logging read "resource.type=cloud_run_revision AND httpRequest.userAgent!~'node-test'" --limit=20

# Check for failed authentication attempts
gcloud logging read "resource.type=cloud_run_revision AND jsonPayload.level=ERROR AND jsonPayload.message~'auth'" --limit=10
```

#### Vulnerability Monitoring
```bash
# Check for security alerts
cargo audit

# Review dependency vulnerabilities
npm audit --audit-level=high

# Check base image vulnerabilities
gcloud container analysis vulnerabilities list --project=loxation-f8e1c
```

### Weekly Security Tasks

#### Certificate Validation
```bash
# Verify TLS certificate
openssl s_client -connect loxation-messaging-4dygmq5xta-uc.a.run.app:443 -servername loxation-messaging-4dygmq5xta-uc.a.run.app | openssl x509 -noout -dates
```

#### Access Log Analysis
```bash
# Analyze access patterns for anomalies
gcloud logging read "resource.type=cloud_run_revision" --format="csv(timestamp,httpRequest.remoteIp,httpRequest.userAgent)" > access_analysis.csv

# Check for unusual user agents or IP patterns
```

#### Security Configuration Review
```bash
# Verify NIP-42 authentication is enabled
grep -n "enabled = true" config/rnostr.toml

# Check pubkey allowlist is current
cat config/rnostr.toml | grep -A 20 "pubkey_whitelist"

# Verify environment variables are secure
gcloud run services describe loxation-messaging --region us-central1 --format="value(spec.template.spec.template.spec.containers[0].env[])"
```

### Incident Response Security

#### Suspected Security Breach
```bash
# 1. Immediately disable public access
gcloud run services update loxation-messaging --no-allow-unauthenticated --region us-central1

# 2. Capture logs for analysis
gcloud logging read "resource.type=cloud_run_revision" --freshness=24h > security_incident_logs.txt

# 3. Review all access in past 24 hours
gcloud logging read "resource.type=cloud_run_revision AND httpRequest.status>=200" --freshness=24h

# 4. Check for data exfiltration
# Review Firestore access logs
# Check for unusual download patterns

# 5. Coordinate with security team
# Follow organization security incident procedures
```

---

## Performance Management

### Performance Monitoring

#### Key Performance Indicators
```bash
# Response Time Monitoring
gcloud monitoring metrics list --filter="metric.type:run.googleapis.com/container/cpu/utilization"

# Throughput Monitoring  
gcloud logging read "resource.type=cloud_run_revision" --format="value(timestamp)" | wc -l

# Error Rate Monitoring
gcloud logging read "resource.type=cloud_run_revision AND severity>=ERROR" --freshness=1h | wc -l
```

#### Performance Baselines
- **Response Time P95**: < 500ms
- **Throughput**: 1000 events/second per instance
- **CPU Utilization**: < 70% average
- **Memory Usage**: < 80% of allocated
- **Error Rate**: < 1%

### Performance Optimization

#### Scaling Configuration
```bash
# Optimize autoscaling settings
gcloud run services update loxation-messaging \
  --min-instances=2 \
  --max-instances=50 \
  --concurrency=100 \
  --cpu=1 \
  --memory=1Gi \
  --region=us-central1
```

#### Database Optimization
```bash
# Monitor Firestore performance
# Review indexes and query patterns
# Consider read replicas for read-heavy workloads

# Optimize connection pooling
# Review connection pool settings in configuration
```

#### Caching Strategy
```bash
# Implement caching for frequently accessed data
# Consider Redis or Memcached for session data
# Use Cloud CDN for static content
```

---

## Troubleshooting Playbooks

### WebSocket Connection Issues

#### Symptoms
- Clients cannot establish WebSocket connections
- Connection drops frequently
- Authentication failures

#### Diagnostic Steps
```bash
# 1. Test WebSocket connectivity
websocat wss://loxation-messaging-4dygmq5xta-uc.a.run.app

# 2. Check for proxy/firewall issues
curl -H "Upgrade: websocket" -H "Connection: Upgrade" -H "Sec-WebSocket-Version: 13" -H "Sec-WebSocket-Key: test" https://loxation-messaging-4dygmq5xta-uc.a.run.app

# 3. Review connection logs
gcloud logging read "resource.type=cloud_run_revision AND jsonPayload.message~'websocket'" --limit=20

# 4. Check for resource limits
gcloud run services describe loxation-messaging --region us-central1 --format="value(spec.template.spec.template.spec.containers[0].resources)"
```

#### Common Solutions
- Increase connection timeout settings
- Scale up instances for higher connection capacity
- Check client authentication implementation
- Verify WebSocket headers and protocols

### Database Connection Problems

#### Symptoms
- High database error rates
- Slow query performance
- Connection pool exhaustion

#### Diagnostic Steps
```bash
# 1. Check Firestore service status
gcloud firestore operations list

# 2. Review connection errors
gcloud logging read "resource.type=cloud_run_revision AND jsonPayload.message~'firestore'" --limit=20

# 3. Monitor connection pool usage
# Check application metrics for pool exhaustion

# 4. Verify IAM permissions
gcloud projects get-iam-policy loxation-f8e1c --flatten="bindings[].members" --filter="bindings.members:firebase-adminsdk"
```

#### Common Solutions
- Increase connection pool size
- Optimize query patterns and indexes
- Implement connection retrying with backoff
- Check for quota limits and increase if needed

### High Memory Usage

#### Symptoms
- Memory usage > 90%
- Out of memory errors
- Performance degradation

#### Diagnostic Steps
```bash
# 1. Check memory allocation
gcloud run services describe loxation-messaging --region us-central1 --format="value(spec.template.spec.template.spec.containers[0].resources.limits.memory)"

# 2. Review memory usage patterns
gcloud monitoring timeseries list --filter='metric.type="run.googleapis.com/container/memory/utilizations"'

# 3. Check for memory leaks
# Review application logs for memory-related errors

# 4. Analyze request patterns
gcloud logging read "resource.type=cloud_run_revision" --format="value(httpRequest.requestSize)" | sort -nr | head -20
```

#### Common Solutions
- Increase memory allocation
- Optimize data structures and caching
- Implement garbage collection tuning
- Review for memory leaks in application code

---

## Emergency Procedures

### Complete Service Outage

#### Immediate Response (0-5 minutes)
1. **Confirm outage scope**
   ```bash
   curl -f https://loxation-messaging-4dygmq5xta-uc.a.run.app/health
   gcloud run services describe loxation-messaging --region us-central1
   ```

2. **Check service status**
   ```bash
   gcloud run services list --filter="loxation-messaging"
   gcloud logging read "resource.type=cloud_run_revision AND severity>=ERROR" --limit=10
   ```

3. **Immediate mitigation**
   ```bash
   # Try service restart
   gcloud run services update loxation-messaging --region us-central1 --update-env-vars=RESTART_TIME=$(date +%s)
   ```

#### Short-term Response (5-30 minutes)
1. **Rollback to last known good version**
   ```bash
   LAST_GOOD_REVISION=$(gcloud run revisions list --service=loxation-messaging --region=us-central1 --limit=2 --format="value(metadata.name)" | tail -1)
   gcloud run services update-traffic loxation-messaging --to-revisions=$LAST_GOOD_REVISION=100 --region=us-central1
   ```

2. **Scale up instances**
   ```bash
   gcloud run services update loxation-messaging --min-instances=5 --region=us-central1
   ```

3. **Enable maintenance mode** (if needed)
   ```bash
   # Deploy minimal service that returns maintenance message
   ```

#### Long-term Response (30+ minutes)
1. **Root cause analysis**
2. **Implement proper fix**
3. **Comprehensive testing**
4. **Gradual traffic restoration**

### Data Corruption

#### Immediate Response
1. **Stop all write operations**
   ```bash
   gcloud run services update loxation-messaging --min-instances=0 --region=us-central1
   ```

2. **Assess corruption scope**
   ```bash
   # Run data integrity checks
   # Identify affected timeframe
   ```

3. **Initiate data recovery**
   ```bash
   # Restore from most recent clean backup
   gcloud firestore import gs://loxation-f8e1c-backups/LATEST_CLEAN_BACKUP
   ```

### Security Incident

#### Immediate Response
1. **Isolate service**
   ```bash
   gcloud run services update loxation-messaging --no-allow-unauthenticated --region=us-central1
   ```

2. **Preserve evidence**
   ```bash
   gcloud logging read "resource.type=cloud_run_revision" --freshness=48h > security_logs_$(date +%Y%m%d_%H%M%S).txt
   ```

3. **Coordinate response**
   - Notify security team
   - Follow organization incident procedures
   - Document all actions taken

---

## Contact Information

### Escalation Matrix

| Role | Primary | Secondary | Contact Method |
|------|---------|-----------|----------------|
| On-Call Engineer | John Doe | Jane Smith | PagerDuty + Slack |
| Platform Lead | Tech Lead | Senior Engineer | Slack + Email |
| Security Team | CISO | Security Engineer | Email + Phone |
| Management | Engineering Manager | Director | Email + Phone |

### Emergency Contacts

- **24/7 On-Call**: PagerDuty escalation
- **Security Hotline**: security@company.com
- **Infrastructure Team**: infra-oncall@company.com
- **Management Escalation**: engineering-leadership@company.com

### External Contacts

- **Google Cloud Support**: Case priority based on severity
- **Security Vendor**: Contact if security incident
- **Legal/Compliance**: For regulatory issues

---

*Last Updated: January 2025*  
*Version: 1.0.0*  
*Owner: Platform Engineering Team*