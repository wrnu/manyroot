//! Content hashes: BLAKE3 of a payload's bytes, and nothing else.
//!
//! A content hash is not a [`manyroot::ContributionId`], though both are 32 bytes
//! and both print as 64 hex digits (docs/design/weave.md, *Content hash and
//! ContributionId*). So in text a content hash is always labelled
//! `blake3:<64 hex>`, in output and in input, and a bare 64 hex digits is refused
//! here: it is a ContributionId. Inside payloads it is a 32-byte byte string, with
//! no label.

use std::fmt;
use std::str::FromStr;

use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize, Serializer};

/// The label every content hash carries in text.
pub const PREFIX: &str = "blake3:";

/// BLAKE3 of a payload's bytes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ContentHash(pub [u8; 32]);

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum ParseHashError {
    #[error("a content hash is written blake3:<64 hex digits>; a bare 64 hex digits is a contribution id")]
    Unlabelled,
    #[error("expected blake3:<64 hex digits>")]
    Malformed,
}

impl ContentHash {
    pub fn of(bytes: &[u8]) -> ContentHash {
        ContentHash(*blake3::hash(bytes).as_bytes())
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// The 64 hex digits, without the label.
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{PREFIX}{}", self.to_hex())
    }
}

impl fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ContentHash({PREFIX}{})", &self.to_hex()[..manyroot::id::SHORT_LEN])
    }
}

fn parse_hex(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return None;
    }
    let mut out = [0u8; 32];
    hex::decode_to_slice(s, &mut out).ok()?;
    Some(out)
}

impl FromStr for ContentHash {
    type Err = ParseHashError;

    fn from_str(s: &str) -> Result<ContentHash, ParseHashError> {
        match s.strip_prefix(PREFIX) {
            Some(rest) => parse_hex(rest).map(ContentHash).ok_or(ParseHashError::Malformed),
            None if parse_hex(&s.to_ascii_lowercase()).is_some() => Err(ParseHashError::Unlabelled),
            None => Err(ParseHashError::Malformed),
        }
    }
}

impl Serialize for ContentHash {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if serializer.is_human_readable() {
            serializer.serialize_str(&self.to_string())
        } else {
            serializer.serialize_bytes(&self.0)
        }
    }
}

impl<'de> Deserialize<'de> for ContentHash {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<ContentHash, D::Error> {
        struct V;

        impl<'de> Visitor<'de> for V {
            type Value = ContentHash;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("32 bytes, or blake3:<64 hex digits>")
            }

            fn visit_bytes<E: de::Error>(self, v: &[u8]) -> Result<ContentHash, E> {
                let arr: [u8; 32] = v.try_into().map_err(|_| E::invalid_length(v.len(), &self))?;
                Ok(ContentHash(arr))
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<ContentHash, E> {
                v.parse().map_err(E::custom)
            }
        }

        if deserializer.is_human_readable() {
            deserializer.deserialize_str(V)
        } else {
            deserializer.deserialize_bytes(V)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_form_is_labelled_and_bare_hex_is_refused() {
        let h = ContentHash::of(b"hello");
        let text = h.to_string();
        assert!(text.starts_with("blake3:"));
        assert_eq!(text.parse::<ContentHash>(), Ok(h));
        assert_eq!(h.to_hex().parse::<ContentHash>(), Err(ParseHashError::Unlabelled));
        assert_eq!(h.to_hex().to_uppercase().parse::<ContentHash>(), Err(ParseHashError::Unlabelled));
        assert_eq!(
            format!("blake3:{}", h.to_hex().to_uppercase()).parse::<ContentHash>(),
            Err(ParseHashError::Malformed)
        );
        assert_eq!("blake3:abc".parse::<ContentHash>(), Err(ParseHashError::Malformed));
    }

    #[test]
    fn json_uses_the_label() {
        let h = ContentHash::of(b"x");
        let json = serde_json::to_string(&h).expect("serializing a hash cannot fail");
        assert_eq!(json, format!("\"{h}\""));
        assert_eq!(serde_json::from_str::<ContentHash>(&json).expect("round-trips"), h);
    }
}
