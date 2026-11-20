//! Agent identities: an ed25519 keypair plus a local, human-friendly name.
//!
//! The name never leaves this machine; other replicas see only the public key.

use std::path::Path;

use ed25519_dalek::{Signer as _, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::contribution::ContributionContent;
use crate::fsx::{self, Dir};
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

/// What signs a contribution as one author: a local [`Identity`], or something that holds the key
/// elsewhere, such as a server signing for its credentials. It's handed the content as well as the id,
/// so a signer that holds the key elsewhere can read what it signs before signing it. Every signer signs
/// the same message, `"manyroot/v1/contribution:" || id`; nothing about the encoding changes with it.
pub trait Signer {
    /// The author key the signatures verify under.
    fn agent_id(&self) -> AgentId;

    /// The author's signature over `id`, the id of `content`.
    fn sign_content(&self, content: &ContributionContent, id: &ContributionId) -> Result<Signature, SignError>;
}

/// A signer that couldn't sign, saying why.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct SignError(pub String);

impl Signer for Identity {
    fn agent_id(&self) -> AgentId {
        Identity::agent_id(self)
    }

    fn sign_content(&self, _: &ContributionContent, id: &ContributionId) -> Result<Signature, SignError> {
        Ok(self.sign(id))
    }
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

    /// Reads an identity file, `<name>.json` for the identity it holds. Refuses anything but a regular
    /// file (a symlink included, opened without following it), a name [`validate_name`] refuses, or a
    /// file named for another identity.
    pub fn load(path: &Path) -> Result<Self, IdentityError> {
        let p = path.display().to_string();
        let parent = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        let bytes =
            Dir::open(parent).and_then(|d| d.read(name, 1 << 20)).map_err(|source| match fsx::refusal(&source) {
                Some(reason) => IdentityError::Malformed { path: p.clone(), reason },
                None => IdentityError::Io { path: p.clone(), source },
            })?;
        Self::parse(path, &bytes)
    }

    /// An identity from the bytes of its file at `path`, checking the name as [`Identity::load`] does.
    pub(crate) fn parse(path: &Path, bytes: &[u8]) -> Result<Self, IdentityError> {
        let p = path.display().to_string();
        let malformed = |reason: String| IdentityError::Malformed { path: p.clone(), reason };
        let file: IdentityFile = serde_json::from_slice(bytes).map_err(|e| malformed(e.to_string()))?;
        validate_name(&file.name)?;
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if file_name != format!("{}.json", file.name) {
            let stem = file_name.strip_suffix(".json").unwrap_or(file_name);
            return Err(malformed(format!("holds identity {:?}, not {stem:?}", file.name)));
        }
        let mut secret = [0u8; 32];
        hex::decode_to_slice(&file.secret_key, &mut secret).map_err(|e| malformed(e.to_string()))?;
        let identity = Self { name: file.name, key: SigningKey::from_bytes(&secret) };
        if identity.agent_id() != file.public_key {
            return Err(malformed("public key does not match secret key".into()));
        }
        Ok(identity)
    }

    /// Whether `bytes` holds this identity's private key, as hex in any case or as its raw bytes: the
    /// likely accidents of a key file attached, or a key pasted into text.
    pub fn key_is_in(&self, bytes: &[u8]) -> bool {
        let raw = self.key.to_bytes();
        let hex = hex::encode(raw);
        let lower = bytes.to_ascii_lowercase();
        lower.windows(hex.len()).any(|w| w == hex.as_bytes()) || bytes.windows(raw.len()).any(|w| w == raw)
    }

    /// Writes the identity file, refusing to overwrite an existing one. It is written whole or not at
    /// all: to a 0600 temporary file in the same directory (a dot name without `.json`, so no listing
    /// reads it), synced, then linked into place, never through a symlink at `path`.
    pub fn save(&self, path: &Path) -> Result<(), IdentityError> {
        let io = |source| IdentityError::Io { path: path.display().to_string(), source };
        let parent = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        self.save_in(&Dir::open(parent).map_err(io)?, name)
    }

    /// [`Identity::save`] into the open directory `dir`, as `name`.
    pub(crate) fn save_in(&self, dir: &Dir, name: &str) -> Result<(), IdentityError> {
        let file = IdentityFile {
            name: self.name.clone(),
            public_key: self.agent_id(),
            secret_key: hex::encode(self.key.to_bytes()),
        };
        let json = serde_json::to_string_pretty(&file).expect("identity serializes") + "\n";
        dir.write_new(name, json.as_bytes(), 0o600)
            .map_err(|source| IdentityError::Io { path: dir.join(name).display().to_string(), source })
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
