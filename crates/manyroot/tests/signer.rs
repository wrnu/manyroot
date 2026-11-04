//! Signing through the [`Signer`] trait (docs/design/agent-keys.md, 4.1): a contribution signed through it
//! is byte for byte the one a local identity signs, content travels in canonical encoding only, and a
//! signer's answer is checked before it's used.

use manyroot::contribution::DecodeError;
use manyroot::*;

fn ground() -> GroundId {
    GroundId([7; 32])
}

fn claim(text: &str) -> Body {
    Body::Claim { statement: text.into(), tags: vec!["note".into()] }
}

/// A signer that holds its key elsewhere, as a server does: here, an identity it reaches only through the
/// content it's sent, which it decodes first as a server would.
struct Remote(Identity);

impl Signer for Remote {
    fn agent_id(&self) -> AgentId {
        self.0.agent_id()
    }

    fn sign_content(&self, content: &ContributionContent, id: &ContributionId) -> Result<Signature, SignError> {
        let sent = ContributionContent::decode(&content.encode()).map_err(|e| SignError(e.to_string()))?;
        assert_eq!(&sent.id(), id);
        Ok(self.0.sign(&sent.id()))
    }
}

/// Signs with one key but claims another's.
struct Wrong(Identity, AgentId);

impl Signer for Wrong {
    fn agent_id(&self) -> AgentId {
        self.1
    }

    fn sign_content(&self, _: &ContributionContent, id: &ContributionId) -> Result<Signature, SignError> {
        Ok(self.0.sign(id))
    }
}

struct Refusing;

impl Signer for Refusing {
    fn agent_id(&self) -> AgentId {
        AgentId([1; 32])
    }

    fn sign_content(&self, _: &ContributionContent, _: &ContributionId) -> Result<Signature, SignError> {
        Err(SignError("outside this identity's scope".into()))
    }
}

#[test]
fn a_contribution_signed_through_a_signer_is_the_one_its_identity_signs() {
    let who = Identity::generate("signer").unwrap();
    let parent = Contribution::create(ground(), &who, [], 1, claim("first"));
    let local = Contribution::create(ground(), &who, [parent.id()], 2, claim("second"));
    let through = Contribution::create_with(ground(), &Remote(who), [parent.id()], 2, claim("second")).unwrap();
    assert_eq!(through.encode(), local.encode());
    assert!(through.signature_is_valid());
    let decoded = Contribution::decode(&through.encode()).unwrap();
    assert_eq!(decoded.id(), local.id());

    let identity = Identity::generate("direct").unwrap();
    let direct = Contribution::create_with(ground(), &identity, [], 3, claim("third")).unwrap();
    assert_eq!(direct.encode(), Contribution::create(ground(), &identity, [], 3, claim("third")).encode());
}

#[test]
fn a_signers_bad_signature_or_refusal_makes_no_contribution() {
    let (a, b) = (Identity::generate("a").unwrap(), Identity::generate("b").unwrap());
    let err = Contribution::create_with(ground(), &Wrong(a, b.agent_id()), [], 1, claim("x")).unwrap_err();
    assert!(err.to_string().contains("doesn't verify"), "{err}");
    let err = Contribution::create_with(ground(), &Refusing, [], 1, claim("x")).unwrap_err();
    assert_eq!(err.to_string(), "outside this identity's scope");
}

#[test]
fn content_decodes_from_its_canonical_encoding_only() {
    let who = Identity::generate("c").unwrap();
    let c = Contribution::create(ground(), &who, [], 5, claim("canonical"));
    let bytes = c.content().encode();
    let back = ContributionContent::decode(&bytes).unwrap();
    assert_eq!(&back, c.content());
    assert_eq!(back.id(), c.id());

    // Trailing bytes, and a whole contribution (content and signature) in place of its content.
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(matches!(ContributionContent::decode(&trailing), Err(DecodeError::NotCanonical)));
    assert!(ContributionContent::decode(&c.encode()).is_err());
    // The timestamp, 5, as a two-byte integer: the same value in a longer encoding.
    let longer = reencode_timestamp(&bytes);
    assert_ne!(longer, bytes);
    assert!(matches!(ContributionContent::decode(&longer), Err(DecodeError::NotCanonical)));
}

/// The content with its `timestamp_ms` value, the small integer 5 (one byte, 0x05), written as a
/// two-byte integer (0x18 0x05) instead: valid CBOR for the same value, but not canonical.
fn reencode_timestamp(bytes: &[u8]) -> Vec<u8> {
    let key = b"timestamp_ms";
    let at = bytes.windows(key.len()).position(|w| w == key).expect("a timestamp_ms key") + key.len();
    assert_eq!(bytes[at], 0x05);
    let mut out = bytes[..at].to_vec();
    out.extend_from_slice(&[0x18, 0x05]);
    out.extend_from_slice(&bytes[at + 1..]);
    out
}
