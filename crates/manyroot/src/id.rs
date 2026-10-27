//! Fixed-size identifiers: grounds, contributions (blake3 hashes), agents
//! (ed25519 public keys) and signatures.
//!
//! Both serialize as raw bytes in binary formats (CBOR) and as lowercase hex in
//! human-readable formats (JSON), so the canonical encoding stays compact while
//! agent-facing output stays legible.

use std::fmt;
use std::str::FromStr;

use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize, Serializer};

/// Number of hex characters shown by `short()`.
pub const SHORT_LEN: usize = 12;

#[derive(Debug, thiserror::Error)]
pub enum ParseIdError {
    #[error("expected {expected} hex characters, got {got}")]
    Length { expected: usize, got: usize },
    #[error("invalid hex: {0}")]
    Hex(#[from] hex::FromHexError),
}

macro_rules! byte_id {
    ($(#[$meta:meta])* $name:ident, $len:expr) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub [u8; $len]);

        impl $name {
            pub const LEN: usize = $len;

            pub fn as_bytes(&self) -> &[u8; $len] {
                &self.0
            }

            pub fn to_hex(&self) -> String {
                hex::encode(self.0)
            }

            /// Abbreviated hex form for display. Not guaranteed unique.
            pub fn short(&self) -> String {
                let mut s = self.to_hex();
                s.truncate(SHORT_LEN);
                s
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.to_hex())
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.short())
            }
        }

        impl FromStr for $name {
            type Err = ParseIdError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                if s.len() != $len * 2 {
                    return Err(ParseIdError::Length { expected: $len * 2, got: s.len() });
                }
                let mut out = [0u8; $len];
                hex::decode_to_slice(s, &mut out)?;
                Ok(Self(out))
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                if serializer.is_human_readable() {
                    serializer.serialize_str(&self.to_hex())
                } else {
                    serializer.serialize_bytes(&self.0)
                }
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                struct V;

                impl<'de> Visitor<'de> for V {
                    type Value = $name;

                    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                        write!(f, "{} bytes or {} hex characters", $len, $len * 2)
                    }

                    fn visit_bytes<E: de::Error>(self, v: &[u8]) -> Result<$name, E> {
                        let arr: [u8; $len] =
                            v.try_into().map_err(|_| E::invalid_length(v.len(), &self))?;
                        Ok($name(arr))
                    }

                    fn visit_str<E: de::Error>(self, v: &str) -> Result<$name, E> {
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
    };
}

byte_id!(
    /// Content address of a contribution: blake3 hash of its canonical encoding.
    ContributionId,
    32
);

byte_id!(
    /// Identifies a Ground: a bounded workspace with its own contribution graph.
    /// Chosen at random when the Ground is created.
    GroundId,
    32
);

byte_id!(
    /// An agent's ed25519 public key. Agents are identified by key, not by name.
    AgentId,
    32
);

byte_id!(
    /// An ed25519 signature over a contribution id.
    Signature,
    64
);
