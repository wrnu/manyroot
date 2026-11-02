//! The fixture Weave Ground (docs/design/weave.md, section 5 *Protocol changes*; build step 8).
//!
//! `tests/fixtures/weave-v1` is a Weave Ground committed as test data. It holds a record of every Weave
//! media type, through lines, decisions, proposals, a resolution, check results, reviews, a goal and a
//! policy change, written by version 0.1.0 of `bracken-tree`. Its line `main` holds rule 5's answers and
//! settling; a second line, `side`, holds rule 6's guardrails, the evidence-backed basis, `fell_back` and
//! `accepted_breakage` answers, a fork settled by a merge, and an executable file. Together with `format_guard.rs`'s golden
//! vectors it pins Weave's payload formats: this test checks that it opens, that every record still
//! decodes and re-encodes to its bytes, and that every derived status (qualification, heads, green,
//! counted results, reviews, goals, open objections) is what the scenario below says it is.
//!
//! If a test here fails, a change altered how existing Weave Grounds read. That is protocol class: an
//! independent review, the manager's go logged as `owner-review`, and an entry in docs/releases.md.
//! Don't rebuild the fixture or update a pinned value to make it pass.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use bracken_tree::check::{Exit, RecordedTail};
use bracken_tree::format::{
    Answer, Breakage, Change, Decision, Disposition, FILE_MEDIA_TYPE, GOAL_TAG, Guardrails, Line, MEDIA_TYPE_PREFIX,
    Manifest, Marker, Proposal, Record, Resolution, Source, VERSION,
};
use bracken_tree::ground::Index;
use bracken_tree::lines::{Lines, Qualified, Unqualified};
use bracken_tree::merge::Key;
use bracken_tree::results::{Checks, Green, details};
use bracken_tree::reviews::{Reviews, goals};
use bracken_tree::{ContentHash, Entry, FileValue, Mode};
use manyroot::store::{AgentEntry, GroundInfo};
use manyroot::{Body, Contribution, ContributionId, GroundId, Identity, RelationKind, Store, Verdict};

/// The fixture, checked in. Only `write_weave_v1_fixture` writes it.
fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/weave-v1")
}

const GROUND: GroundId = GroundId([0x77; 32]);

/// Fixed test keys, the same as the coordination fixture's: (name, secret seed, the public key it
/// must derive). alice is the line's author, integrator and goal setter; bob its validator, and carol's
/// too from the second policy version; carol proposes, reviews and objects.
const ALICE: (&str, [u8; 32], &str) =
    ("alice", [0xa1; 32], "bc7cbcb5636375fa1d82434d466724d92377f53b980695dd49d26d0ce12205a5");
const BOB: (&str, [u8; 32], &str) =
    ("bob", [0xb0; 32], "705fbac01f5519899f437bc42e40255ae9ab54bff00de3433af7d687d9e71ad5");
const CAROL: (&str, [u8; 32], &str) =
    ("carol", [0xc0; 32], "5b9437adeaffbe8f41b13d96ed49d2f51cd6c266cd8ecc284b0552ec4912b8dd");

/// An identity from a fixed seed, through an identity file in a scratch directory. The file is built
/// at runtime, so the source holds no key file (gotcha 5a3dda0b1f93).
fn identity((name, seed, public_key): (&str, [u8; 32], &str)) -> Identity {
    let tmp = tempfile::tempdir().expect("a scratch directory");
    let path = tmp.path().join(format!("{name}.json"));
    let field = ["secret", "key"].join("_");
    let file = serde_json::json!({ "name": name, "public_key": public_key, field: hex::encode(seed) });
    std::fs::write(&path, file.to_string()).expect("write");
    let identity = Identity::load(&path).expect("the seed derives the pinned public key");
    assert_eq!(identity.agent_id().to_hex(), public_key);
    identity
}

fn artifact<R: Record>(record: &R) -> Body {
    Body::Artifact {
        name: record.artifact_name().to_owned(),
        media_type: R::MEDIA_TYPE.to_owned(),
        content: record.encode(),
    }
}

fn file(bytes: &[u8]) -> Body {
    Body::Artifact { name: "file".into(), media_type: FILE_MEDIA_TYPE.into(), content: bytes.to_vec() }
}

fn claim(statement: &str, tags: &[&str]) -> Body {
    Body::Claim { statement: statement.into(), tags: tags.iter().map(|t| t.to_string()).collect() }
}

fn relate(from: ContributionId, kind: RelationKind, to: ContributionId, note: &str) -> Body {
    Body::Relation { from, kind, to, note: note.into() }
}

fn tree(files: &[(&str, &[u8])]) -> Manifest {
    Manifest::new(
        files
            .iter()
            .map(|(path, bytes)| Entry {
                path: (*path).into(),
                mode: Mode::File,
                hash: ContentHash::of(bytes),
                size: bytes.len() as u64,
            })
            .collect(),
    )
}

fn value(bytes: &[u8]) -> Option<FileValue> {
    Some(FileValue { hash: ContentHash::of(bytes), mode: Mode::File })
}

/// The check every policy version names: `test`, run as `sh test.sh`.
fn checks() -> BTreeMap<String, Vec<String>> {
    BTreeMap::from([("test".to_owned(), vec!["sh".to_owned(), "test.sh".to_owned()])])
}

fn policy(validators: &[&Identity], max_known: u64) -> Line {
    let alice = identity(ALICE).agent_id();
    Line {
        v: VERSION,
        name: "main".into(),
        integrators: vec![alice],
        validators: validators.iter().map(|v| v.agent_id()).collect(),
        goal_setters: vec![alice],
        checks: checks(),
        executors: BTreeMap::new(),
        guardrails: Guardrails { max_known, max_age: 3, protected: vec!["docs".into()] },
        owner_decision: Some("0123456789abcdef".into()),
    }
}

fn method() -> String {
    let policy = policy(&[], 1);
    policy.check_method("test").expect("the policy's check")
}

/// A check result as `weave validate` records it.
fn result(target: ContributionId, manifest: &ContentHash, attempt: u64, passed: bool) -> Body {
    let argv = checks()["test"].clone();
    let (exit, tail) = if passed { (Exit::Code(0), "ok\n") } else { (Exit::Code(1), "a.txt: expected two\n") };
    Body::Evidence {
        target,
        verdict: if passed { Verdict::Pass } else { Verdict::Fail },
        method: method(),
        details: details(manifest, attempt, &argv, &exit, &RecordedTail::Text(tail.into())),
    }
}

/// Every contribution id in the fixture, sorted.
const FIXTURE_IDS: [&str; 80] = [
    "02ed4a94d1d2569473e30be1f270dc90752bd878902620efea4b06651fdc77de",
    "0525c67be79c22b6a859118457df7da8d63b0177083243e67619079e9f8a0db5",
    "0d322d12d64bc174d2033b111f0bfdd8fe70fdbb52eb09a2c3e57c13499b9b3b",
    "0d821746c136e057bbb45f17c94866daa31e5aa2df3062ae6593b6652cceb384",
    "1178acf6c5f0ab4af59b5aaec805e2ff63b25c3c18e7b3f2aa601805d75b0874",
    "11e730e6f30bfa0750baee3f99d2fec394dc31814eb47e1d3fe3cd376de9fd02",
    "159a0a3f202ee2181349d30a240b97217412c65b07cea6ddcf771e4a1d2ca60a",
    "1abc36543dc53a474e19b186a0f26bdced0d2c2649dc80adafeb3a89935d59a2",
    "1ec860df36e83ba5cdf51e6b3ad46f36b0489defa43e4feb5d1740286fd40529",
    "1f5709d2d713154ae93276855c966cd18eab476598b7a7e0dcc5c0f4a3ba0b94",
    "1ff5e8c02eebb6d352dd399ba035403e95fe266a24ef36e8960430817f13fba4",
    "26dc4c822c3752a155c01ff2a1ea77bcdc78788ba304075155b632141b2fa6fd",
    "281008a75e0bafc0542ce06fe495a63c0123495ca00f9417a7483123bd30f112",
    "28449118f0d77fdb533846be50487f9c66890ccc61bd471e7be24a31beebd797",
    "2850793053b5d5fe001a7fea8e7410f063c9c74995f6651935ae3a1caf7c1fe3",
    "290ddc4df22a6ed46fc47cbaf855a661a85567a3e5dcd120e29df95090bd81c4",
    "29c0381f01cda24b7f1790483dbd547b78ab1475e05318583080f0e2c77420fa",
    "2cc41fd30a2ace9635bc497c6d3ba7275708809202e33509b15d6acc75993e6f",
    "3358588e9b1493d92e23afd606c2ac585c4e23f37f1d56aeff5e52ba6c35185c",
    "38fd1c5d712bead9e9ae4ed56c7dbd8f0bb077230dd83979bdca54aedd721370",
    "3c8c45fbd76414e96d38a3a87dc852818120f747d35d96c159bcd990c919abc9",
    "41def19d1751a0614bd9dc5d3f374a21af00e818ad72ea09d4e4b902b6b94ab0",
    "4419cbf93108b801e3533995426942bbd0ac07bf5a0a85410afcf29df5f7689e",
    "477db27ef3f9b2443765042d7ba502741a38d8a2d71cf4e80d5dad27a6a68bb1",
    "47a0bfbcdfdac6a2ef2a926c3eeb9e88ba5e7dc985d00032172dbdbbea8df767",
    "492f3fb48bbf17c6794042617511350739a7a641901198aeb85af370be3171a4",
    "49d7bd687fd0eb38bed9273b5f31bd7c8ebe587e009469edfab8fe29e8ff3cec",
    "4a0f7a8953038f2ff47e0ca27687fb41efac5ebee11fdd12b0dfe49173feb285",
    "4d3b29d121c95c8fd7fe247e151fdac76c5da700e52f27393b6cb6ad64882ee9",
    "4dc4858f0641bad087542aaf0dff36c60c1dddfd26b9c2f37ae8c07f5922a359",
    "4dda7ca83553d0414a5b231c86fa3e9736f00515fa28551d9b6ee8d6b046e67c",
    "4ecd2d926d1d3652b22e51748c3ef4d9d65f7b11eaf033ff1b84c9e450e76be0",
    "53b02c326c47e54b3372136d197edc0ceb0ba5c4197946c10ca899b3aab071f1",
    "5be59461a282f77136b4a51ea09c814ad8ad3d5daa8ce4911c1b6dca25d57f43",
    "5e8420b4e457633c8c8b5598b5178d848e389ab7f6147ccd0fc2228902914fd8",
    "5eb872b4b9ead5bfd491acebfbdf451839f6f2e2eb895aa4e79b4ec83e4dcdc6",
    "621ca23af37dc87a259b42e024d307c4cfcd0863ba4481fa19d6e8253dbfad4c",
    "62a542ed9a359f2aaf6aaea9005b9919f57f1f8ff3193d6aac84c8cc9bbc1cb1",
    "6336ce287139c751e5d6a5317069809ffaecf9c421890b2fbcb34e1d6d9cbfb4",
    "645884c90b4cc48343506321a0f0aab5fb7cd410a72d30dbe379e1e2c3f6668a",
    "651479459da4efcd59c0e3f67e78f056b8f593c1c600021776f72c46751d1de9",
    "6b1110429307ab99211aab5aaf210c235cfd3f558fcaf22a0c8d0ca424901cdf",
    "7c6da55d1cb55026ad14af4357b672ed09819221e115ab3c9106489028a2cc5f",
    "80b59db7cb6535cffa9ca855d6b75af10cc4180b0587c69bbc9121b74bc57d89",
    "82e97b06c4aeca8d557778d890f34cddf4df14a3bbf334b971b6958e852edf76",
    "8315e7692743c5563c52988eda9398a9928146e45cb9ea5af60821e01b80f733",
    "84b94f469e873389eb1085db849bc74ee9be5e7bb80334582e98b5661c6aefa2",
    "854517bd38945ad4b4fb2f780d38688422c7b8ed88675329ce0fd7f9921f55f5",
    "8963c5f18d7193618f10501caa9d08de0fb0845415f6971fd7b2d3fd77bf0b26",
    "897a1f8d0656a51d2ce165a8aece58ad232b6bfa1abe254554674c4c0c7fd83b",
    "8d58bab9466f3f2b6f89f8a82ccd039921125b6d8e6afe9dcea3a43ae82fa48d",
    "8e2f0f44b4cae0b49907b227def060672a14f182efbe32cee1ffc77f4ed9b333",
    "90e457d9938a46a5c20750a4b8f9e7b7d3549e34b38b6c95a3b340c5bc67db51",
    "91873bf78c48c9a9d4534e500f94b7940855053c47ccd3e99a4b51362081f75a",
    "9e931bf5a5e405fe29cb69890a8bf56af218b9717dc9b3168b584e75915ef6d0",
    "a5d808d5dfd12035c2a27361dcf6b12139fc4e357a35d73338bb7408bff1182b",
    "ac4f30d4f903e128243000c2b96b578c1bebef8d31887e720f5e11923cd722b7",
    "b32ddef3715accd377d76c91a99ecbaeeb7d85417798f162f011c613a621f738",
    "bf6ea6f3f69cd63eb9416c589f9f63f160849b72108eda80d5d6e2623d39d443",
    "c04db9b428391e7c1e871ebd5896a721c1a76fa5ef677367ea8ad5753397be6a",
    "c0dda9fb4e200d8d68a830012cf57d6aed13cd5b9a1141e9dfc7cd5df5d55f36",
    "c1a0117beaff8fa4503e3ef2e8edfb5237e89ceb53a8337fd5aa94ca0e0d4c7c",
    "c1c0f15b236e9853d35397c556fb100efd5f319b57160ad682dee58fcbe58a93",
    "c5ecacec4ff4f782fe2c24b0ff41462584fd31a3366a65df4aa5fec3bd87a6be",
    "cba91ef24804086c14eedec58b785fe1f1c7e4a7a8505588309ab13c27629409",
    "cc41079a7321cfb2f04740a4e7cc3040a76add0b0b94bd670db90ff916de4834",
    "cc73d1cd4fbf23ef9ea07127c6a0fcdb5e276dbfccc9bf8a5ed9cd9b7388bca7",
    "cca7b25fba2b093536117d421efc63f485823aedb7dd287fb7769515dfd3cee5",
    "cd6327ede12bf62e1be86845e6777a2e016ada8514900326ef35ab280dd861c6",
    "d2c5f7ffbe38b59a3995876051928908924f53143c6fd1f7fac63115a38391c4",
    "d56cb32a984e90a502bae653d6b79f93f234c25dcc7a8b2fe9b61154d673a2a1",
    "d697934d055530ee6cdf646c4c388fd5fa5f0968b04e9dbf52136a6148047744",
    "db2f11731aed55d9525b6127451970d293f9ba20f281345ad2d06d65e0833b95",
    "ecdd5fce003e34ba8e47ccb8bac1b51e660174c56637abb81bd4e0eeb6f63135",
    "ee87f71a427e3c7acaf23ceee4e7f319eceb4bb327c0e67113902d62e24d42ca",
    "f0c38e50b6674e63a1a2fbd399bd53fb72de62a17f8be483dc31c4ee62ceddf0",
    "f2234e1ac1c55437f60468ce65eb730678970adf75316cb78e3054af7e563d9f",
    "f4576552ebf56c54955a8d5d05e0966ff2e875e8b131106a6ba9f2ac8960ab6e",
    "f9bfd7c27665e5f263a98de1a5cd103cfa886977f3fbb1def9abad0cfcd5f718",
    "fc0bf6354abec2654b17885539991a5c9d9ca02ad99c9686d2bb049ed315055f",
];

/// The fixture's named contributions, in the order the scenario writes them, by the ids it pins.
struct Named {
    marker: ContributionId,
    line: ContributionId,
    line_v2: ContributionId,
    one: ContributionId,
    two: ContributionId,
    d1: ContributionId,
    d2: ContributionId,
    d3: ContributionId,
    d4: ContributionId,
    p1: ContributionId,
    p2: ContributionId,
    resolution: ContributionId,
    goal: ContributionId,
    goal_link: ContributionId,
    review_d1: ContributionId,
    review_d3: ContributionId,
    objection: ContributionId,
    m1_pass: ContributionId,
    m4_fail: ContributionId,
    m4_pass: ContributionId,
    m2_pass_bob: ContributionId,
    m2_pass_carol: ContributionId,
    blanket_1: ContributionId,
    blanket_2: ContributionId,
    d5: ContributionId,
    d6: ContributionId,
    restored: ContributionId,
    d7: ContributionId,
    on_d7: ContributionId,
    d8: ContributionId,
    m6_pass_bob: ContributionId,
    m6_pass_carol: ContributionId,
    goal_backed: ContributionId,
    d9: ContributionId,
    side: ContributionId,
    s1: ContributionId,
    s1_fail: ContributionId,
    evidence_backed: ContributionId,
    s2: ContributionId,
    s3: ContributionId,
    s4: ContributionId,
    s5: ContributionId,
    s6: ContributionId,
    s7: ContributionId,
    on_s6: ContributionId,
    s8: ContributionId,
    s9a: ContributionId,
    s9b: ContributionId,
    s10: ContributionId,
    r1: ContributionId,
    r2: ContributionId,
    r3: ContributionId,
    r4: ContributionId,
    t1: ContributionId,
}

/// The fixture's manifests, by content: M1 (a.txt one), M2 (two), M3 (deux), M4 (the resolved merge).
fn manifests() -> [Manifest; 4] {
    [
        tree(&[("a.txt", b"one\n"), ("test.sh", TEST)]),
        tree(&[("a.txt", b"two\n"), ("test.sh", TEST)]),
        tree(&[("a.txt", b"deux\n"), ("test.sh", TEST)]),
        tree(&[("a.txt", b"two\ndeux\n"), ("test.sh", TEST)]),
    ]
}

const TEST: &[u8] = b"grep -qx two a.txt\n";

const RUN: &[u8] = b"#!/bin/sh\nsh test.sh\n";

/// The side line's trees: M7, a.txt two with an executable run.sh, and M8, the same with b.txt three
/// (whose file version D5's tree already wrote).
fn side_trees() -> [Manifest; 2] {
    let entry = |path: &str, bytes: &[u8], mode: Mode| Entry {
        path: path.into(),
        mode,
        hash: ContentHash::of(bytes),
        size: bytes.len() as u64,
    };
    let base = vec![
        entry("a.txt", b"two\n", Mode::File),
        entry("run.sh", RUN, Mode::Executable),
        entry("test.sh", TEST, Mode::File),
    ];
    let mut with_b = base.clone();
    with_b.push(entry("b.txt", b"three\n", Mode::File));
    [Manifest::new(base), Manifest::new(with_b)]
}

/// The manifests, then the trees D5 and D6's descendants record: M2 with b.txt saying three (M5), and
/// four (M6).
fn manifests_with_b() -> [Manifest; 6] {
    let [m1, m2, m3, m4] = manifests();
    let with_b = |b: &[u8]| tree(&[("a.txt", b"two\n"), ("b.txt", b), ("test.sh", TEST)]);
    [m1, m2, m3, m4, with_b(b"three\n"), with_b(b"four\n")]
}

/// The scenario, written into `store`, each contribution on the tip before it, one second apart, so
/// every id is fixed by its content.
fn write(store: &mut Store) -> Named {
    let (alice, bob, carol) = (identity(ALICE), identity(BOB), identity(CAROL));
    let mut tip: Vec<ContributionId> = vec![];
    let mut clock = 1_793_500_000_000u64;
    let mut add = |store: &mut Store, who: &Identity, body: Body| {
        clock += 1000;
        let c = Contribution::create(GROUND, who, std::mem::take(&mut tip), clock, body);
        let id = c.id();
        assert!(store.add(c).expect("a valid contribution"), "{id} was already held");
        tip = vec![id];
        id
    };
    let [m1, m2, m3, m4] = manifests();
    let (h1, h2, h3, h4) = (m1.content_hash(), m2.content_hash(), m3.content_hash(), m4.content_hash());

    // The Ground's marker and the line `main`, whose only validator is bob.
    let marker = add(store, &alice, artifact(&Marker { v: VERSION, purpose: "fixture".into(), in_place: None }));
    let line = add(store, &alice, artifact(&policy(&[&bob], 1)));

    // D1, the root: a.txt says one.
    add(store, &alice, file(b"one\n"));
    add(store, &alice, file(TEST));
    let one = add(store, &alice, artifact(&m1));
    let draft = |manifest: ContentHash, parents: Vec<ContributionId>| Decision {
        v: VERSION,
        line,
        manifest,
        parents,
        proposals: vec![],
        breakage: vec![],
        answers: vec![],
        resolutions: vec![],
        goals: vec![],
        source: None,
        note: String::new(),
    };
    let d1 = add(
        store,
        &alice,
        artifact(&Decision {
            source: Some(Source { git: "0123456789abcdef0123456789abcdef01234567".into() }),
            note: "the first tree".into(),
            ..draft(h1, vec![])
        }),
    );
    let m1_pass = add(store, &bob, result(one, &h1, 1, true));
    let agrees = add(store, &carol, claim("reads well", &[]));
    let review_d1 = add(store, &carol, relate(agrees, RelationKind::Supports, d1, "reads well"));

    // A goal alice sets on the line.
    let goal = add(store, &alice, claim("a.txt says two", &[GOAL_TAG]));
    let goal_link = add(store, &alice, relate(goal, RelationKind::Refines, line, ""));

    // Two proposals from D1 that both change a.txt: carol's to two, bob's to deux.
    add(store, &carol, file(b"two\n"));
    let two = add(store, &carol, artifact(&m2));
    let p1 = add(
        store,
        &carol,
        artifact(&Proposal {
            v: VERSION,
            line,
            base: d1,
            manifest: h2,
            change: Change::between(&m1, &m2).hash(),
            summary: "a.txt says two".into(),
        }),
    );
    add(store, &bob, file(b"deux\n"));
    add(store, &bob, artifact(&m3));
    let p2 = add(
        store,
        &bob,
        artifact(&Proposal {
            v: VERSION,
            line,
            base: d1,
            manifest: h3,
            change: Change::between(&m1, &m3).hash(),
            summary: "a.txt says deux".into(),
        }),
    );

    // alice resolves the conflict at a.txt with both lines, and integrates both proposals: D2.
    add(store, &alice, file(b"two\ndeux\n"));
    let resolution = add(
        store,
        &alice,
        artifact(&Resolution::new(
            "a.txt".into(),
            value(b"one\n"),
            [value(b"two\n"), value(b"deux\n")],
            value(b"two\ndeux\n"),
        )),
    );
    let four = add(store, &alice, artifact(&m4));
    let mut proposals = vec![p1, p2];
    proposals.sort();
    let d2 = add(
        store,
        &alice,
        artifact(&Decision { proposals, resolutions: vec![resolution], goals: vec![goal], ..draft(h4, vec![d1]) }),
    );
    // bob's check fails on D2's tree, then passes when run again.
    let m4_fail = add(store, &bob, result(four, &h4, 1, false));
    let m4_pass = add(store, &bob, result(four, &h4, 2, true));
    // carol objects to D2.
    let keep = add(store, &carol, claim("a.txt should hold one line", &[]));
    let objection = add(store, &carol, relate(keep, RelationKind::Contradicts, d2, "one line"));

    // A second policy version makes carol a validator too, and allows two known breakages.
    let line_v2 = add(store, &alice, artifact(&policy(&[&bob, &carol], 2)));
    add(store, &alice, relate(line_v2, RelationKind::Supersedes, line, "carol validates too"));

    // D3 goes back to two, answering carol's objection: fixed. Both validators pass it, under the new policy.
    let d3 = add(
        store,
        &alice,
        artifact(&Decision {
            answers: vec![Answer { objection, disposition: Disposition::Fixed, reason: "back to one line".into() }],
            note: "a.txt says two".into(),
            ..draft(h2, vec![d2])
        }),
    );
    let m2_pass_bob = add(store, &bob, result(two, &h2, 1, true));
    let m2_pass_carol = add(store, &carol, result(two, &h2, 1, true));

    // carol, not an integrator, writes a decision of her own: it doesn't qualify.
    let d4 = add(store, &carol, artifact(&draft(h1, vec![d3])));
    // carol's review of D3, retracted.
    let fine = add(store, &carol, claim("fine", &[]));
    let review_d3 = add(store, &carol, relate(fine, RelationKind::Supports, d3, ""));
    add(store, &carol, Body::Retract { target: review_d3, reason: "too quick".into() });

    // The cases of rule 5's settling and of answers that cover many objections (decisions 8cf20fe7669a and
    // 4afee4855944), appended so that every id above stays as it was. `fixed` needs a manifest that differs
    // from the objected decision's, so D5 and D8 each record a new tree, with b.txt added.
    let [_, _, _, _, m5, m6] = manifests_with_b();
    let (h5, h6) = (m5.content_hash(), m6.content_hash());

    // A blanket answer, and settled objections: bob objects to D3 twice. D5 answers both with one answer
    // naming D3 (fixed); D6, below D5, answers neither and still qualifies, as both are settled at D5.
    let slow = add(store, &bob, claim("test.sh is slow", &[]));
    let blanket_1 = add(store, &bob, relate(slow, RelationKind::Contradicts, d3, "slow"));
    let terse = add(store, &bob, claim("the note is terse", &[]));
    let blanket_2 = add(store, &bob, relate(terse, RelationKind::Contradicts, d3, "terse"));
    add(store, &alice, file(b"three\n"));
    add(store, &alice, artifact(&m5));
    let fixed = Answer { objection: d3, disposition: Disposition::Fixed, reason: "both seen to".into() };
    let d5 = add(store, &alice, artifact(&Decision { answers: vec![fixed], ..draft(h5, vec![d3]) }));
    let d6 = add(store, &alice, artifact(&draft(h5, vec![d5])));

    // A restored objection: carol objects to D6 and retracts it, so D7 qualifies without answering it.
    // Then she retracts the retraction, which restores it: it was never judged at D7, so it isn't
    // settled, and binds the next move (finding 0bf37b345567's second case).
    let wrong = add(store, &carol, claim("a.txt is wrong", &[]));
    let restored = add(store, &carol, relate(wrong, RelationKind::Contradicts, d6, "wrong"));
    let withdrawn = add(store, &carol, Body::Retract { target: restored, reason: "spoke too soon".into() });
    let d7 = add(store, &alice, artifact(&draft(h5, vec![d6])));
    add(store, &carol, Body::Retract { target: withdrawn, reason: "I was right".into() });

    // A line-wide answer: bob objects to D7 too. D8's one answer naming the line (fixed, on a new tree)
    // answers both objections in its window, on two ancestor decisions, and settles them. Both validators
    // pass D8's tree.
    let three = add(store, &bob, claim("b.txt should say four", &[]));
    let on_d7 = add(store, &bob, relate(three, RelationKind::Contradicts, d7, "four"));
    add(store, &alice, file(b"four\n"));
    let six = add(store, &alice, artifact(&m6));
    let line_wide = Answer { objection: line, disposition: Disposition::Fixed, reason: "b.txt says four".into() };
    let d8 = add(store, &alice, artifact(&Decision { answers: vec![line_wide], ..draft(h6, vec![d7]) }));
    let m6_pass_bob = add(store, &bob, result(six, &h6, 1, true));
    let m6_pass_carol = add(store, &carol, result(six, &h6, 1, true));

    // A goal-backed objection that stays open: carol objects to D8 from alice's binding goal. D9's
    // blanket overridden on D8 covers it, but overridden isn't allowed for a goal-backed objection, so D9
    // doesn't qualify, and the objection stays open.
    let goal_backed = add(store, &carol, relate(goal, RelationKind::Contradicts, d8, "a.txt must say two"));
    let overridden = Answer { objection: d8, disposition: Disposition::Overridden, reason: "shipping anyway".into() };
    let d9 = add(store, &alice, artifact(&Decision { answers: vec![overridden], ..draft(h6, vec![d8]) }));

    // A second line, `side`, holds the cases of rule 6 and the other dispositions, so that `main` and its
    // open goal-backed objection stay as they are. Its policy is `main`'s second version under another
    // name: bob and carol validate, at most two known breakages, three moves old, none under docs.
    let side_policy = Line { name: "side".into(), ..policy(&[&bob, &carol], 2) };
    let side = add(store, &alice, artifact(&side_policy));
    let on_side =
        |manifest: ContentHash, parents: Vec<ContributionId>| Decision { line: side, ..draft(manifest, parents) };
    let [m7, m8] = side_trees();
    let (h7, h8) = (m7.content_hash(), m8.content_hash());
    let broken = |paths: &[&str]| Breakage {
        check: "test".into(),
        detail: "a.txt says two, not one".into(),
        paths: paths.iter().map(|p| p.to_string()).collect(),
    };

    // S1, the root: its tree has an executable run.sh, a mode other than a plain file.
    add(store, &alice, file(RUN));
    let seven = add(store, &alice, artifact(&m7));
    let s1 = add(store, &alice, artifact(&on_side(h7, vec![])));
    // An evidence-backed objection: bob's check fails on S1's tree, and he objects from that result.
    let s1_fail = add(store, &bob, result(seven, &h7, 1, false));
    let evidence_backed = add(store, &bob, relate(s1_fail, RelationKind::Contradicts, s1, "test fails"));
    // S2 answers it fixed on a new tree, but still lists the failing check as breakage: fixed is refused.
    add(store, &alice, artifact(&m8));
    let fixed = Answer { objection: evidence_backed, disposition: Disposition::Fixed, reason: "fixed".into() };
    let s2 = add(
        store,
        &alice,
        artifact(&Decision { answers: vec![fixed], breakage: vec![broken(&["a.txt"])], ..on_side(h8, vec![s1]) }),
    );
    // S3 accepts the breakage instead, under the guardrails: it qualifies.
    let accepted =
        Answer { objection: evidence_backed, disposition: Disposition::AcceptedBreakage, reason: "known".into() };
    let s3 = add(
        store,
        &alice,
        artifact(&Decision { answers: vec![accepted], breakage: vec![broken(&["a.txt"])], ..on_side(h7, vec![s1]) }),
    );
    // S4 names a protected path in its breakage: refused, always.
    let s4 =
        add(store, &alice, artifact(&Decision { breakage: vec![broken(&["docs/x.md"])], ..on_side(h7, vec![s3]) }));
    // S5 and S6 carry the breakage on, 2 and 3 moves old, within max_age 3; S7 would make it 4: refused.
    let s5 = add(store, &alice, artifact(&Decision { breakage: vec![broken(&["a.txt"])], ..on_side(h8, vec![s3]) }));
    let s6 = add(store, &alice, artifact(&Decision { breakage: vec![broken(&["a.txt"])], ..on_side(h8, vec![s5]) }));
    let s7 = add(store, &alice, artifact(&Decision { breakage: vec![broken(&["a.txt"])], ..on_side(h8, vec![s6]) }));
    // carol objects to S6; S8 falls back to S3's tree, an ancestor's, with no breakage: fell_back.
    let doubt = add(store, &carol, claim("b.txt should say three", &[]));
    let on_s6 = add(store, &carol, relate(doubt, RelationKind::Contradicts, s6, "three"));
    let fell_back = Answer { objection: on_s6, disposition: Disposition::FellBack, reason: "back to S3".into() };
    let s8 = add(store, &alice, artifact(&Decision { answers: vec![fell_back], ..on_side(h7, vec![s6]) }));
    // A fork: two decisions on S8. The merge, last of all, has both as parents and settles it.
    let s9a = add(store, &alice, artifact(&Decision { note: "a".into(), ..on_side(h7, vec![s8]) }));
    let s9b = add(store, &alice, artifact(&Decision { note: "b".into(), ..on_side(h8, vec![s8]) }));
    let mut forked = vec![s9a, s9b];
    forked.sort();
    let s10 = add(store, &alice, artifact(&on_side(h7, forked)));

    // Rule 6's count and its reducing exception (finding 05acef0e6c81), appended so every id above stays.
    // R1, R2 and R3 list breakage for test and lint, 1, 2 and 3 moves old. R4 lists test alone, 4 moves
    // old, past max_age 3, but its breakage is a strict subset of its parent's: it qualifies, reducing.
    let entry = |check: &str| Breakage { check: check.into(), detail: "known".into(), paths: vec!["a.txt".into()] };
    let both = || vec![entry("lint"), entry("test")];
    let r1 = add(store, &alice, artifact(&Decision { breakage: both(), ..on_side(h7, vec![s10]) }));
    let r2 = add(store, &alice, artifact(&Decision { breakage: both(), ..on_side(h7, vec![r1]) }));
    let r3 = add(store, &alice, artifact(&Decision { breakage: both(), ..on_side(h7, vec![r2]) }));
    let r4 = add(store, &alice, artifact(&Decision { breakage: vec![entry("test")], ..on_side(h7, vec![r3]) }));
    // T1 lists three entries, past max_known 2, and R4's breakage holds only test, so it doesn't reduce:
    // refused.
    let three = vec![entry("doc"), entry("lint"), entry("test")];
    let t1 = add(store, &alice, artifact(&Decision { breakage: three, ..on_side(h7, vec![r4]) }));

    let _ = h3;
    Named {
        marker,
        line,
        line_v2,
        one,
        two,
        d1,
        d2,
        d3,
        d4,
        p1,
        p2,
        resolution,
        goal,
        goal_link,
        review_d1,
        review_d3,
        objection,
        m1_pass,
        m4_fail,
        m4_pass,
        m2_pass_bob,
        m2_pass_carol,
        blanket_1,
        blanket_2,
        d5,
        d6,
        restored,
        d7,
        on_d7,
        d8,
        m6_pass_bob,
        m6_pass_carol,
        goal_backed,
        d9,
        side,
        s1,
        s1_fail,
        evidence_backed,
        s2,
        s3,
        s4,
        s5,
        s6,
        s7,
        on_s6,
        s8,
        s9a,
        s9b,
        s10,
        r1,
        r2,
        r3,
        r4,
        t1,
    }
}

/// The scenario's ids, by writing it again into a scratch Ground: they depend only on content.
fn named() -> Named {
    let tmp = tempfile::tempdir().expect("a scratch directory");
    let mut store =
        Store::create(&tmp.path().join("g"), GroundInfo { ground: GROUND, name: "weave-v1".into() }).expect("create");
    write(&mut store)
}

#[test]
fn the_fixture_opens_and_every_record_reads_back_to_its_bytes() {
    let dir = fixture_dir();
    let store = Store::open(&dir).expect("the Weave fixture Ground opens");
    assert_eq!(store.info().ground, GROUND);
    assert_eq!(store.info().name, "weave-v1");
    let g = store.graph();
    let mut ids: Vec<String> = g.iter().map(|c| c.id().to_hex()).collect();
    ids.sort();
    assert_eq!(ids, FIXTURE_IDS);
    for c in g.iter() {
        assert!(c.signature_is_valid(), "{} signature no longer verifies", c.id());
        let hex = c.id().to_hex();
        let bytes = std::fs::read(dir.join("objects").join(&hex[..2]).join(&hex[2..])).expect("the object");
        assert_eq!(c.encode(), bytes, "{} re-encodes differently", c.id());
    }

    // A record of every Weave media type, and each decodes and re-encodes to its payload.
    let mut media = BTreeSet::new();
    for c in g.iter() {
        let Body::Artifact { media_type, content, .. } = c.body() else { continue };
        if !media_type.starts_with(MEDIA_TYPE_PREFIX) {
            continue;
        }
        media.insert(media_type.as_str());
        let again = match media_type.as_str() {
            m if m == FILE_MEDIA_TYPE => content.clone(),
            m if m == Manifest::MEDIA_TYPE => Manifest::decode(content).expect("a manifest").encode(),
            m if m == Marker::MEDIA_TYPE => Marker::decode(content).expect("a marker").encode(),
            m if m == Line::MEDIA_TYPE => Line::decode(content).expect("a line").encode(),
            m if m == Decision::MEDIA_TYPE => Decision::decode(content).expect("a decision").encode(),
            m if m == Proposal::MEDIA_TYPE => Proposal::decode(content).expect("a proposal").encode(),
            m if m == Resolution::MEDIA_TYPE => Resolution::decode(content).expect("a resolution").encode(),
            other => panic!("{}: a Weave media type this test doesn't know: {other}", c.id()),
        };
        assert_eq!(&again, content, "{} re-encodes its payload differently", c.id());
    }
    let every = [
        FILE_MEDIA_TYPE,
        Manifest::MEDIA_TYPE,
        Marker::MEDIA_TYPE,
        Line::MEDIA_TYPE,
        Decision::MEDIA_TYPE,
        Proposal::MEDIA_TYPE,
        Resolution::MEDIA_TYPE,
    ];
    assert_eq!(media, BTreeSet::from(every));

    let agents: Vec<(String, String)> =
        store.agents().expect("agents").into_iter().map(|e| (e.name, e.agent.to_hex())).collect();
    let expected: Vec<(String, String)> =
        [ALICE, BOB, CAROL].iter().map(|(name, _, key)| (name.to_string(), key.to_string())).collect();
    assert_eq!(agents, expected);
}

#[test]
fn every_derived_status_is_as_pinned() {
    let store = Store::open(&fixture_dir()).expect("the Weave fixture Ground opens");
    let g = store.graph();
    let n = named();
    let (alice, bob, carol) = (identity(ALICE).agent_id(), identity(BOB).agent_id(), identity(CAROL).agent_id());
    let [m1, m2, m3, m4] = manifests().map(|m| m.content_hash());

    let ix = Index::build(g);
    assert_eq!(ix.counted_marker(), Some(n.marker));
    assert_eq!(ix.problems(), &[]);
    for (hash, kind) in [(m1, "manifest"), (m2, "manifest"), (m3, "manifest"), (m4, "manifest")] {
        assert_eq!(ix.hash_kind(&hash), Some(kind), "{hash}");
    }
    for bytes in [&b"one\n"[..], b"two\n", b"deux\n", b"two\ndeux\n", TEST] {
        assert_eq!(ix.hash_kind(&ContentHash::of(bytes)), Some("file"));
    }
    assert_eq!(ix.manifest(&m2).map(|(id, _)| id), Some(n.two));
    assert_eq!(ix.manifest(&m1).map(|(id, _)| id), Some(n.one));
    assert!(ix.missing(&manifests()[3]).is_empty(), "D2's tree is complete");
    // alice's resolution of the conflict at a.txt, between two and deux, from one.
    let key = Key {
        path: "a.txt".into(),
        base: value(b"one\n"),
        sides: Resolution::new("a.txt".into(), None, [value(b"two\n"), value(b"deux\n")], None).sides,
    };
    let held: Vec<ContributionId> = ix.resolutions().for_key(&key).iter().map(|(id, _)| *id).collect();
    assert_eq!(held, [n.resolution]);

    // The line, its policy versions, and every decision's verdict and the policy in force for it.
    let lines = Lines::build(g, &ix);
    assert!(lines.objections().differ_from_walking(&ix, &lines).is_empty(), "windows and a walk disagree");
    assert_eq!(lines.default_line(), Some(n.line));
    let mut both = vec![n.line, n.side];
    both.sort();
    assert_eq!(lines.lines().map(|v| v.id).collect::<Vec<_>>(), both);
    assert_eq!(lines.current_versions(&n.line), [n.line_v2]);
    assert_eq!(lines.ignored_versions(), &[]);
    let qualified = |policy| Ok(Qualified { line: n.line, policy });
    assert_eq!(lines.verdict(&n.d1), Some(&qualified(n.line)));
    assert_eq!(lines.verdict(&n.d2), Some(&qualified(n.line)));
    assert_eq!(lines.verdict(&n.d3), Some(&qualified(n.line_v2)));
    assert_eq!(lines.verdict(&n.d4), Some(&Err(Unqualified::NotAnIntegrator { author: carol, policy: n.line_v2 })));
    // Rule 5's answers and settling. D5's one answer naming D3 answers both of bob's objections, which
    // are settled at D5, so D6 needn't answer them. D7 didn't have to answer carol's objection, retracted
    // then. D8's one answer naming the line answers it, restored, and bob's objection to D7. D9's blanket
    // overridden covers the goal-backed objection, but isn't allowed for it.
    for d in [n.d5, n.d6, n.d7, n.d8] {
        assert_eq!(lines.verdict(&d), Some(&qualified(n.line_v2)), "{d}");
    }
    assert_eq!(
        lines.verdict(&n.d9),
        Some(&Err(Unqualified::AnswerNotAllowed {
            objection: n.goal_backed,
            disposition: Disposition::Overridden,
            basis: "goal-backed"
        }))
    );
    assert_eq!(lines.heads(&n.line), [n.d8]);
    let log: Vec<ContributionId> = lines.log(g, &n.line, None).iter().map(|d| d.id).collect();
    assert_eq!(log, [n.d8, n.d7, n.d6, n.d5, n.d3, n.d2, n.d1]);
    for p in [n.p1, n.p2] {
        assert_eq!(lines.proposal(&p).map(|r| r.verdict.clone()), Some(Ok(())), "{p}");
    }
    assert_eq!(lines.proposal(&n.p1).map(|r| r.author), Some(carol));
    assert_eq!(lines.proposal(&n.p2).map(|r| r.author), Some(bob));

    // Results: bob's under the first policy don't count under the second (the epoch); D3's tree has a
    // pass from each validator, so D3 is green.
    let checks = Checks::build(g, &ix, &lines);
    assert_eq!(checks.green(g, &lines, &n.line), Green::One(n.d8));
    let counted = checks.counted(g, &lines, &n.d3).expect("D3 is a decision");
    assert_eq!(counted.len(), 1);
    let mut passes = vec![n.m2_pass_bob, n.m2_pass_carol];
    passes.sort();
    let mut got = counted[0].passes.clone();
    got.sort();
    assert_eq!((counted[0].check.as_str(), got, counted[0].fails.clone()), ("test", passes, vec![]));
    // D8, the green head: both validators passed its tree.
    let d8 = checks.counted(g, &lines, &n.d8).expect("D8 is a decision");
    let mut passes = vec![n.m6_pass_bob, n.m6_pass_carol];
    passes.sort();
    let mut got = d8[0].passes.clone();
    got.sort();
    assert_eq!((got, d8[0].fails.clone()), (passes, vec![]));
    let d2 = checks.counted(g, &lines, &n.d2).expect("D2 is a decision");
    assert_eq!((d2[0].passes.clone(), d2[0].fails.clone(), d2[0].replaced.clone()), (vec![], vec![], vec![]));
    assert_eq!(checks.latest(&bob, &m4, &method()), [n.m4_pass]);
    assert_eq!(checks.next_attempt(&bob, &m4, &method()), 3);
    assert_eq!(checks.latest(&bob, &m1, &method()), [n.m1_pass]);
    assert_eq!(checks.latest(&alice, &m1, &method()), &[] as &[ContributionId]);
    let _ = n.m4_fail;

    // Reviews: carol's of D1, her objection to D2, and her retracted review of D3.
    let reviews = Reviews::build(&ix, &lines);
    let of = |d: &ContributionId| -> Vec<(ContributionId, bool, bool)> {
        reviews.of_decision(&lines, d).iter().map(|(r, _)| (r.id, r.objection, r.retracted)).collect()
    };
    assert_eq!(of(&n.d1), [(n.review_d1, false, false)]);
    assert_eq!(of(&n.d2), [(n.objection, true, false)]);
    // The view of now leaves out a review retracted now (carol's of D3); the index still holds it, marked.
    let mut bobs = vec![(n.blanket_1, true, false), (n.blanket_2, true, false)];
    bobs.sort();
    assert_eq!(of(&n.d3), bobs);
    let held: Vec<(ContributionId, bool)> =
        ix.reviews().iter().filter(|r| r.to == n.d3 && !r.objection).map(|r| (r.id, r.retracted)).collect();
    assert_eq!(held, [(n.review_d3, true)]);
    // The restored objection is listed as not retracted; bob's on D7 and the goal-backed one on D8 too.
    assert_eq!(of(&n.d6), [(n.restored, true, false)]);
    assert_eq!(of(&n.d7), [(n.on_d7, true, false)]);
    assert_eq!(of(&n.d8), [(n.goal_backed, true, false)]);

    // alice's goal binds the line under its current policy.
    let set = goals(&ix, &n.line);
    assert_eq!(set.iter().map(|goal| (goal.id, goal.link)).collect::<Vec<_>>(), [(n.goal, n.goal_link)]);
    let current = &lines.version(&n.line_v2).expect("the current version").policy;
    assert!(set[0].binds(current));

    // Open on the line: only the goal-backed objection, which D9's blanket overridden couldn't answer.
    // Every other objection was answered and settled: bob's two at D5, the restored one and bob's
    // objection to D7 at D8. Nothing is open on either proposal.
    let open = lines.open_objections(g, &ix, &n.line).expect("the line has a current policy");
    let got: Vec<(ContributionId, &str)> = open.iter().map(|o| (o.objection.id, o.basis.kind())).collect();
    assert_eq!(got, [(n.goal_backed, "goal-backed")]);
    // A goal-backed objection allows exactly fixed and fell_back.
    assert_eq!(open[0].allowed, [Disposition::Fixed, Disposition::FellBack]);
    for p in [n.p1, n.p2] {
        assert_eq!(lines.proposal_objections(g, &ix, &p), Some(vec![]), "{p}");
    }

    // The side line: rule 6, the evidence-backed basis, fell_back, accepted_breakage, a fork and a merge.
    assert_eq!(lines.current_versions(&n.side), [n.side]);
    let on_side = |d: &ContributionId| lines.verdict(d).cloned();
    let ok = Some(Ok(Qualified { line: n.side, policy: n.side }));
    for d in [n.s1, n.s3, n.s5, n.s6, n.s8, n.s9a, n.s9b, n.s10] {
        assert_eq!(on_side(&d), ok, "{d}");
    }
    // fixed is refused while the failing check is still listed as breakage.
    let refused = Unqualified::AnswerNotAllowed {
        objection: n.evidence_backed,
        disposition: Disposition::Fixed,
        basis: "evidence-backed",
    };
    assert_eq!(on_side(&n.s2), Some(Err(refused)));
    // A protected path, always refused.
    let protected = Unqualified::ProtectedPath { check: "test".into(), path: "docs/x.md".into() };
    assert_eq!(on_side(&n.s4), Some(Err(protected)));
    // S3, S5 and S6 list test in an unbroken chain: S7 would make it 4 moves old, past max_age 3.
    assert_eq!(on_side(&n.s7), Some(Err(Unqualified::BreakageTooOld { check: "test".into(), age: 4, max: 3 })));
    // R4 qualifies by reducing: test is 4 moves old, past max_age 3, but {test} is a strict subset of
    // R3's {lint, test}. T1's three entries pass max_known 2, and don't reduce R4's one.
    for d in [n.r1, n.r2, n.r3, n.r4] {
        assert_eq!(on_side(&d), ok, "{d}");
    }
    assert_eq!(on_side(&n.t1), Some(Err(Unqualified::TooMuchBreakage { count: 3, max: 2 })));
    assert_eq!(lines.heads(&n.side), [n.r4]);
    // The merge settles the fork: the fork's heads, on the Ground as it was before the merge, are S9a
    // and S9b, whose merge base is S8; and S10, the merge, is below the head.
    assert!(lines.decision_ancestry(&n.r4).contains(&n.s10));
    let mut forked = vec![n.s9a, n.s9b];
    forked.sort();
    assert_eq!(lines.merge_bases(&n.s9a, &n.s9b), [n.s8]);
    let mut before = manyroot::Graph::new(GROUND);
    // The scenario writes each on the one before it, one second apart, so time order is causal order:
    // the Ground before the merge is everything written before it.
    let merged_at = g.get(&n.s10).expect("the merge").content().timestamp_ms;
    let mut chain: Vec<&Contribution> = g.iter().filter(|c| c.content().timestamp_ms < merged_at).collect();
    chain.sort_by_key(|c| c.content().timestamp_ms);
    for c in chain {
        before.insert(c.clone()).expect("each one's parents come first");
    }
    let before_ix = Index::build(&before);
    let before_lines = Lines::build(&before, &before_ix);
    assert!(
        before_lines.objections().differ_from_walking(&before_ix, &before_lines).is_empty(),
        "windows and a walk disagree"
    );
    assert_eq!(before_lines.heads(&n.side), forked);
    // Both objections on the side line were answered and settled: none is open.
    assert_eq!(lines.open_objections(g, &ix, &n.side), Some(vec![]));
    // bob's fail on S1's tree is the latest result there, so no decision on the side line is green.
    assert_eq!(checks.green(g, &lines, &n.side), Green::None);
    assert_eq!(checks.latest(&bob, &side_tree_hash(0), &method()), [n.s1_fail]);
    let _ = n.on_s6;
}

/// The content hash of the side line's tree `i` (M7 or M8).
fn side_tree_hash(i: usize) -> ContentHash {
    side_trees()[i].content_hash()
}

// The generator rebuilds the committed fixture byte for byte, so its ids are fixed by the scenario
// above alone, and the names the status test uses are the fixture's own.
#[test]
fn the_scenario_rebuilds_the_fixture_byte_for_byte() {
    let tmp = tempfile::tempdir().expect("a scratch directory");
    let dir = tmp.path().join("g");
    let mut store = Store::create(&dir, GroundInfo { ground: GROUND, name: "weave-v1".into() }).expect("create");
    write(&mut store);
    let objects = |root: &Path| -> BTreeMap<PathBuf, Vec<u8>> {
        let mut found = BTreeMap::new();
        let mut stack = vec![root.join("objects")];
        while let Some(at) = stack.pop() {
            for entry in std::fs::read_dir(&at).expect("read objects") {
                let path = entry.expect("an entry").path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    let bytes = std::fs::read(&path).expect("read");
                    found.insert(path.strip_prefix(root).expect("inside").to_path_buf(), bytes);
                }
            }
        }
        found
    };
    assert_eq!(objects(&dir), objects(&fixture_dir()));
}

#[test]
fn the_fixture_holds_no_private_keys() {
    let dir = fixture_dir();
    assert!(!dir.join("identities").exists());
    let mut forbidden: Vec<Vec<u8>> = vec![["secret", "key"].join("_").into_bytes()];
    for (_, seed, public_key) in [ALICE, BOB, CAROL] {
        let keypair = [seed.as_slice(), &hex::decode(public_key).expect("hex")].concat();
        for secret in [seed.to_vec(), keypair] {
            forbidden.push(hex::encode(&secret).into_bytes());
            forbidden.push(hex::encode_upper(&secret).into_bytes());
            forbidden.push(secret);
        }
    }
    let mut stack = vec![dir];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read the fixture") {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let bytes = std::fs::read(&path).expect("read");
            for secret in &forbidden {
                assert!(!bytes.windows(secret.len()).any(|w| w == secret), "{} holds a key", path.display());
            }
        }
    }
}

/// Rebuilds the fixture. Rebuilding it changes what existing Weave Grounds are pinned to: protocol class
/// (docs/design/weave.md, section 5). Refuses to overwrite an existing fixture.
///
/// ```text
/// rm -r crates/tree/tests/fixtures/weave-v1
/// cargo test -p bracken-tree --test weave_fixture -- --ignored write_weave_v1_fixture
/// ```
#[test]
#[ignore = "rewrites the checked-in fixture; protocol class, see docs/design/weave.md section 5"]
fn write_weave_v1_fixture() {
    let dir = fixture_dir();
    let mut store = Store::create(&dir, GroundInfo { ground: GROUND, name: "weave-v1".into() }).expect("create");
    std::fs::remove_dir(store.identities_dir()).expect("a new Ground's identities/ is empty");
    std::fs::create_dir_all(store.agents_dir()).expect("mkdir agents");
    for who in [ALICE, BOB, CAROL].map(identity) {
        let entry = AgentEntry { name: who.name().into(), agent: who.agent_id() };
        let json = serde_json::to_string_pretty(&entry).expect("JSON") + "\n";
        std::fs::write(store.agents_dir().join(format!("{}.json", who.name())), json).expect("write");
    }
    write(&mut store);
}
