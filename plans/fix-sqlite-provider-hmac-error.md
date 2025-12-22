# Fix for SqliteProvider HMAC Error

## Problem Summary

The `SqliteProvider` implementation in `../loxation-mls/rust/src/sqlite_provider.rs` is missing the required `hmac` method from the `OpenMlsCrypto` trait, causing a compilation error at line 363.

## Root Cause

The OpenMLS library (using the main branch) has added a new `hmac` method to the `OpenMlsCrypto` trait, but the `SqliteProvider` implementation hasn't been updated to include this method.

## Solution

Add the missing `hmac` method to the `SqliteProvider` implementation by delegating to the internal `crypto_provider` (which is of type `OpenMlsRustCrypto`).

### Method Signature

The required method signature from the `OpenMlsCrypto` trait is:

```rust
fn hmac(
    &self,
    hash_type: HashType,
    key: &[u8],
    message: &[u8],
) -> Result<SecretVLBytes, CryptoError>;
```

### Implementation

The implementation should follow the same pattern as other crypto methods in `SqliteProvider`, delegating to the `crypto_provider`:

```rust
fn hmac(
    &self,
    hash_type: HashType,
    key: &[u8],
    message: &[u8],
) -> Result<SecretVLBytes, CryptoError> {
    self.crypto_provider.crypto().hmac(hash_type, key, message)
}
```

### Insertion Location

The method should be added to the `impl OpenMlsCrypto for SqliteProvider` block, which starts at line 363. Based on the pattern observed in the file, it should be placed after the `hkdf_expand` method (around line 388-389).

## Implementation Steps

1. Open `../loxation-mls/rust/src/sqlite_provider.rs`
2. Locate the `impl OpenMlsCrypto for SqliteProvider` block (line 363)
3. Find a suitable location within the implementation (after `hkdf_expand` method)
4. Add the `hmac` method implementation
5. Save the file
6. Run `cargo check` or `cargo build` to verify the fix

## Testing

After implementation:
1. Ensure the compilation error is resolved
2. Run any existing tests for the SqliteProvider
3. Consider adding a test specifically for the HMAC functionality if not already covered

## Related Considerations

- This fix addresses compatibility with the latest OpenMLS main branch
- No changes to the `SqliteProvider` struct or its fields are required
- The delegation pattern maintains consistency with other method implementations in the file