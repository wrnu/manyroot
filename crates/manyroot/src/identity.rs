//! Agent identities: an ed25519 keypair plus a local, human-friendly name.
//!
//! The name never leaves this machine; other replicas see only the public key.

use std::fs;
use std::io::Write;
use std::path::Path;

use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::id::{AgentId, ContributionId, Signature};

/// Domain-separation prefix for contribution signatures.
const SIGNING_CONTEXT: &[u8] = b"manyroot/v1/contribution:";

#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    #[error("invalid identity name {0:?}: use 1-64 characters from [a-z0-9_-]")]
    InvalidName(String),
    #[error("could not generate key: {0}")]
    Random(getrandom::Error),
    #[error("identity file {path}")]
    Io { path: String, source: std::io::Error },
    #[error("identity file {path} is malformed: {reason}")]
    Malformed { path: String, reason: String },
}

pub struct Identity {
    name: String,
    key: SigningKey,
}

#[derive(Serialize, Deserialize)]
struct IdentityFile {
    name: String,
    public_key: AgentId,
    secret_key: String,
}

pub fn validate_name(name: &str) -> Result<(), IdentityError> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-');
    if ok { Ok(()) } else { Err(IdentityError::InvalidName(name.to_owned())) }
}

impl Identity {
    pub fn generate(name: &str) -> Result<Self, IdentityError> {
        validate_name(name)?;
        let mut secret = [0u8; 32];
        getrandom::fill(&mut secret).map_err(IdentityError::Random)?;
        Ok(Self { name: name.to_owned(), key: SigningKey::from_bytes(&secret) })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn agent_id(&self) -> AgentId {
        AgentId(self.key.verifying_key().to_bytes())
    }

    pub fn sign(&self, id: &ContributionId) -> Signature {
        Signature(self.key.sign(&signing_message(id)).to_bytes())
    }

    pub fn load(path: &Path) -> Result<Self, IdentityError> {
        let p = path.display().to_string();
        let text = fs::read_to_string(path).map_err(|source| IdentityError::Io { path: p.clone(), source })?;
        let malformed = |reason: String| IdentityError::Malformed { path: p.clone(), reason };
        let file: IdentityFile = serde_json::from_str(&text).map_err(|e| malformed(e.to_string()))?;
        let mut secret = [0u8; 32];
        hex::decode_to_slice(&file.secret_key, &mut secret).map_err(|e| malformed(e.to_string()))?;
        let identity = Self { name: file.name, key: SigningKey::from_bytes(&secret) };
        if identity.agent_id() != file.public_key {
            return Err(malformed("public key does not match secret key".into()));
        }
        Ok(identity)
    }

    /// Writes the identity file, refusing to overwrite an existing one.
    pub fn save(&self, path: &Path) -> Result<(), IdentityError> {
        let io = |source| IdentityError::Io { path: path.display().to_string(), source };
        let file = IdentityFile {
            name: self.name.clone(),
            public_key: self.agent_id(),
            secret_key: hex::encode(self.key.to_bytes()),
        };
        let json = serde_json::to_string_pretty(&file).expect("identity serializes");

        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut f = options.open(path).map_err(io)?;
        f.write_all(json.as_bytes()).map_err(io)?;
        f.write_all(b"\n").map_err(io)
    }
}

/// Checks that `signature` is `author`'s signature over `id`.
pub fn verify(author: &AgentId, id: &ContributionId, signature: &Signature) -> bool {
    let Ok(key) = VerifyingKey::from_bytes(author.as_bytes()) else {
        return false;
    };
    let sig = ed25519_dalek::Signature::from_bytes(&signature.0);
    key.verify_strict(&signing_message(id), &sig).is_ok()
}

fn signing_message(id: &ContributionId) -> Vec<u8> {
    let mut msg = Vec::with_capacity(SIGNING_CONTEXT.len() + ContributionId::LEN);
    msg.extend_from_slice(SIGNING_CONTEXT);
    msg.extend_from_slice(id.as_bytes());
    msg
}
