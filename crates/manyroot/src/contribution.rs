//! Events: the immutable, signed, content-addressed unit of contribution.
//!
//! A contribution's id is the blake3 hash of the canonical CBOR encoding of its
//! [`ContributionContent`]. The author signs the id. Because ids are recomputed from
//! decoded content on every read, a replica never trusts an id it was handed.

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::id::{AgentId, ContributionId, GroundId, Signature};
use crate::identity::{self, Identity, SignError, Signer};

/// Protocol version written into every contribution.
pub const PROTOCOL_VERSION: u32 = 1;

/// Upper bound on an encoded contribution, artifact content included.
pub const MAX_CONTRIBUTION_BYTES: usize = 16 * 1024 * 1024;

/// What a contribution says. Claims and artifacts are the *subjects* of the graph;
/// relations, evidence and retractions are statements *about* other contributions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Body {
    /// An idea, hypothesis, observation, requirement or decision.
    Claim { statement: String, tags: Vec<String> },
    /// A concrete piece of work: source code, a document, a dataset.
    Artifact {
        name: String,
        media_type: String,
        #[serde(with = "serde_bytes")]
        content: Vec<u8>,
    },
    /// A typed link between two contributions, asserted by the relation's author.
    Relation { from: ContributionId, kind: RelationKind, to: ContributionId, note: String },
    /// The outcome of checking a contribution: a test run, a proof check, a review.
    Evidence { target: ContributionId, verdict: Verdict, method: String, details: String },
    /// The author withdraws one of their own contributions. The contribution stays in the
    /// graph; views treat it as withdrawn.
    Retract { target: ContributionId, reason: String },
}

impl Body {
    pub fn kind(&self) -> BodyKind {
        match self {
            Body::Claim { .. } => BodyKind::Claim,
            Body::Artifact { .. } => BodyKind::Artifact,
            Body::Relation { .. } => BodyKind::Relation,
            Body::Evidence { .. } => BodyKind::Evidence,
            Body::Retract { .. } => BodyKind::Retract,
        }
    }

    /// Ids of the contributions this body talks about. These must all be parents.
    pub fn references(&self) -> Vec<ContributionId> {
        match self {
            Body::Claim { .. } | Body::Artifact { .. } => vec![],
            Body::Relation { from, to, .. } => vec![*from, *to],
            Body::Evidence { target, .. } | Body::Retract { target, .. } => vec![*target],
        }
    }

    /// Claims and artifacts can be accepted; the other kinds annotate them.
    pub fn is_subject(&self) -> bool {
        matches!(self, Body::Claim { .. } | Body::Artifact { .. })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BodyKind {
    Claim,
    Artifact,
    Relation,
    Evidence,
    Retract,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationKind {
    /// `from` is evidence or argument for `to`.
    Supports,
    /// `from` and `to` cannot both hold.
    Contradicts,
    /// `from` needs `to` to make sense or to work.
    DependsOn,
    /// `from` is a narrower or more precise version of `to`.
    Refines,
    /// `from` replaces `to`. Only counts when asserted by `from`'s author.
    Supersedes,
    /// `from` (an artifact) realizes `to` (usually a claim).
    Implements,
    /// `from` (usually an artifact) checks `to`.
    Tests,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Pass,
    Fail,
}

macro_rules! snake_case_str {
    ($ty:ty { $($variant:ident => $s:literal),+ $(,)? }) => {
        impl $ty {
            pub const ALL: &'static [$ty] = &[$(<$ty>::$variant),+];

            pub fn as_str(&self) -> &'static str {
                match self { $(<$ty>::$variant => $s),+ }
            }
        }

        impl fmt::Display for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.pad(self.as_str())
            }
        }

        impl FromStr for $ty {
            type Err = String;

            fn from_str(s: &str) -> Result<Self, String> {
                match s.replace('-', "_").as_str() {
                    $($s => Ok(<$ty>::$variant),)+
                    _ => {
                        let options: Vec<&str> = vec![$($s),+];
                        Err(format!("unknown value {s:?}, expected one of: {}", options.join(", ")))
                    }
                }
            }
        }
    };
}

snake_case_str!(BodyKind {
    Claim => "claim",
    Artifact => "artifact",
    Relation => "relation",
    Evidence => "evidence",
    Retract => "retract",
});

snake_case_str!(RelationKind {
    Supports => "supports",
    Contradicts => "contradicts",
    DependsOn => "depends_on",
    Refines => "refines",
    Supersedes => "supersedes",
    Implements => "implements",
    Tests => "tests",
});

snake_case_str!(Verdict { Pass => "pass", Fail => "fail" });

/// The signed part of a contribution.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContributionContent {
    pub protocol: u32,
    /// The Ground this contribution belongs to. Binding it into the signed
    /// content stops a contribution being replayed into another Ground.
    pub ground: GroundId,
    pub author: AgentId,
    /// Causal dependencies, strictly ascending. Everything the author had
    /// seen when writing, and at least every contribution the body references.
    pub parents: Vec<ContributionId>,
    /// Advisory wall-clock time in milliseconds since the Unix epoch. Never
    /// used for ordering; causality comes from `parents`.
    pub timestamp_ms: u64,
    pub body: Body,
}

impl ContributionContent {
    /// The content of a new contribution: `parents` in any order, with duplicates, and every id the body
    /// references added.
    fn assemble(
        ground: GroundId,
        author: AgentId,
        parents: impl IntoIterator<Item = ContributionId>,
        timestamp_ms: u64,
        body: Body,
    ) -> ContributionContent {
        let mut set: BTreeSet<ContributionId> = parents.into_iter().collect();
        set.extend(body.references());
        ContributionContent {
            protocol: PROTOCOL_VERSION,
            ground,
            author,
            parents: set.into_iter().collect(),
            timestamp_ms,
            body,
        }
    }

    pub fn id(&self) -> ContributionId {
        ContributionId(*blake3::hash(&self.encode()).as_bytes())
    }

    /// The canonical encoding, whose blake3 hash is the id: what a remote signer is sent.
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        ciborium::into_writer(self, &mut bytes).expect("writing to a Vec cannot fail");
        bytes
    }

    /// Decodes content in canonical encoding only, as [`Contribution::decode`] does a contribution: bytes
    /// that don't re-encode to themselves are refused, so the id a signer computes is the one every
    /// replica will.
    pub fn decode(bytes: &[u8]) -> Result<ContributionContent, DecodeError> {
        if bytes.len() > MAX_CONTRIBUTION_BYTES {
            return Err(DecodeError::TooLarge(bytes.len()));
        }
        let content: ContributionContent =
            ciborium::from_reader(bytes).map_err(|e| DecodeError::Malformed(e.to_string()))?;
        if content.encode() != bytes {
            return Err(DecodeError::NotCanonical);
        }
        Ok(content)
    }
}

/// On-disk and on-wire form.
#[derive(Serialize, Deserialize)]
struct WireContribution {
    content: ContributionContent,
    signature: Signature,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Contribution {
    id: ContributionId,
    content: ContributionContent,
    signature: Signature,
}

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("contribution is {0} bytes, over the {MAX_CONTRIBUTION_BYTES} byte limit")]
    TooLarge(usize),
    #[error("malformed contribution: {0}")]
    Malformed(String),
    #[error("contribution is not in canonical encoding")]
    NotCanonical,
}

impl Contribution {
    /// Builds and signs a contribution. `parents` may be in any order and contain
    /// duplicates; every id the body references is added automatically.
    pub fn create(
        ground: GroundId,
        identity: &Identity,
        parents: impl IntoIterator<Item = ContributionId>,
        timestamp_ms: u64,
        body: Body,
    ) -> Contribution {
        let content = ContributionContent::assemble(ground, identity.agent_id(), parents, timestamp_ms, body);
        let id = content.id();
        let signature = identity.sign(&id);
        Contribution { id, content, signature }
    }

    /// [`Contribution::create`] through any [`Signer`], such as one whose key is held elsewhere. Fails if
    /// the signer does, or if what it returns isn't a valid signature by its author key over the id.
    pub fn create_with(
        ground: GroundId,
        signer: &dyn Signer,
        parents: impl IntoIterator<Item = ContributionId>,
        timestamp_ms: u64,
        body: Body,
    ) -> Result<Contribution, SignError> {
        let content = ContributionContent::assemble(ground, signer.agent_id(), parents, timestamp_ms, body);
        let id = content.id();
        let signature = signer.sign_content(&content, &id)?;
        let c = Contribution { id, content, signature };
        if !c.signature_is_valid() {
            return Err(SignError("the signer returned a signature that doesn't verify under its author key".into()));
        }
        Ok(c)
    }

    pub fn id(&self) -> ContributionId {
        self.id
    }

    pub fn content(&self) -> &ContributionContent {
        &self.content
    }

    pub fn ground(&self) -> GroundId {
        self.content.ground
    }

    pub fn author(&self) -> AgentId {
        self.content.author
    }

    pub fn parents(&self) -> &[ContributionId] {
        &self.content.parents
    }

    pub fn body(&self) -> &Body {
        &self.content.body
    }

    pub fn signature(&self) -> &Signature {
        &self.signature
    }

    pub fn signature_is_valid(&self) -> bool {
        identity::verify(&self.content.author, &self.id, &self.signature)
    }

    pub fn encode(&self) -> Vec<u8> {
        let wire = WireContribution { content: self.content.clone(), signature: self.signature };
        let mut bytes = Vec::new();
        ciborium::into_writer(&wire, &mut bytes).expect("writing to a Vec cannot fail");
        bytes
    }

    /// Decodes a contribution in canonical encoding and recomputes its id. Does not check the signature
    /// or any graph rules; see [`crate::graph::Graph::check`].
    pub fn decode(bytes: &[u8]) -> Result<Contribution, DecodeError> {
        if bytes.len() > MAX_CONTRIBUTION_BYTES {
            return Err(DecodeError::TooLarge(bytes.len()));
        }
        let wire: WireContribution = ciborium::from_reader(bytes).map_err(|e| DecodeError::Malformed(e.to_string()))?;
        let c = Contribution { id: wire.content.id(), content: wire.content, signature: wire.signature };
        // Exactly one byte string per contribution: no trailing data and no
        // alternative CBOR encodings of the same value.
        if c.encode() != bytes {
            return Err(DecodeError::NotCanonical);
        }
        Ok(c)
    }
}
