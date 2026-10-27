//! Protocol guard: golden vectors, negative vectors, a bundle vector and a
//! fixture Ground, pinned to bytes written by version 0.1.0 of `manyroot`.
//!
//! If a test here fails, the change you made alters the protocol or the
//! storage format, and every existing Ground (this repo's own `.bracken/`
//! included) may no longer open. Don't update the pinned values to make the
//! test pass. A protocol change needs independent review, the owner's go and
//! a migration note in `docs/releases.md`; see "Protocol guard" in
//! `docs/protocol.md`.

use std::path::{Path, PathBuf};

use manyroot::validation::{Conflict, Policy, Status, open_conflicts, open_supersessors};
use manyroot::*;

/// The fixture Ground, checked in. Only `write_ground_v1_fixture` writes it.
fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ground-v1")
}

const GROUND: GroundId = GroundId([0x67; 32]);

/// Fixed test keys: (name, secret seed, the public key it must derive).
const ALICE: (&str, [u8; 32], &str) =
    ("alice", [0xa1; 32], "bc7cbcb5636375fa1d82434d466724d92377f53b980695dd49d26d0ce12205a5");
const BOB: (&str, [u8; 32], &str) =
    ("bob", [0xb0; 32], "705fbac01f5519899f437bc42e40255ae9ab54bff00de3433af7d687d9e71ad5");
const CAROL: (&str, [u8; 32], &str) =
    ("carol", [0xc0; 32], "5b9437adeaffbe8f41b13d96ed49d2f51cd6c266cd8ecc284b0552ec4912b8dd");

/// Builds an identity from a fixed seed through the public API: an identity
/// file in a scratch directory. `Identity::load` rejects the file unless the
/// seed derives the pinned public key, so key derivation is pinned too.
fn identity((name, seed, public_key): (&str, [u8; 32], &str)) -> Identity {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join(format!("{name}.json"));
    let file = serde_json::json!({ "name": name, "public_key": public_key, "secret_key": hex::encode(seed) });
    std::fs::write(&path, file.to_string()).unwrap();
    let identity = Identity::load(&path).expect("the seed derives the pinned public key");
    assert_eq!(identity.agent_id().to_hex(), public_key);
    identity
}

fn claim(statement: &str, tags: &[&str]) -> Body {
    Body::Claim { statement: statement.into(), tags: tags.iter().map(|t| t.to_string()).collect() }
}

fn relate(from: ContributionId, kind: RelationKind, to: ContributionId, note: &str) -> Body {
    Body::Relation { from, kind, to, note: note.into() }
}

fn evidence(target: ContributionId, verdict: Verdict, method: &str, details: &str) -> Body {
    Body::Evidence { target, verdict, method: method.into(), details: details.into() }
}

// ---------------------------------------------------------------------------
// Golden vectors: one contribution of every body kind.
// ---------------------------------------------------------------------------

/// Pinned encodings, ids and signatures, in the order `vector_contributions`
/// builds them.
struct Vector {
    kind: BodyKind,
    encoding: &'static str,
    id: &'static str,
    signature: &'static str,
}

const VECTORS: [Vector; 5] = [
    Vector {
        kind: BodyKind::Claim,
        encoding: "a267636f6e74656e74a66870726f746f636f6c016667726f756e645820676767676767676767676767676767676767676767676767676767676767676766617574686f725820bc7cbcb5636375fa1d82434d466724d92377f53b980695dd49d26d0ce12205a567706172656e7473806c74696d657374616d705f6d731b000001a17720ca0064626f6479a165636c61696da26973746174656d656e74781c746f6b656e7320657870697265206166746572206f6e6520686f7572647461677382686465636973696f6e6461757468697369676e6174757265584092f119701f8eded9abe2c18bf62f2a9b1c8a4983997ec0af4c21f592e3d6df3def594aaa979be0c5cdaaeb53c79c899aeef2f4a79e916b582f572bcba79c4601",
        id: "1866e725093c0dd494a91d990f519591f70c372abe1556720f856f5e143b8735",
        signature: "92f119701f8eded9abe2c18bf62f2a9b1c8a4983997ec0af4c21f592e3d6df3def594aaa979be0c5cdaaeb53c79c899aeef2f4a79e916b582f572bcba79c4601",
    },
    Vector {
        kind: BodyKind::Artifact,
        encoding: "a267636f6e74656e74a66870726f746f636f6c016667726f756e645820676767676767676767676767676767676767676767676767676767676767676766617574686f725820bc7cbcb5636375fa1d82434d466724d92377f53b980695dd49d26d0ce12205a567706172656e74738158201866e725093c0dd494a91d990f519591f70c372abe1556720f856f5e143b87356c74696d657374616d705f6d731b000001a17720ca0164626f6479a1686172746966616374a3646e616d6567617574682e72736a6d656469615f747970656b746578742f782d7275737467636f6e74656e74582670756220636f6e737420544f4b454e5f54544c5f534543533a20753634203d20333630303b0a697369676e6174757265584048f76db1346e5140bbcea6f25ea5d916b491c4f298a60bf2001715c62fe05833314d7b0e885786fa97f8eea405752256fae442abc3776ab56e845a9701c59f02",
        id: "edfb478710f8256442fc5eca5360bf97c25855c6851e03befea162ebeea3bc85",
        signature: "48f76db1346e5140bbcea6f25ea5d916b491c4f298a60bf2001715c62fe05833314d7b0e885786fa97f8eea405752256fae442abc3776ab56e845a9701c59f02",
    },
    Vector {
        kind: BodyKind::Relation,
        encoding: "a267636f6e74656e74a66870726f746f636f6c016667726f756e645820676767676767676767676767676767676767676767676767676767676767676766617574686f725820bc7cbcb5636375fa1d82434d466724d92377f53b980695dd49d26d0ce12205a567706172656e74738258201866e725093c0dd494a91d990f519591f70c372abe1556720f856f5e143b87355820edfb478710f8256442fc5eca5360bf97c25855c6851e03befea162ebeea3bc856c74696d657374616d705f6d731b000001a17720ca0264626f6479a16872656c6174696f6ea46466726f6d5820edfb478710f8256442fc5eca5360bf97c25855c6851e03befea162ebeea3bc85646b696e646a696d706c656d656e747362746f58201866e725093c0dd494a91d990f519591f70c372abe1556720f856f5e143b8735646e6f74656374746c697369676e61747572655840a4bdbe901bec1abf496a871229635966d164b99426dcf0754aed0ab53957042130f132f3f590e7216bb2bd89062065a0149a70b7c64dd0fcdcee90e2f901e703",
        id: "24d6b1b6ce01934d9aa66e20e89f2f39082a9f3029f9b5e33e6b3e299b30f949",
        signature: "a4bdbe901bec1abf496a871229635966d164b99426dcf0754aed0ab53957042130f132f3f590e7216bb2bd89062065a0149a70b7c64dd0fcdcee90e2f901e703",
    },
    Vector {
        kind: BodyKind::Evidence,
        encoding: "a267636f6e74656e74a66870726f746f636f6c016667726f756e645820676767676767676767676767676767676767676767676767676767676767676766617574686f725820705fbac01f5519899f437bc42e40255ae9ab54bff00de3433af7d687d9e71ad567706172656e747382582024d6b1b6ce01934d9aa66e20e89f2f39082a9f3029f9b5e33e6b3e299b30f9495820edfb478710f8256442fc5eca5360bf97c25855c6851e03befea162ebeea3bc856c74696d657374616d705f6d731b000001a17720ca0364626f6479a16865766964656e6365a4667461726765745820edfb478710f8256442fc5eca5360bf97c25855c6851e03befea162ebeea3bc8567766572646963746470617373666d6574686f6474636172676f2074657374204020303132336162636764657461696c7369313220706173736564697369676e61747572655840a29a66344e58d15435bd0f100ddd95df78635be16c45de7098f04a0e74c8d14a577d786145fd45b99242d7fd2de8f9fc7f14c485bef1ec01f530176ccfaa8303",
        id: "c9993a0f5e1ee4b0a301f5f760064ad938ac8e57e292fab2c2f9d4e1b4a58ae1",
        signature: "a29a66344e58d15435bd0f100ddd95df78635be16c45de7098f04a0e74c8d14a577d786145fd45b99242d7fd2de8f9fc7f14c485bef1ec01f530176ccfaa8303",
    },
    Vector {
        kind: BodyKind::Retract,
        encoding: "a267636f6e74656e74a66870726f746f636f6c016667726f756e645820676767676767676767676767676767676767676767676767676767676767676766617574686f725820bc7cbcb5636375fa1d82434d466724d92377f53b980695dd49d26d0ce12205a567706172656e74738258201866e725093c0dd494a91d990f519591f70c372abe1556720f856f5e143b87355820c9993a0f5e1ee4b0a301f5f760064ad938ac8e57e292fab2c2f9d4e1b4a58ae16c74696d657374616d705f6d731b000001a17720ca0464626f6479a16772657472616374a26674617267657458201866e725093c0dd494a91d990f519591f70c372abe1556720f856f5e143b873566726561736f6e69746f6f2073686f7274697369676e61747572655840e2bbc9caa9a60ec597d25f67fbe109aed2d4472230d27faf40469ff8575554a03f2b94db2acbca96aecb1de828c881c6b8a3110b7a0184c73818aaa2c0259901",
        id: "7005c0f4a6f99d28e9c45253099ec341531972469444e88be7aade52e32825ba",
        signature: "e2bbc9caa9a60ec597d25f67fbe109aed2d4472230d27faf40469ff8575554a03f2b94db2acbca96aecb1de828c881c6b8a3110b7a0184c73818aaa2c0259901",
    },
];

fn vector_contributions() -> Vec<Contribution> {
    let (alice, bob) = (identity(ALICE), identity(BOB));
    let t = 1_793_000_000_000;
    let c = Contribution::create(GROUND, &alice, [], t, claim("tokens expire after one hour", &["decision", "auth"]));
    let artifact = Body::Artifact {
        name: "auth.rs".into(),
        media_type: "text/x-rust".into(),
        content: b"pub const TOKEN_TTL_SECS: u64 = 3600;\n".to_vec(),
    };
    let a = Contribution::create(GROUND, &alice, [c.id()], t + 1, artifact);
    let r =
        Contribution::create(GROUND, &alice, [a.id()], t + 2, relate(a.id(), RelationKind::Implements, c.id(), "ttl"));
    let e = Contribution::create(
        GROUND,
        &bob,
        [r.id()],
        t + 3,
        evidence(a.id(), Verdict::Pass, "cargo test @ 0123abc", "12 passed"),
    );
    let x = Contribution::create(
        GROUND,
        &alice,
        [e.id()],
        t + 4,
        Body::Retract { target: c.id(), reason: "too short".into() },
    );
    vec![c, a, r, e, x]
}

#[test]
fn golden_vectors_encode_to_pinned_bytes() {
    let built = vector_contributions();
    assert_eq!(built.len(), VECTORS.len());
    for (c, v) in built.iter().zip(&VECTORS) {
        assert_eq!(c.body().kind(), v.kind);
        assert_eq!(hex::encode(c.encode()), v.encoding, "{} encoding changed", v.kind);
        assert_eq!(c.id().to_hex(), v.id, "{} id changed", v.kind);
        assert_eq!(c.signature().to_hex(), v.signature, "{} signature changed", v.kind);
    }
}

#[test]
fn golden_vectors_decode_and_verify() {
    let mut graph = Graph::new(GROUND);
    for v in &VECTORS {
        let bytes = hex::decode(v.encoding).unwrap();
        let c = Contribution::decode(&bytes).unwrap_or_else(|e| panic!("{} no longer decodes: {e}", v.kind));
        assert_eq!(c.body().kind(), v.kind);
        assert_eq!(c.id().to_hex(), v.id, "{} decodes to a different id", v.kind);
        assert_eq!(c.signature().to_hex(), v.signature);
        assert!(c.signature_is_valid(), "{} signature no longer verifies", v.kind);
        graph.insert(c).unwrap_or_else(|e| panic!("{} is no longer valid: {e}", v.kind));
    }
}

// ---------------------------------------------------------------------------
// Negative vectors: what a replica must reject. Each one fails if a rejection
// rule is loosened.
// ---------------------------------------------------------------------------

/// The pinned claim vector with one hex substring replaced. Panics unless
/// `from` occurs exactly once, so an edit can't silently miss.
fn edited_claim(from: &str, to: &str) -> Vec<u8> {
    let encoding = VECTORS[0].encoding;
    assert_eq!(encoding.matches(from).count(), 1, "{from} must occur once in the claim vector");
    hex::decode(encoding.replace(from, to)).unwrap()
}

/// Encodes `content` with `signature` as a contribution on the wire, without
/// going through `Contribution::create`, so a test can sign whatever it likes.
fn wire(content: &contribution::ContributionContent, signature: Signature) -> Vec<u8> {
    #[derive(serde::Serialize)]
    struct Wire<'a> {
        content: &'a contribution::ContributionContent,
        signature: Signature,
    }
    let mut bytes = Vec::new();
    ciborium::into_writer(&Wire { content, signature }, &mut bytes).unwrap();
    bytes
}

#[test]
fn non_canonical_encodings_are_rejected() {
    const PROTOCOL_1: &str = "6870726f746f636f6c01"; // "protocol": 1
    let ground_entry = format!("6667726f756e645820{}", "67".repeat(32)); // "ground": bytes(32)
    let mut trailing = hex::decode(VECTORS[0].encoding).unwrap();
    trailing.push(0x00);
    let mut indefinite = hex::decode(VECTORS[0].encoding).unwrap();
    assert_eq!(indefinite[0], 0xa2, "the claim vector starts with a 2-entry map");
    indefinite[0] = 0xbf; // indefinite-length map...
    indefinite.push(0xff); // ...and its break byte

    let cases = [
        ("non-shortest integer", edited_claim(PROTOCOL_1, "6870726f746f636f6c1801")),
        (
            "map keys out of order",
            edited_claim(&format!("{PROTOCOL_1}{ground_entry}"), &format!("{ground_entry}{PROTOCOL_1}")),
        ),
        ("indefinite-length map", indefinite),
        ("trailing bytes", trailing),
    ];
    for (what, bytes) in cases {
        let result = Contribution::decode(&bytes);
        assert!(matches!(result, Err(contribution::DecodeError::NotCanonical)), "{what}: got {result:?}");
    }
}

#[test]
fn other_protocol_versions_and_grounds_are_rejected() {
    let alice = identity(ALICE);
    let graph = Graph::new(GROUND);

    let mut content = Contribution::create(GROUND, &alice, [], 1, claim("from the future", &[])).content().clone();
    content.protocol = 2;
    let future = Contribution::decode(&wire(&content, alice.sign(&content.id()))).unwrap();
    assert!(future.signature_is_valid(), "validly signed, so only the version check can reject it");
    assert_eq!(graph.check(&future), Err(RuleViolation::UnsupportedProtocol(2)));

    let elsewhere = Contribution::create(GroundId([0x68; 32]), &alice, [], 1, claim("elsewhere", &[]));
    assert!(matches!(graph.check(&elsewhere), Err(RuleViolation::WrongGround { .. })));
}

#[test]
fn signatures_are_verified_strictly() {
    // The Ed25519 identity point as public key, with R = identity and s = 0,
    // satisfies the cofactorless equation [s]B = R + [k]A for every message.
    // Plain RFC 8032 verification accepts it; strict verification rejects the
    // small-order key.
    let mut identity_point = [0u8; 32];
    identity_point[0] = 1;
    let mut signature = [0u8; 64];
    signature[0] = 1;

    let mut content =
        Contribution::create(GROUND, &identity(ALICE), [], 1, claim("anyone wrote this", &[])).content().clone();
    content.author = AgentId(identity_point);
    let forged = Contribution::decode(&wire(&content, Signature(signature))).unwrap();
    assert!(!forged.signature_is_valid());
    assert_eq!(Graph::new(GROUND).check(&forged), Err(RuleViolation::BadSignature));
}

#[test]
fn the_size_limit_is_exactly_16_mib() {
    const LIMIT: usize = 16 * 1024 * 1024;
    let alice = identity(ALICE);
    let artifact = |len: usize| Body::Artifact { name: "big".into(), media_type: "x".into(), content: vec![0; len] };
    let sized = |len: usize| Contribution::create(GROUND, &alice, [], 1, artifact(len));

    // Content lengths from 64 KiB up to 4 GiB share one header size, so the
    // overhead measured at 1 MiB holds at the limit.
    let overhead = sized(1 << 20).encode().len() - (1 << 20);
    let at_limit = sized(LIMIT - overhead);
    assert_eq!(at_limit.encode().len(), LIMIT);
    assert_eq!(Graph::new(GROUND).check(&at_limit), Ok(()));
    assert!(Contribution::decode(&at_limit.encode()).is_ok());

    let over = sized(LIMIT - overhead + 1);
    assert_eq!(Graph::new(GROUND).check(&over), Err(RuleViolation::TooLarge(LIMIT + 1)));
    let result = Contribution::decode(&over.encode());
    assert!(matches!(result, Err(contribution::DecodeError::TooLarge(n)) if n == LIMIT + 1), "got {result:?}");
}

#[test]
fn parents_must_be_strictly_ascending_and_list_every_reference() {
    let alice = identity(ALICE);
    let mut graph = Graph::new(GROUND);
    let mut ids = Vec::new();
    for (t, statement) in [(1, "a"), (2, "b")] {
        let c = Contribution::create(GROUND, &alice, [], t, claim(statement, &[]));
        ids.push(c.id());
        graph.insert(c).unwrap();
    }
    ids.sort();
    let (a, b) = (ids[0], ids[1]); // a < b

    // Re-signs a copy of `base` with other parents, as a forger could.
    let with_parents = |base: &Contribution, parents: Vec<ContributionId>| {
        let mut content = base.content().clone();
        content.parents = parents;
        Contribution::decode(&wire(&content, alice.sign(&content.id()))).unwrap()
    };
    let child = Contribution::create(GROUND, &alice, [a, b], 3, claim("child", &[]));
    assert_eq!(graph.check(&child), Ok(()));
    for parents in [vec![b, a], vec![a, a, b]] {
        assert_eq!(graph.check(&with_parents(&child, parents)), Err(RuleViolation::ParentsNotCanonical));
    }

    let about_a = Contribution::create(GROUND, &alice, [b], 3, evidence(a, Verdict::Pass, "review", ""));
    assert_eq!(graph.check(&about_a), Ok(()));
    assert_eq!(graph.check(&with_parents(&about_a, vec![b])), Err(RuleViolation::UnlistedReference(a)));
}

// ---------------------------------------------------------------------------
// Bundle vector: the transport format between replicas.
// ---------------------------------------------------------------------------

/// `"MRBUNDL1"`, the Ground id, then each contribution after its u32
/// big-endian length: here the claim and artifact vectors.
fn pinned_bundle() -> String {
    let (claim, artifact) = (VECTORS[0].encoding, VECTORS[1].encoding);
    format!("4d5242554e444c31{}0000011a{claim}00000157{artifact}", "67".repeat(32))
}

#[test]
fn bundles_encode_to_pinned_bytes() {
    let mut bytes = Vec::new();
    let n = bundle::write(&mut bytes, GROUND, vector_contributions().into_iter().take(2)).unwrap();
    assert_eq!(n, 2);
    assert_eq!(hex::encode(&bytes), pinned_bundle());

    let (ground, read) = bundle::read(hex::decode(pinned_bundle()).unwrap().as_slice()).unwrap();
    assert_eq!(ground, GROUND);
    let ids: Vec<String> = read.iter().map(|c| c.id().to_hex()).collect();
    assert_eq!(ids, [VECTORS[0].id, VECTORS[1].id]);
}

// ---------------------------------------------------------------------------
// Fixture Ground: a replica on disk, as version 0.1.0 wrote it.
// ---------------------------------------------------------------------------

/// Pinned ids of the fixture's subjects and their statuses under the default
/// policy: every status, and the precedence between them.
const FIXTURE_STATUSES: [(&str, Status); 11] = [
    // "tokens expire after one hour"
    ("f844bc81e7bc741a5b952acd65cb0346c6c57324468fad777381b0b8121470c8", Status::Superseded),
    // auth.rs
    ("a7a43fb2aba5b6eb830790052802f708780d08dbd02f268496ad2993fc76104f", Status::Accepted),
    // "tokens expire after two hours"
    ("76178bfa7147234c4892fed630ae51dafc19b3a879f5a92f2e0a8852391168c3", Status::Accepted),
    // "cache sessions in memory": a counted pass and a counted fail
    ("6c51c956c53b61728e823de4dcddd7017aef5e1f38659b0f78f103f94565ded8", Status::Rejected),
    // "drop sessions entirely"
    ("6924e584cd3aa6a11702bf51be4404af6ac93ab76196b9adcb65b6d9ed74af5d", Status::Retracted),
    // "keep sessions in redis"
    ("f34348d4d6c2c099688dd8b79194860dbfd44b5cdaca73e2c6659a1499b353ec", Status::Pending),
    // "rotate signing keys daily": retracted, with a counted fail
    ("98355b3ed012fa04b1559f12fe161d6a992ecf1c42babcc5d5ace0e4971f4fe2", Status::Retracted),
    // Appended for the supersession rule (decision 4c55e65bc290).
    // "tokens expire after 90 minutes": bob's, claims to supersede alice's
    // "two hours", which stays accepted above because that doesn't count
    ("f732a8371497252331f513156a478bc66aa87891d5f48ca25826c6fabcf7b6eb", Status::Accepted),
    // "log failed logins": alice accepted carol's replacement, which counts
    ("4eedeb02e6ef7a23c18352a3a70a22ac98dbe57e62887eb078b6a3bc9827901e", Status::Superseded),
    // "log failed and successful logins"
    ("c76ceda569128634a7967495f46756587d02f5d592ac5c18952e070483bb5f2e", Status::Accepted),
    // "keep sessions in postgres": bob's, claims to supersede carol's "redis",
    // whose contradiction therefore stays open below
    ("660d33f71236a66f412490ef71202e299d19afd0faecba64c8c19e503146715c", Status::Pending),
];

// The fixture's contribution of each body kind, relation kind and verdict, so
// every wire name is pinned by bytes on disk. The matches have no wildcard:
// adding a variant fails to compile until the fixture covers it.

fn fixture_body(kind: BodyKind) -> &'static str {
    match kind {
        BodyKind::Claim => "f844bc81e7bc741a5b952acd65cb0346c6c57324468fad777381b0b8121470c8",
        BodyKind::Artifact => "a7a43fb2aba5b6eb830790052802f708780d08dbd02f268496ad2993fc76104f",
        BodyKind::Relation => "e628ba51cba47bed175d752d922751649a9cfb651312f26a14ca516c79a5f443",
        BodyKind::Evidence => "becb406c3f2ca5ec53aa8980062a363d301d644c1abfaff5020fd94e475fa7ee",
        BodyKind::Retract => "d7f4c3ac87730f5c5eb6c8dc040f4825134328fd47361bd38da342b41ab57108",
    }
}

fn fixture_relation(kind: RelationKind) -> &'static str {
    match kind {
        RelationKind::Supports => "4db47ffb86e0fc48c535fd9939b2ab9c73e90992b1a95d148abadf240e4eb40e",
        RelationKind::Contradicts => "808d58df5d4f5570a40eece2e4c9c5f14ca550643ae508527d2ff410bd00ba6b",
        RelationKind::DependsOn => "6172802e2bc6d3930339e877c47ef8bc89efebc327fa8a42d7d0f89d2c1bfe85",
        RelationKind::Refines => "51672e5f18a0ac576460395991392b0dce47c002049c916d25080b037d1f4d44",
        RelationKind::Supersedes => "222245bf60eb1e6375b992b7a7cb5df7990be4182570f02da538bbc93b4b5c79",
        RelationKind::Implements => "e628ba51cba47bed175d752d922751649a9cfb651312f26a14ca516c79a5f443",
        RelationKind::Tests => "e37f81ed02ed8188b061058a223c74807b86953db7852509c9c06a636e9a816f",
    }
}

fn fixture_evidence(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Pass => "becb406c3f2ca5ec53aa8980062a363d301d644c1abfaff5020fd94e475fa7ee",
        Verdict::Fail => "15b433921525a23fc72d03b75e948c93952ab5f7a6e8ea975546016b0094ac48",
    }
}

#[test]
fn fixture_covers_every_kind() {
    let store = Store::open(&fixture_dir()).expect("the v1 fixture Ground opens");
    let get = |id: &str| store.graph().get(&id.parse().unwrap()).expect("pinned id is in the fixture").body().clone();
    for kind in BodyKind::ALL {
        assert_eq!(get(fixture_body(*kind)).kind(), *kind);
    }
    for kind in RelationKind::ALL {
        assert!(matches!(get(fixture_relation(*kind)), Body::Relation { kind: k, .. } if k == *kind), "{kind}");
    }
    for verdict in Verdict::ALL {
        assert!(matches!(get(fixture_evidence(*verdict)), Body::Evidence { verdict: v, .. } if v == *verdict));
    }
}

/// Every contribution id in the fixture, sorted.
const FIXTURE_IDS: [&str; 33] = [
    "15b433921525a23fc72d03b75e948c93952ab5f7a6e8ea975546016b0094ac48",
    "178367f61edbdd30afc59a23f772edf3180876289327a18467640f8802fed1c6",
    "222245bf60eb1e6375b992b7a7cb5df7990be4182570f02da538bbc93b4b5c79",
    "34838f28015d42849a3ea06f0a5bb0381bf4920a75f3ffdf71620d22b1fd38a3",
    "4db47ffb86e0fc48c535fd9939b2ab9c73e90992b1a95d148abadf240e4eb40e",
    "4eedeb02e6ef7a23c18352a3a70a22ac98dbe57e62887eb078b6a3bc9827901e",
    "51672e5f18a0ac576460395991392b0dce47c002049c916d25080b037d1f4d44",
    "6172802e2bc6d3930339e877c47ef8bc89efebc327fa8a42d7d0f89d2c1bfe85",
    "660d33f71236a66f412490ef71202e299d19afd0faecba64c8c19e503146715c",
    "6924e584cd3aa6a11702bf51be4404af6ac93ab76196b9adcb65b6d9ed74af5d",
    "6c51c956c53b61728e823de4dcddd7017aef5e1f38659b0f78f103f94565ded8",
    "76178bfa7147234c4892fed630ae51dafc19b3a879f5a92f2e0a8852391168c3",
    "7639fc3e700c95c879504ce3667dfb72f2a2e7d99b743f68b2228dd22c05f048",
    "7d94bd0888a748f8f4fd82afe64616a716c1fca03e8aff821f1127184986239c",
    "808d58df5d4f5570a40eece2e4c9c5f14ca550643ae508527d2ff410bd00ba6b",
    "964254b1499d51a3b3f09d2c7bb7b6d12456f394495837b8444737faebbbebba",
    "98355b3ed012fa04b1559f12fe161d6a992ecf1c42babcc5d5ace0e4971f4fe2",
    "a7a43fb2aba5b6eb830790052802f708780d08dbd02f268496ad2993fc76104f",
    "b94165ce6e141d541e770dbfab33de769ffca2cca43170e98e01ae38c9dc6314",
    "bac6e7a4abcb3cde1b9bc6b89ebe5ce8e35c03d4def68db239464ccce1afd2b1",
    "be7454450a144d7991ed5d4b1edf738e106cb0caeb6eb4eddbd681e5f6781452",
    "becb406c3f2ca5ec53aa8980062a363d301d644c1abfaff5020fd94e475fa7ee",
    "c2c0a2141cf90d3142a79d77f7ea606c7b9c443702488ae39c94740e2449aaf9",
    "c76ceda569128634a7967495f46756587d02f5d592ac5c18952e070483bb5f2e",
    "c8e94d5a68b5cadb611588c1064ef1198375e02d63de193e985c442cf8fecfb8",
    "d7f4c3ac87730f5c5eb6c8dc040f4825134328fd47361bd38da342b41ab57108",
    "e37f81ed02ed8188b061058a223c74807b86953db7852509c9c06a636e9a816f",
    "e628ba51cba47bed175d752d922751649a9cfb651312f26a14ca516c79a5f443",
    "f2cf2005456cf8de1a13a4dfdeab069ad762f15b44943bcbff585aedca796114",
    "f34348d4d6c2c099688dd8b79194860dbfd44b5cdaca73e2c6659a1499b353ec",
    "f732a8371497252331f513156a478bc66aa87891d5f48ca25826c6fabcf7b6eb",
    "f844bc81e7bc741a5b952acd65cb0346c6c57324468fad777381b0b8121470c8",
    "f9a3bb0a9f2ef6f3bc0579c40756381be95f5657e6816d17f6b2c5aa3d0fc305",
];

#[test]
fn fixture_ground_opens_and_verifies() {
    let dir = fixture_dir();
    let store = Store::open(&dir).expect("the v1 fixture Ground opens");
    assert_eq!(store.info().ground, GROUND);
    assert_eq!(store.info().name, "ground-v1");

    let graph = store.graph();
    let mut ids: Vec<String> = graph.iter().map(|c| c.id().to_hex()).collect();
    ids.sort();
    assert_eq!(ids, FIXTURE_IDS);
    for c in graph.iter() {
        assert!(c.signature_is_valid(), "{} signature no longer verifies", c.id());
        let hex = c.id().to_hex();
        let bytes = std::fs::read(dir.join("objects").join(&hex[..2]).join(&hex[2..])).unwrap();
        assert_eq!(c.encode(), bytes, "{} re-encodes differently", c.id());
    }

    let policy = Policy::default();
    for (id, status) in FIXTURE_STATUSES {
        let id: ContributionId = id.parse().unwrap();
        assert_eq!(policy.assess(graph, &id).map(|a| a.status), Some(status), "status of {id}");
    }
    assert_eq!(open_conflicts(graph).len(), 1);
    assert!(matches!(open_conflicts(graph)[0], Conflict::Contradiction { .. }));
    // Only the target's author can supersede it, with a replacement anyone wrote.
    let redis: ContributionId = "f34348d4d6c2c099688dd8b79194860dbfd44b5cdaca73e2c6659a1499b353ec".parse().unwrap();
    let failed: ContributionId = "4eedeb02e6ef7a23c18352a3a70a22ac98dbe57e62887eb078b6a3bc9827901e".parse().unwrap();
    let all: ContributionId = "c76ceda569128634a7967495f46756587d02f5d592ac5c18952e070483bb5f2e".parse().unwrap();
    assert!(open_supersessors(graph, &redis).is_empty());
    assert_eq!(open_supersessors(graph, &failed), vec![all]);

    let agents: Vec<(String, String)> =
        store.agents().unwrap().into_iter().map(|e| (e.name, e.agent.to_hex())).collect();
    let expected: Vec<(String, String)> =
        [ALICE, BOB, CAROL].iter().map(|(name, _, key)| (name.to_string(), key.to_string())).collect();
    assert_eq!(agents, expected);
}

#[test]
fn fixture_holds_no_private_keys() {
    let dir = fixture_dir();
    assert!(!dir.join("identities").exists());

    // The test seeds and the 64-byte keypairs derived from them, raw and in
    // hex, plus the field name of an identity file.
    let mut forbidden: Vec<Vec<u8>> = vec![b"secret_key".to_vec()];
    for (_, seed, public_key) in [ALICE, BOB, CAROL] {
        let keypair = [seed.as_slice(), &hex::decode(public_key).unwrap()].concat();
        for secret in [seed.to_vec(), keypair] {
            forbidden.push(hex::encode(&secret).into_bytes());
            forbidden.push(hex::encode_upper(&secret).into_bytes());
            forbidden.push(secret);
        }
    }

    let mut stack = vec![dir];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let bytes = std::fs::read(&path).unwrap();
            for secret in &forbidden {
                assert!(!bytes.windows(secret.len()).any(|w| w == secret), "{} holds a key", path.display());
            }
        }
    }
}

/// Rebuilds the fixture. Rebuilding it is a protocol change: see "Protocol
/// guard" in `docs/protocol.md`. Refuses to overwrite an existing fixture.
///
/// ```text
/// rm -r crates/manyroot/tests/fixtures/ground-v1
/// cargo test -p manyroot --test protocol_guard -- --ignored write_ground_v1_fixture
/// ```
#[test]
#[ignore = "rewrites the checked-in fixture; run only with the owner's go"]
fn write_ground_v1_fixture() {
    let dir = fixture_dir();
    let mut store = Store::create(&dir, GroundInfo { ground: GROUND, name: "ground-v1".into() }).unwrap();
    std::fs::remove_dir(store.identities_dir()).expect("a new Ground's identities/ is empty");
    let (alice, bob, carol) = (identity(ALICE), identity(BOB), identity(CAROL));

    std::fs::create_dir_all(store.agents_dir()).unwrap();
    for who in [&alice, &bob, &carol] {
        let entry = AgentEntry { name: who.name().into(), agent: who.agent_id() };
        let json = serde_json::to_string_pretty(&entry).unwrap() + "\n";
        std::fs::write(store.agents_dir().join(format!("{}.json", who.name())), json).unwrap();
    }

    let mut t = 1_793_000_000_000;
    let mut add = |store: &mut Store, who: &Identity, body: Body| {
        t += 1000;
        let heads: Vec<ContributionId> = store.graph().heads().collect();
        let c = Contribution::create(GROUND, who, heads, t, body);
        let id = c.id();
        assert!(store.add(c).unwrap());
        id
    };

    let one_hour = add(&mut store, &alice, claim("tokens expire after one hour", &["decision"]));
    let artifact = Body::Artifact {
        name: "auth.rs".into(),
        media_type: "text/x-rust".into(),
        content: b"pub const TOKEN_TTL_SECS: u64 = 3600;\n".to_vec(),
    };
    let code = add(&mut store, &alice, artifact);
    add(&mut store, &alice, relate(code, RelationKind::Implements, one_hour, ""));
    add(&mut store, &bob, evidence(one_hour, Verdict::Pass, "review", "agreed"));
    add(&mut store, &bob, evidence(code, Verdict::Pass, "cargo test @ 0123abc", "12 passed"));
    let two_hours = add(&mut store, &alice, claim("tokens expire after two hours", &["decision"]));
    add(&mut store, &alice, relate(two_hours, RelationKind::Supersedes, one_hour, "users asked for longer sessions"));
    add(&mut store, &carol, evidence(two_hours, Verdict::Pass, "review", ""));
    let cache = add(&mut store, &alice, claim("cache sessions in memory", &[]));
    add(&mut store, &bob, evidence(cache, Verdict::Fail, "load test", "evicted under load"));
    let drop = add(&mut store, &bob, claim("drop sessions entirely", &[]));
    add(&mut store, &bob, Body::Retract { target: drop, reason: "breaks logins".into() });
    let redis = add(&mut store, &carol, claim("keep sessions in redis", &["note"]));
    add(&mut store, &carol, relate(redis, RelationKind::Contradicts, cache, "one store only"));
    add(&mut store, &carol, relate(redis, RelationKind::DependsOn, two_hours, ""));
    add(&mut store, &carol, evidence(cache, Verdict::Pass, "review", "fast enough"));
    add(&mut store, &bob, relate(code, RelationKind::Supports, two_hours, "the constant is easy to change"));
    add(&mut store, &alice, relate(two_hours, RelationKind::Refines, one_hour, ""));
    let test = Body::Artifact {
        name: "auth_test.rs".into(),
        media_type: "text/x-rust".into(),
        content: b"#[test]\nfn ttl() { assert_eq!(TOKEN_TTL_SECS, 3600); }\n".to_vec(),
    };
    let test = add(&mut store, &bob, test);
    add(&mut store, &bob, relate(test, RelationKind::Tests, code, ""));
    let rotate = add(&mut store, &carol, claim("rotate signing keys daily", &[]));
    add(&mut store, &bob, evidence(rotate, Verdict::Fail, "review", "too often"));
    add(&mut store, &carol, Body::Retract { target: rotate, reason: "premature".into() });

    // Appended for the supersession rule (decision 4c55e65bc290): a supersedes
    // relation counts only when its author wrote the contribution it replaces.
    // Everything above is unchanged, so its ids are too.

    // Another author's supersession doesn't count: "two hours" stays accepted.
    let ninety = add(&mut store, &bob, claim("tokens expire after 90 minutes", &["decision"]));
    add(&mut store, &bob, relate(ninety, RelationKind::Supersedes, two_hours, "a compromise"));
    add(&mut store, &carol, evidence(ninety, Verdict::Pass, "review", ""));
    // The replaced claim's author accepting someone else's replacement does count.
    let failed = add(&mut store, &alice, claim("log failed logins", &[]));
    add(&mut store, &bob, evidence(failed, Verdict::Pass, "review", ""));
    let all = add(&mut store, &carol, claim("log failed and successful logins", &[]));
    add(&mut store, &bob, evidence(all, Verdict::Pass, "review", ""));
    add(&mut store, &alice, relate(all, RelationKind::Supersedes, failed, "I accept yours"));
    // Another author's supersession doesn't silence a contradiction (task
    // 780f658d4b84): "keep sessions in redis" stays current.
    let postgres = add(&mut store, &bob, claim("keep sessions in postgres", &[]));
    add(&mut store, &bob, relate(postgres, RelationKind::Supersedes, redis, ""));
}

/// One sync message of every type, as pinned bytes (docs/protocol.md, "Sync"). Append new
/// vectors; never edit these.
const SYNC_VECTORS: [(&str, &str); 6] = [
    (
        "hello",
        "a564747970656568656c6c6f6870726f746f636f6c016667726f756e6458206767676767676767676767676767676767676767676767676767676767676767666c696d697473a16b62617463685f62797465731a0400000068666561747572657380",
    ),
    (
        "heads",
        "a26474797065656865616473656865616473825820111111111111111111111111111111111111111111111111111111111111111158202222222222222222222222222222222222222222222222222222222222222222",
    ),
    ("offer", "a26474797065656f66666572636964738158203333333333333333333333333333333333333333333333333333333333333333"),
    ("need", "a26474797065646e656564636964738158203333333333333333333333333333333333333333333333333333333333333333"),
    (
        "batch",
        "a364747970656562617463686662756e646c6558284d5242554e444c316767676767676767676767676767676767676767676767676767676767676767646c617374f5",
    ),
    (
        "result",
        "a5647479706566726573756c74656164646564026f616c72656164795f70726573656e74006872656a656374656481a26269645820444444444444444444444444444444444444444444444444444444444444444466726561736f6e6d626164207369676e61747572656a756e7265736f6c76656401",
    ),
];

fn sync_messages() -> [sync::Message; 6] {
    use sync::{Limits, Message, Rejected};
    let id = |b: u8| ContributionId([b; 32]);
    let mut empty_bundle = Vec::new();
    bundle::write(&mut empty_bundle, GROUND, Vec::<Contribution>::new()).unwrap();
    [
        Message::Hello {
            protocol: 1,
            ground: GROUND,
            limits: Limits { batch_bytes: 64 * 1024 * 1024 },
            features: vec![],
        },
        Message::Heads { heads: vec![id(0x11), id(0x22)] },
        Message::Offer { ids: vec![id(0x33)] },
        Message::Need { ids: vec![id(0x33)] },
        Message::Batch { bundle: empty_bundle, last: true },
        Message::Result {
            added: 2,
            already_present: 0,
            rejected: vec![Rejected { id: id(0x44), reason: "bad signature".into() }],
            unresolved: 1,
        },
    ]
}

#[test]
fn sync_messages_encode_to_pinned_bytes() {
    for ((name, pinned), message) in SYNC_VECTORS.iter().zip(sync_messages()) {
        assert_eq!(hex::encode(message.encode()), *pinned, "{name} encoding changed");
        let bytes = hex::decode(pinned).unwrap();
        assert_eq!(sync::Message::decode(&bytes, bytes.len()).unwrap(), message, "{name} no longer decodes");
    }
    let sync::Message::Batch { bundle: pinned_batch, .. } = &sync_messages()[4] else { unreachable!() };
    assert_eq!(bundle::read(pinned_batch.as_slice()).unwrap().0, GROUND, "a batch carries a bundle");
}

/// Id lists are split across messages of at most this many ids; a short message ends the list.
const SYNC_MAX_IDS: usize = 65_536;
/// An empty offer: how a list that fills its earlier messages exactly ends.
const SYNC_EMPTY_OFFER: &str = "a26474797065656f666665726369647380";

#[test]
fn sync_id_lists_are_split_at_a_pinned_size() {
    assert_eq!(sync::MAX_IDS, SYNC_MAX_IDS, "the id-list size is part of the sync protocol");
    let empty = sync::Message::Offer { ids: vec![] };
    assert_eq!(hex::encode(empty.encode()), SYNC_EMPTY_OFFER, "an empty offer's encoding changed");
    // A full list fits the message limit of any replica, whatever its batch_bytes.
    let full = sync::Message::Need { ids: vec![ContributionId([1; 32]); SYNC_MAX_IDS] }.encode();
    let smallest = sync::Limits { batch_bytes: 44 };
    assert_eq!(
        sync::Message::decode(&full, smallest.max_message_bytes()).unwrap(),
        sync::Message::Need { ids: vec![ContributionId([1; 32]); SYNC_MAX_IDS] }
    );
}

/// A result lists at most this many rejected contributions, with reasons of at most this many bytes.
const SYNC_MAX_REJECTED: usize = 1024;
const SYNC_MAX_REASON_BYTES: usize = 256;

#[test]
fn sync_results_are_bounded_at_a_pinned_size() {
    assert_eq!(sync::MAX_REJECTED, SYNC_MAX_REJECTED, "the rejected-list bound is part of the sync protocol");
    assert_eq!(sync::MAX_REASON_BYTES, SYNC_MAX_REASON_BYTES, "the reason bound is part of the sync protocol");
    // The largest result fits the smallest replica's message limit.
    let rejected = sync::Rejected { id: ContributionId([1; 32]), reason: "r".repeat(SYNC_MAX_REASON_BYTES) };
    let largest = sync::Message::Result {
        added: u64::MAX,
        already_present: u64::MAX,
        rejected: vec![rejected; SYNC_MAX_REJECTED],
        unresolved: u64::MAX,
    };
    let bytes = largest.encode();
    assert_eq!(sync::Message::decode(&bytes, sync::Limits { batch_bytes: 44 }.max_message_bytes()).unwrap(), largest);
}

/// The smallest batch_bytes a hello may carry: an empty bundle (magic and Ground id) plus one length prefix.
const SYNC_MIN_BATCH_BYTES: u64 = 44;

#[test]
fn sync_hello_batch_bytes_has_a_pinned_minimum() {
    let graph = Graph::new(GROUND);
    let hello = |batch_bytes| sync::Message::Hello {
        protocol: 1,
        ground: GROUND,
        limits: sync::Limits { batch_bytes },
        features: vec![],
    };
    let mut session = sync::Session::new(GROUND, sync::Limits::default());
    assert_eq!(
        session.receive(&graph, hello(SYNC_MIN_BATCH_BYTES - 1)).unwrap_err(),
        sync::SyncError::LimitTooSmall(SYNC_MIN_BATCH_BYTES - 1)
    );
    let mut session = sync::Session::new(GROUND, sync::Limits::default());
    assert!(session.receive(&graph, hello(SYNC_MIN_BATCH_BYTES)).is_ok());
}
