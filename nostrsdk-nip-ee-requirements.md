# NostrSDK NIP-EE Support Requirements

## Overview

We need to enhance NostrSDK to support NIP-EE (MLS over Nostr) functionality for keypackage management. Our relay `wss://messaging.loxation.com` is NIP-EE compliant and manages keypackage lifecycle server-side.

## Required Functionality

### 1. Query Support for Kind 443 Events

We need the ability to query and retrieve kind 443 (MLS KeyPackage) events from the relay.

```swift
// Proposed API
protocol NostrTransport {
    /// Query events from relay with a timeout
    /// - Parameters:
    ///   - filter: Filter criteria for events
    ///   - timeout: Maximum time to wait for results
    /// - Returns: Array of matching events
    func query(filter: Filter, timeout: TimeInterval) async -> [NostrEvent]
}
```

#### Filter Requirements
- Support filtering by kind (443 for keypackages)
- Support filtering by author (pubkey)
- Support filtering by creation time (since)
- Support result limits

### 2. Subscription Enhancement for Kind 443

Add a dedicated subscription method for keypackage events:

```swift
extension NostrTransport {
    /// Subscribe to keypackage events from specific authors
    /// - Parameters:
    ///   - authors: List of pubkeys to monitor for keypackages
    ///   - since: Timestamp to start from (nil for all)
    ///   - onEvent: Callback when keypackage event is received
    /// - Returns: Subscription ID for later unsubscribe
    @discardableResult
    func subscribeKeyPackages(
        authors: [String],
        since: Int64?,
        onEvent: @escaping @Sendable (String) -> Void
    ) -> String
}
```

### 3. Event Kind Support

Extend the `NostrProtocol.EventKind` enum to include NIP-EE kinds:

```swift
enum EventKind: Int {
    // Existing kinds...
    
    // NIP-EE kinds
    case mlsKeyPackage = 443
    case mlsWelcome = 444  
    case mlsGroupMessage = 445
    case keyPackageRequest = 447
    case rosterPolicy = 450
    
    // NIP-EE management
    case keyPackageRelayList = 10051  // Optional for multi-relay setups
}
```

### 4. Relay Lifecycle Management Features

Since `messaging.loxation.com` manages keypackage lifecycle:

```swift
struct RelayCapabilities {
    /// Indicates if relay manages keypackage consumption tracking
    let managesKeyPackageLifecycle: Bool = true
    
    /// Indicates if relay enforces per-user keypackage limits
    let enforcesKeyPackageLimits: Bool = true
    
    /// Maximum keypackages per user (if enforced)
    let maxKeyPackagesPerUser: Int? = 10
}
```

## Use Cases

### 1. Supply Monitoring
```swift
// Check how many keypackages are available on relay
let filter = Filter(
    kinds: [443],
    authors: [myPubkey],
    since: Int64(Date().addingTimeInterval(-90 * 24 * 60 * 60).timeIntervalSince1970)
)
let myKeyPackages = await transport.query(filter: filter, timeout: 5.0)
let currentSupply = myKeyPackages.count
```

### 2. Fetching Keypackages for Group Creation
```swift
// Fetch latest keypackage for each group member
let memberKeyPackages = await withTaskGroup(of: (String, NostrEvent?).self) { group in
    for memberPubkey in groupMembers {
        group.addTask {
            let filter = Filter(
                kinds: [443],
                authors: [memberPubkey],
                limit: 1
            )
            let events = await transport.query(filter: filter, timeout: 2.0)
            return (memberPubkey, events.first)
        }
    }
    
    var results: [String: String] = [:]
    for await (pubkey, event) in group {
        if let kp = event {
            results[pubkey] = kp.content
        }
    }
    return results
}
```

### 3. Publishing Keypackages
```swift
// Existing functionality should work, just need proper event construction
let keyPackageEvent = NostrEvent(
    pubkey: myPubkey,
    createdAt: Date(),
    kind: .mlsKeyPackage,
    tags: [
        ["mls_protocol_version", "1.0"],
        ["ciphersuite", "0x0001"],
        ["extensions", "0x0001,0x0002"],
        ["client", "loxation-ios", handlerEventId, "wss://messaging.loxation.com"],
        ["relays", "wss://messaging.loxation.com"],
        ["-"]  // NIP-70 auth
    ],
    content: keyPackageBase64
)
```

## Relay-Specific Behavior

`messaging.loxation.com` handles:
- Tracking which keypackages have been consumed
- Removing consumed keypackages from query results
- Enforcing per-user limits
- Expiring old keypackages automatically

The client only needs to:
- Monitor supply levels
- Publish new keypackages when supply is low
- Trust the relay to return only valid/unconsumed keypackages

## Priority

High priority items:
1. Query support with filters (essential for supply checking and fetching)
2. Kind 443 in EventKind enum
3. Basic subscription support for keypackages

Medium priority:
- Relay capability detection
- Enhanced subscription methods

Low priority:
- Kind 10051 support (not needed for single-relay setup)