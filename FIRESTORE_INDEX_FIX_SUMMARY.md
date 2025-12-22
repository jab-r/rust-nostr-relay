# Firestore Index Fix Summary

## Problem
The MLS Gateway was experiencing persistent Firestore index errors due to complex queries with multiple inequality filters, ordering requirements, and the combination of these constraints.

The problematic query pattern was:
- Filter by `owner_pubkey` (optional)
- Filter by `created_at >= since` (optional)
- Filter by `expires_at > now` (always)
- Order by `created_at` DESC

This created complex composite index requirements that were difficult to manage.

## Solution
We simplified the query pattern and added a separate cleanup mechanism:

### 1. Simplified Query (extensions/src/mls_gateway/firestore.rs)
- Removed ordering by `created_at` (keypackages are fungible, order doesn't matter)
- Removed expiration filtering from queries
- Simplified to just filter by `owner_pubkey` when needed
- Result: Queries now only need simple single-field indexes that Firestore creates automatically

### 2. Added Cleanup Method (extensions/src/mls_gateway/firestore.rs)
- Added `cleanup_expired_keypackages()` method to FirestoreStorage
- Queries for keypackages where `expires_at <= now`
- Deletes expired keypackages
- Returns count of deleted items

### 3. Added Cleanup Command (src/cleanup.rs, src/main.rs)
- Added `rnostr cleanup` command
- Connects to Firestore and runs cleanup
- Can be packaged as a Cloud Run Job

### 4. Updated Firestore Indexes (firestore.indexes.json)
- Removed all complex composite indexes for `mls_keypackages`
- Firestore will use automatic single-field indexes
- No more index errors!

## Deployment Steps

### 1. Deploy the code changes
```bash
./scripts/deploy.sh
```

### 2. Deploy the Firestore indexes
```bash
firebase deploy --only firestore:indexes
```
Note: When asked to delete old composite indexes, answer YES

### 3. Set up Cloud Run Job for cleanup

#### Option A: Same container, different command
Use the same container image but with the cleanup command:
```bash
# Create the Cloud Run Job
gcloud run jobs create mls-keypackage-cleanup \
  --image gcr.io/loxation-f8e1c/loxation-messaging:latest \
  --args "cleanup" \
  --region us-central1 \
  --parallelism 1 \
  --task-timeout 5m \
  --max-retries 1
```

#### Option B: Add scheduler trigger
In Cloud Console:
1. Go to Cloud Run → Jobs → mls-keypackage-cleanup
2. Click "Triggers" tab → "Add Scheduler Trigger"
3. Set schedule: `0 2 * * *` (daily at 2 AM)
4. Set timezone: `America/New_York`

Or via gcloud:
```bash
gcloud scheduler jobs create http mls-keypackage-cleanup-schedule \
  --location us-central1 \
  --schedule "0 2 * * *" \
  --time-zone "America/New_York" \
  --uri "https://us-central1-run.googleapis.com/apis/run.googleapis.com/v1/namespaces/loxation-f8e1c/jobs/mls-keypackage-cleanup:run" \
  --http-method POST \
  --oauth-service-account-email PROJECT-NUMBER-compute@developer.gserviceaccount.com
```

## Environment Variables

The cleanup job needs the same environment variables as your main application:
- `MLS_FIRESTORE_PROJECT_ID` or `GOOGLE_CLOUD_PROJECT` or `GCP_PROJECT`
- Any Firestore authentication credentials (usually automatic in Cloud Run)

## Testing

### Test locally:
```bash
# Set environment variables
export GOOGLE_CLOUD_PROJECT=loxation-f8e1c

# Run cleanup
cargo run -- cleanup
```

### Test the Cloud Run Job manually:
```bash
gcloud run jobs execute mls-keypackage-cleanup --region us-central1
```

## Benefits

1. **No more index errors** - Simple queries don't need complex composite indexes
2. **Better performance** - Queries are simpler and faster
3. **Cleaner database** - Expired keypackages are removed daily
4. **Easier maintenance** - No complex index management required
5. **Secure** - No public endpoint needed, runs as a scheduled job