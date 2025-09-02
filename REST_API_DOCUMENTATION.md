# MLS Secure Relay REST API Documentation

## Overview

The MLS Secure Relay provides a REST API for auxiliary MLS (Messaging Layer Security) operations that complement the core Nostr relay functionality. This API enables key package distribution, welcome message delivery, group registry management, and offline message retrieval.

## Base URL

```
https://your-relay-domain.com/api/v1
```

## Authentication

The API uses HTTP Basic Authentication with service credentials. Authentication requirements vary by endpoint:

- **Protected endpoints**: HTTP Basic Auth with service credentials required
- **Admin endpoints**: Additional authorization checks
- **No anonymous access**: All endpoints require authentication

## Nostr Event Equivalents

Most REST API functionality has equivalent Nostr events:

| REST Endpoint | Nostr Event Kind | Purpose | Security Note |
|---------------|------------------|---------|---------------|
| `POST /keypackages` | Kind 443 | Store key packages | Private - server acts as mailbox |
| `GET /keypackages` | Kind 447 | Request key packages | Cross-relay interoperability |
| `POST /welcome` | Kind 444 (in 1059) | Store welcome messages | Private - server acts as mailbox |
| `GET /welcome` | Kind 444 (in 1059) | Retrieve welcome messages | Private - server acts as mailbox |

**Note**: Kind 450 (Roster/Policy) is for **admin-signed membership management**, not public group listing. It provides deterministic group membership updates but does not expose group information publicly. The `/groups` REST endpoint is a separate service for group discovery that should be restricted or removed in production.

### ⚠️ Security Warning: Removed Group Listing

**The `/groups` endpoints have been removed** from the documentation due to security risks:

1. **Group Discovery**: Public listing reveals private group IDs
2. **Metadata Exposure**: Group names and member counts expose structure
3. **Enumeration Attacks**: Enables systematic probing of group membership
4. **Privacy Violation**: Users expect group communication to be private

**Recommendation**: Remove the `/groups` routes from production deployments or restrict access to authenticated admin users only.

**Configuration**: Add an environment variable `MLS_GATEWAY_ENABLE_GROUP_LISTING=false` (default) to control exposure of group information.

## API Endpoints

### Key Package Mailbox

#### Store Key Package

**POST** `/keypackages`

Store a key package for delivery to a recipient.

**Request Body:**
```json
{
  "recipient": "npub1...",
  "sender": "npub1...",
  "content_b64": "base64_encoded_key_package",
  "tags": [["relay", "wss://relay.example.com"]]
}
```

**Response:**
```json
{
  "ok": true,
  "id": "keypackage_123"
}
```

#### List Key Packages

**GET** `/keypackages?recipient={pubkey}`

Retrieve key packages for a specific recipient.

**Parameters:**
- `recipient` (query): Recipient's public key (hex or bech32)

**Response:**
```json
{
  "ok": true,
  "items": [
    {
      "id": "keypackage_123",
      "recipient_pubkey": "hex_pubkey",
      "sender_pubkey": "hex_pubkey",
      "content_b64": "base64_encoded_key_package",
      "created_at": 1640995200,
      "expires_at": 1641600000,
      "picked_up_at": null
    }
  ]
}
```

#### Acknowledge Key Package

**POST** `/keypackages/{id}/ack`

Acknowledge receipt of a key package.

**Parameters:**
- `id` (path): Key package ID

**Request Body:**
```json
{
  "recipient": "npub1...",
  "sig": "signature_proving_ownership"
}
```

**Response:**
```json
{
  "ok": true
}
```

### Welcome Message Mailbox

#### Store Welcome Message

**POST** `/welcome`

Store a welcome message for delivery to a new group member.

**Request Body:**
```json
{
  "recipient": "npub1...",
  "sender": "npub1...",
  "group_id": "group123",
  "welcome_b64": "base64_encoded_welcome_message"
}
```

**Response:**
```json
{
  "ok": true,
  "id": "welcome_123"
}
```

#### List Welcome Messages

**GET** `/welcome?recipient={pubkey}`

Retrieve welcome messages for a specific recipient.

**Parameters:**
- `recipient` (query): Recipient's public key (hex or bech32)
- `limit` (query, optional): Maximum number of messages (default: 20)

**Response:**
```json
{
  "ok": true,
  "items": [
    {
      "id": "welcome_123",
      "recipient_pubkey": "hex_pubkey",
      "sender_pubkey": "hex_pubkey",
      "group_id": "group123",
      "welcome_b64": "base64_encoded_welcome_message",
      "created_at": 1640995200,
      "expires_at": 1641600000,
      "picked_up_at": null
    }
  ]
}
```

#### Acknowledge Welcome Message

**POST** `/welcome/{id}/ack`

Acknowledge receipt of a welcome message.

**Parameters:**
- `id` (path): Welcome message ID

**Request Body:**
```json
{
  "recipient": "npub1...",
  "sig": "signature_proving_ownership"
}
```

**Response:**
```json
{
  "ok": true
}
```

### Message Archive

#### Get Missed Messages

**POST** `/messages/missed`

Retrieve missed messages for offline delivery.

**Request Body:**
```json
{
  "pubkey": "hex_pubkey",
  "since": 1640995200,
  "limit": 100
}
```

**Parameters:**
- `pubkey`: User's public key (hex)
- `since`: Unix timestamp to retrieve messages from
- `limit` (optional): Maximum messages to return (default: 100, max: 500)

**Response:**
```json
{
  "messages": [
    {
      "id": "event_hex_id",
      "kind": 445,
      "content": "encrypted_mls_message",
      "tags": [["h", "group123"], ["e", "epoch1"]],
      "created_at": 1640995200,
      "pubkey": "hex_pubkey",
      "sig": "hex_signature"
    }
  ],
  "count": 42,
  "has_more": true
}
```

## Error Responses

All endpoints return errors in a consistent format:

```json
{
  "error": "Error description",
  "code": "ERROR_CODE"
}
```

### Common Error Codes

- `400 Bad Request`: Invalid request parameters
- `401 Unauthorized`: Authentication required
- `403 Forbidden`: Insufficient permissions
- `404 Not Found`: Resource not found
- `429 Too Many Requests`: Rate limit exceeded
- `500 Internal Server Error`: Server error

## Rate Limiting

The API implements rate limiting to prevent abuse:

- **Unauthenticated requests**: 100 requests per minute
- **Authenticated requests**: 1000 requests per minute
- **Key package storage**: 50 packages per minute per sender
- **Message retrieval**: 10 requests per minute per user

Rate limit headers are included in responses:
- `X-RateLimit-Limit`: Maximum requests per minute
- `X-RateLimit-Remaining`: Remaining requests
- `X-RateLimit-Reset`: Time until reset (Unix timestamp)

## Data Formats

### Public Keys

- **Hex format**: 64-character lowercase hex string
- **Bech32 format**: `npub1...` format supported in query parameters

### Timestamps

- **Unix timestamps**: Integer seconds since epoch
- **ISO 8601**: String format with timezone (in responses)

### Base64 Content

- **Key packages and welcome messages**: Base64-encoded binary data
- **Padding**: Standard base64 padding included
- **Encoding**: URL-safe variant used

## Security Considerations

### Transport Security
- All API endpoints require HTTPS
- TLS 1.3 recommended
- Certificate pinning recommended for mobile clients

### Authentication Security
- Use strong, unique credentials for service accounts
- Rotate credentials regularly
- Implement IP allowlisting for production deployments

### Data Privacy
- Key packages and welcome messages are end-to-end encrypted
- Server cannot decrypt message contents
- Implement proper access logging for compliance

## Examples

### Store Key Package (cURL)

```bash
curl -X POST https://relay.example.com/api/v1/keypackages \
  -H "Authorization: Basic $(echo -n 'username:password' | base64)" \
  -H "Content-Type: application/json" \
  -d '{
    "recipient": "npub1abc...",
    "sender": "npub1xyz...",
    "content_b64": "SGVsbG8gV29ybGQ="
  }'
```

### Retrieve Key Packages (cURL)

```bash
curl "https://relay.example.com/api/v1/keypackages?recipient=npub1abc..." \
  -H "Authorization: Basic $(echo -n 'username:password' | base64)"
```

### Get Missed Messages (cURL)

```bash
curl -X POST https://relay.example.com/api/v1/messages/missed \
  -H "Authorization: Basic $(echo -n 'username:password' | base64)" \
  -H "Content-Type: application/json" \
  -d '{
    "pubkey": "hex_pubkey_here",
    "since": 1640995200,
    "limit": 50
  }'
```

## Integration Guide

### Client Implementation

1. **Discover relay capabilities**: Check for MLS Gateway support
2. **Authenticate**: Obtain and use service credentials
3. **Store key packages**: Upload key packages for group invitations
4. **Poll for messages**: Regularly check for new key packages and welcome messages
5. **Handle offline delivery**: Use message archive for missed messages

### Best Practices

1. **Connection pooling**: Reuse HTTP connections for multiple requests
2. **Exponential backoff**: Implement retry logic with exponential backoff
3. **Batch operations**: Combine multiple operations when possible
4. **Error handling**: Implement comprehensive error handling and logging
5. **Rate limiting**: Respect rate limits and implement client-side throttling

## Version History

- **v1.0.0** (Current): Initial API release with core MLS gateway functionality
  - Group registry endpoints
  - Key package mailbox
  - Welcome message delivery
  - Message archive for offline delivery