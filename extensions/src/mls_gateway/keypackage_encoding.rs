//! KeyPackage content encoding helpers.
//!
//! Stage-1 policy:
//! - Ingest accepts hex by default (no `encoding` tag) and base64 when `encoding=base64`.
//! - Firestore stores canonical **standard base64 with padding**.
//! - Delivery defaults to hex (legacy clients).

use anyhow::{anyhow, bail, Result};

use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
use base64::Engine;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclaredEncoding {
    Hex,
    Base64,
}

impl DeclaredEncoding {
    pub fn as_str(&self) -> &'static str {
        match self {
            DeclaredEncoding::Hex => "hex",
            DeclaredEncoding::Base64 => "base64",
        }
    }
}

/// Determine declared encoding from Nostr tags.
///
/// Contract:
/// - Missing `encoding` tag => hex
/// - `encoding=base64` => base64
/// - `encoding=hex` => hex
pub fn declared_encoding_from_tags(tags: &[Vec<String>]) -> Result<DeclaredEncoding> {
    let enc = tags
        .iter()
        .find(|tag| tag.len() >= 2 && tag[0] == "encoding")
        .map(|tag| tag[1].to_lowercase());

    match enc.as_deref() {
        Some("base64") => Ok(DeclaredEncoding::Base64),
        Some("hex") => Ok(DeclaredEncoding::Hex),
        Some(other) => bail!("unsupported encoding tag value: {other}"),
        None => Ok(DeclaredEncoding::Hex),
    }
}

pub fn decode_keypackage_content(content: &str, encoding: DeclaredEncoding) -> Result<Vec<u8>> {
    let c = content.trim();
    if c.is_empty() {
        bail!("empty keypackage content");
    }
    match encoding {
        DeclaredEncoding::Hex => decode_hex(c),
        DeclaredEncoding::Base64 => decode_base64_flexible(c),
    }
}

pub fn decode_hex(s: &str) -> Result<Vec<u8>> {
    hex::decode(s).map_err(|e| anyhow!("hex decode failed: {e}"))
}

/// Decode base64 accepting common variants.
///
/// Accepts:
/// - standard base64 (padded)
/// - standard base64 (no pad)
/// - url-safe base64 (padded)
/// - url-safe base64 (no pad)
pub fn decode_base64_flexible(s: &str) -> Result<Vec<u8>> {
    // Try common engines in a deterministic order.
    if let Ok(b) = STANDARD.decode(s) {
        return Ok(b);
    }
    if let Ok(b) = STANDARD_NO_PAD.decode(s) {
        return Ok(b);
    }
    if let Ok(b) = URL_SAFE.decode(s) {
        return Ok(b);
    }
    if let Ok(b) = URL_SAFE_NO_PAD.decode(s) {
        return Ok(b);
    }
    bail!("base64 decode failed")
}

/// Encode canonical standard base64 (RFC 4648, padded).
pub fn encode_canonical_base64(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

pub fn encode_hex(bytes: &[u8]) -> String {
    hex::encode(bytes)
}

/// Convert incoming event (tags + content) into canonical base64 for Firestore storage.
pub fn canonical_base64_from_event(tags: &[Vec<String>], content: &str) -> Result<(DeclaredEncoding, String)> {
    let declared = declared_encoding_from_tags(tags)?;
    let bytes = decode_keypackage_content(content, declared)?;
    Ok((declared, encode_canonical_base64(&bytes)))
}

/// Decode Firestore `content` which is expected to be canonical base64.
///
/// Safety-net behavior: if base64 decoding fails, attempt hex decoding.
pub fn bytes_from_firestore_content(content: &str) -> Result<Vec<u8>> {
    let c = content.trim();
    if c.is_empty() {
        bail!("empty firestore keypackage content");
    }

    match decode_base64_flexible(c) {
        Ok(b) => Ok(b),
        Err(_) => {
            // Fallback for legacy docs if purge misses anything.
            decode_hex(c)
        }
    }
}

pub fn hex_from_firestore_content(content: &str) -> Result<String> {
    Ok(encode_hex(&bytes_from_firestore_content(content)?))
}

pub fn base64_from_firestore_content(content: &str) -> Result<String> {
    Ok(encode_canonical_base64(&bytes_from_firestore_content(content)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalizes_hex_default_to_base64() {
        let tags: Vec<Vec<String>> = vec![];
        let (declared, b64) = canonical_base64_from_event(&tags, "48656c6c6f").unwrap();
        assert_eq!(declared, DeclaredEncoding::Hex);
        assert_eq!(b64, "SGVsbG8=");
    }

    #[test]
    fn canonicalizes_base64_to_base64() {
        let tags: Vec<Vec<String>> = vec![vec!["encoding".into(), "base64".into()]];
        let (declared, b64) = canonical_base64_from_event(&tags, "SGVsbG8=").unwrap();
        assert_eq!(declared, DeclaredEncoding::Base64);
        assert_eq!(b64, "SGVsbG8=");
    }

    #[test]
    fn accepts_unpadded_base64_on_ingest() {
        let tags: Vec<Vec<String>> = vec![vec!["encoding".into(), "base64".into()]];
        let (_declared, b64) = canonical_base64_from_event(&tags, "SGVsbG8").unwrap();
        // canonicalized with padding
        assert_eq!(b64, "SGVsbG8=");
    }

    #[test]
    fn firestore_base64_to_hex() {
        let hex = hex_from_firestore_content("SGVsbG8=").unwrap();
        assert_eq!(hex, "48656c6c6f");
    }

    #[test]
    fn firestore_hex_fallback_to_hex() {
        let hex = hex_from_firestore_content("48656c6c6f").unwrap();
        assert_eq!(hex, "48656c6c6f");
    }

    #[test]
    fn firestore_hex_fallback_to_base64() {
        let b64 = base64_from_firestore_content("48656c6c6f").unwrap();
        assert_eq!(b64, "SGVsbG8=");
    }
}
