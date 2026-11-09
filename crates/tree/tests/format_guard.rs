//! Format guard: Weave's payload formats are pinned like the protocol
//! (docs/design/weave.md, section 5). One record of every Weave media type is
//! checked against its exact bytes and content hash. If a test here fails, a
//! change altered a format that existing Weave Grounds hold: that is protocol
//! class, with an independent review, the manager's go and a note in
//! docs/releases.md. Don't update a golden vector to make it pass.

use std::collections::BTreeMap;

use bracken_tree::format::{
    self, DECISION_MEDIA_TYPE, Executor, ExecutorKind, FILE_MEDIA_TYPE, LINE_MEDIA_TYPE, MANIFEST_MEDIA_TYPE,
    MARKER_MEDIA_TYPE, MAX_REFERENCES, PROPOSAL_MEDIA_TYPE, RESOLUTION_MEDIA_TYPE, from_cbor, parse_method, to_cbor,
};
use bracken_tree::{
    Answer, Breakage, Change, CheckDefinition, ContentHash, Decision, Disposition, Entry, FileValue, FormatError,
    Guardrails, Line, Manifest, Marker, Mode, Proposal, Record, Resolution, Source,
};
use ciborium::Value;
use manyroot::{AgentId, ContributionId};

fn id(n: u8) -> ContributionId {
    ContributionId([n; 32])
}

fn agent(n: u8) -> AgentId {
    AgentId([n; 32])
}

const FILE_BYTES: &[u8] = b"fn main() {}\n";

fn file_value(bytes: &[u8], mode: Mode) -> FileValue {
    FileValue { hash: ContentHash::of(bytes), mode }
}

fn entry(path: &str, bytes: &[u8], mode: Mode) -> Entry {
    Entry { path: path.to_owned(), mode, hash: ContentHash::of(bytes), size: bytes.len() as u64 }
}

fn manifest() -> Manifest {
    Manifest::new(vec![
        entry("src/main.rs", FILE_BYTES, Mode::File),
        entry("run.sh", b"#!/bin/sh\n", Mode::Executable),
        entry("latest", b"src/main.rs", Mode::Symlink),
    ])
}

fn marker() -> Marker {
    Marker { v: 1, purpose: "dogfood".to_owned(), in_place: None }
}

fn line() -> Line {
    let mut checks = BTreeMap::new();
    checks.insert("test".to_owned(), vec!["cargo".to_owned(), "test".to_owned(), "--workspace".to_owned()]);
    checks.insert("fmt".to_owned(), vec!["cargo".to_owned(), "fmt".to_owned(), "--check".to_owned()]);
    Line {
        v: 1,
        name: "main".to_owned(),
        integrators: vec![agent(0xa1)],
        validators: vec![agent(0xa2)],
        goal_setters: vec![agent(0xa3)],
        checks,
        executors: BTreeMap::new(),
        guardrails: Guardrails { max_known: 3, max_age: 5, protected: vec!["crates/manyroot".to_owned()] },
        owner_decision: Some("46b91e01edc2".to_owned()),
    }
}

fn decision() -> Decision {
    Decision {
        v: 1,
        line: id(1),
        manifest: manifest().content_hash(),
        parents: vec![id(2), id(3)],
        proposals: vec![id(4)],
        breakage: vec![Breakage {
            check: "test".to_owned(),
            detail: "flaky".to_owned(),
            paths: vec!["src/main.rs".to_owned()],
        }],
        answers: vec![Answer {
            objection: id(5),
            disposition: Disposition::AcceptedBreakage,
            reason: "known".to_owned(),
        }],
        resolutions: vec![id(6)],
        goals: vec![id(7)],
        source: Some(Source { git: "bef07d92".to_owned() }),
        note: "first".to_owned(),
    }
}

fn proposal() -> Proposal {
    Proposal {
        v: 1,
        line: id(1),
        base: id(2),
        manifest: manifest().content_hash(),
        change: ContentHash::of(b"change"),
        summary: "add run.sh".to_owned(),
    }
}

fn resolution() -> Resolution {
    Resolution::new(
        "src/main.rs".to_owned(),
        Some(file_value(b"base", Mode::File)),
        [Some(file_value(b"ours", Mode::File)), None],
        Some(file_value(b"merged", Mode::Executable)),
    )
}

/// One payload of every media type.
fn vectors() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        (FILE_MEDIA_TYPE, FILE_BYTES.to_vec()),
        (MANIFEST_MEDIA_TYPE, manifest().encode()),
        (MARKER_MEDIA_TYPE, marker().encode()),
        (LINE_MEDIA_TYPE, line().encode()),
        (DECISION_MEDIA_TYPE, decision().encode()),
        (PROPOSAL_MEDIA_TYPE, proposal().encode()),
        (RESOLUTION_MEDIA_TYPE, resolution().encode()),
    ]
}

/// (media type, payload hex, content hash). Pinned: see the module comment.
const GOLDEN: &[(&str, &str, &str)] = &[
    (
        "application/vnd.bracken.weave.file",
        "666e206d61696e2829207b7d0a",
        "blake3:2d1ebfa706ba230165250f744796a92accba5e1b6fa357983b65319da33f8e93",
    ),
    (
        "application/vnd.bracken.weave.manifest+cbor",
        "a261760167656e74726965738384666c617465737419a00058206d9bc68d5fc74f698ad6621febe779b24b1882c1a4c3e4635b80b5b3534512ca0b846672756e2e73681981ed5820bc1f407a11c9377c8b9b13f956b279c8462775105eb958fc9ae3c40de87cc96e0a846b7372632f6d61696e2e72731981a458202d1ebfa706ba230165250f744796a92accba5e1b6fa357983b65319da33f8e930d",
        "blake3:cf8a96e087621b97a95bff66994b6e31d29275b19b5e2ef390f101f88f923568",
    ),
    (
        "application/vnd.bracken.weave.ground+cbor",
        "a361760167707572706f736567646f67666f6f6468696e5f706c616365f6",
        "blake3:28778beefe9ec90d43afdcc63db49329004af5430afdadb77b2f59b82a56d908",
    ),
    (
        "application/vnd.bracken.weave.line+cbor",
        "a8617601646e616d65646d61696e6b696e7465677261746f7273815820a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a16a76616c696461746f7273815820a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a26c676f616c5f73657474657273815820a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a366636865636b73a263666d748365636172676f63666d74672d2d636865636b64746573748365636172676f64746573746b2d2d776f726b73706163656a67756172647261696c73a3696d61785f6b6e6f776e03676d61785f616765056970726f746563746564816f6372617465732f6d616e79726f6f746e6f776e65725f6465636973696f6e6c343662393165303165646332",
        "blake3:14f4f5b3db944409ca5f4a07bc4b7eea29cb61d95d0b8b46b69e965d64da26b4",
    ),
    (
        "application/vnd.bracken.weave.decision+cbor",
        "ab617601646c696e6558200101010101010101010101010101010101010101010101010101010101010101686d616e69666573745820cf8a96e087621b97a95bff66994b6e31d29275b19b5e2ef390f101f88f92356867706172656e74738258200202020202020202020202020202020202020202020202020202020202020202582003030303030303030303030303030303030303030303030303030303030303036970726f706f73616c73815820040404040404040404040404040404040404040404040404040404040404040468627265616b61676581a365636865636b64746573746664657461696c65666c616b79657061746873816b7372632f6d61696e2e727367616e737765727381a3696f626a656374696f6e582005050505050505050505050505050505050505050505050505050505050505056b646973706f736974696f6e7161636365707465645f627265616b61676566726561736f6e656b6e6f776e6b7265736f6c7574696f6e73815820060606060606060606060606060606060606060606060606060606060606060665676f616c73815820070707070707070707070707070707070707070707070707070707070707070766736f75726365a163676974686265663037643932646e6f7465656669727374",
        "blake3:a9bcbc4f0b8bf8eb86d9a8a256a135df5788fe1f743d2bdd1a0653c595bac246",
    ),
    (
        "application/vnd.bracken.weave.proposal+cbor",
        "a6617601646c696e6558200101010101010101010101010101010101010101010101010101010101010101646261736558200202020202020202020202020202020202020202020202020202020202020202686d616e69666573745820cf8a96e087621b97a95bff66994b6e31d29275b19b5e2ef390f101f88f923568666368616e67655820f5b97ff1964903ab971cb4d49cb47bbc744e4a1274f9dcab64ab3858b2001f806773756d6d6172796a6164642072756e2e7368",
        "blake3:f8e41a92912a5633a1f85b47e9bf1ec9cf58f34f47b612e675be62163a60bb04",
    ),
    (
        "application/vnd.bracken.weave.resolution+cbor",
        "a561760164706174686b7372632f6d61696e2e72736462617365825820de8f3fcf327eb911b2c368e011e0ffdee22fed456b4f4508337018ba46a0639d1981a465736964657382f6825820cd9488de804d1cf4fdbc6743c1f56364846c5ebb27c041552a946d48ec456ecd1981a466726573756c748258202dac974da83d90d57fbed9795c3d8566af7880b61b95f828278a72c97d13d0301981ed",
        "blake3:ce767f2d40438730166814af3dfe20f685f804a490be7d21d9deced142a53f14",
    ),
];

#[test]
fn golden_vectors() {
    let actual: Vec<(&str, String, String)> = vectors()
        .into_iter()
        .map(|(media, bytes)| (media, hex::encode(&bytes), ContentHash::of(&bytes).to_string()))
        .collect();
    for (media, hex, hash) in &actual {
        println!("    (\"{media}\", \"{hex}\", \"{hash}\"),");
    }
    let golden: Vec<(&str, String, String)> =
        GOLDEN.iter().map(|(m, h, c)| (*m, (*h).to_owned(), (*c).to_owned())).collect();
    assert_eq!(actual, golden, "a Weave payload format changed: this is protocol class");
}

#[test]
fn every_record_round_trips() {
    fn round_trip<R: Record + PartialEq + std::fmt::Debug>(record: R) {
        let bytes = record.encode();
        let decoded = R::decode(&bytes).expect("a valid record decodes");
        assert_eq!(decoded, record);
        assert_eq!(decoded.encode(), bytes);
        assert_eq!(decoded.content_hash(), ContentHash::of(&bytes));
    }
    round_trip(manifest());
    round_trip(marker());
    round_trip(Marker { in_place: Some(id(9)), ..marker() });
    round_trip(line());
    round_trip(Line { owner_decision: None, ..line() });
    round_trip(with_executor());
    round_trip(decision());
    round_trip(Decision { source: None, ..decision() });
    round_trip(proposal());
    round_trip(resolution());
    round_trip(Resolution::new("a".to_owned(), None, [None, Some(file_value(b"x", Mode::File))], None));
    round_trip(Manifest::new(vec![]));
}

#[test]
fn artifact_names_and_media_types() {
    assert_eq!(manifest().artifact_name(), "manifest");
    assert_eq!(marker().artifact_name(), "weave");
    assert_eq!(line().artifact_name(), "main");
    assert_eq!(decision().artifact_name(), "decision");
    assert_eq!(proposal().artifact_name(), "proposal");
    assert_eq!(resolution().artifact_name(), "resolution");
    for (media, _) in vectors() {
        assert!(media.starts_with(format::MEDIA_TYPE_PREFIX), "{media}");
    }
    assert_eq!(format::GOAL_TAG, "weave-goal");
}

/// Re-encodes a record's CBOR map with `edit` applied to its entries.
fn edited<R: Record>(record: &R, edit: impl FnOnce(&mut Vec<(Value, Value)>)) -> Vec<u8> {
    let mut value: Value = ciborium::from_reader(record.encode().as_slice()).expect("a record is CBOR");
    let Value::Map(entries) = &mut value else { panic!("a record is a map") };
    edit(entries);
    let mut bytes = Vec::new();
    ciborium::into_writer(&value, &mut bytes).expect("writing to a Vec cannot fail");
    bytes
}

#[test]
fn non_canonical_payloads_are_refused() {
    let bytes = marker().encode();

    let mut trailing = bytes.clone();
    trailing.push(0x00);
    assert_eq!(Marker::decode(&trailing), Err(FormatError::NotCanonical));

    // Keys out of the listed order.
    let swapped = edited(&marker(), |entries| entries.swap(0, 1));
    assert_eq!(Marker::decode(&swapped), Err(FormatError::NotCanonical));

    // An unknown field.
    let extra = edited(&marker(), |entries| entries.push((Value::Text("extra".into()), Value::Integer(1.into()))));
    assert_eq!(Marker::decode(&extra), Err(FormatError::NotCanonical));

    // A missing nullable field.
    let missing = edited(&marker(), |entries| {
        entries.pop();
    });
    assert_eq!(Marker::decode(&missing), Err(FormatError::NotCanonical));

    // An integer not in its shortest form: "v" as 0x18 0x01 instead of 0x01.
    let position = bytes.windows(3).position(|w| w == [0x61, b'v', 0x01]).expect("the payload starts with v: 1");
    let mut long_int = bytes.clone();
    long_int.splice(position + 2..position + 3, [0x18, 0x01]);
    assert_eq!(Marker::decode(&long_int), Err(FormatError::NotCanonical));

    // Check names out of order in the policy's map.
    let mut line_value: Value = ciborium::from_reader(line().encode().as_slice()).expect("a record is CBOR");
    let Value::Map(fields) = &mut line_value else { panic!("a record is a map") };
    let (_, checks) = fields.iter_mut().find(|(k, _)| k.as_text() == Some("checks")).expect("checks is a field");
    let Value::Map(checks) = checks else { panic!("checks is a map") };
    checks.reverse();
    let mut reordered = Vec::new();
    ciborium::into_writer(&line_value, &mut reordered).expect("writing to a Vec cannot fail");
    assert_eq!(Line::decode(&reordered), Err(FormatError::NotCanonical));

    assert!(matches!(Marker::decode(b"\xff"), Err(FormatError::Malformed(_))));
    assert!(matches!(Manifest::decode(&to_cbor(&"not a map")), Err(FormatError::Malformed(_))));
}

#[test]
fn only_version_1_is_read() {
    let v2 = Marker { v: 2, ..marker() };
    assert_eq!(Marker::decode(&v2.encode()), Err(FormatError::Version(2)));
    assert_eq!(Marker::decode(&Marker { v: 0, ..marker() }.encode()), Err(FormatError::Version(0)));
}

#[test]
fn unknown_modes_are_refused() {
    let bytes = to_cbor(&(ContentHash::of(b"x"), 0o100664u32));
    assert!(from_cbor::<FileValue>(&bytes).is_err());
    for mode in [Mode::File, Mode::Executable, Mode::Symlink] {
        assert_eq!(Mode::from_bits(mode.bits()), Some(mode));
    }
}

#[test]
fn manifest_rules() {
    let unsorted = Manifest { v: 1, entries: vec![entry("b", b"", Mode::File), entry("a", b"", Mode::File)] };
    assert_eq!(Manifest::decode(&unsorted.encode()), Err(FormatError::EntryOrder("a".to_owned())));

    let duplicate = Manifest { v: 1, entries: vec![entry("a", b"1", Mode::File), entry("a", b"2", Mode::File)] };
    assert_eq!(Manifest::decode(&duplicate.encode()), Err(FormatError::EntryOrder("a".to_owned())));

    let bad_path = Manifest::new(vec![entry(".git/config", b"", Mode::File)]);
    assert!(matches!(Manifest::decode(&bad_path.encode()), Err(FormatError::Path { .. })));

    for paths in [["README", "readme"], ["caf\u{e9}", "cafe\u{301}"], ["a", "a/b"], ["h", "H/x"]] {
        let colliding = Manifest::new(paths.iter().map(|p| entry(p, b"", Mode::File)).collect());
        assert!(matches!(Manifest::decode(&colliding.encode()), Err(FormatError::Collision(_))), "{paths:?}");
    }

    // Sorted by path bytes, not by folded or display order.
    let sorted =
        Manifest::new(vec![entry("b", b"", Mode::File), entry("C/x", b"", Mode::File), entry("a", b"", Mode::File)]);
    let paths: Vec<&str> = sorted.entries.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(paths, ["C/x", "a", "b"]);
    assert_eq!(Manifest::decode(&sorted.encode()), Ok(sorted.clone()));
    assert_eq!(sorted.get("a").map(|e| e.path.as_str()), Some("a"));
    assert_eq!(sorted.get("c"), None);
}

#[test]
fn line_rules() {
    let blank_name = Line { name: " \u{3000}".to_owned(), ..line() };
    assert_eq!(Line::decode(&blank_name.encode()), Err(FormatError::Blank("the line's name")));

    let mut blank_check = line();
    blank_check.checks.insert("\t".to_owned(), vec!["true".to_owned()]);
    assert_eq!(Line::decode(&blank_check.encode()), Err(FormatError::Blank("a check's name")));

    let mut empty_argv = line();
    empty_argv.checks.insert("lint".to_owned(), vec![]);
    assert_eq!(Line::decode(&empty_argv.encode()), Err(FormatError::EmptyArgv("lint".to_owned())));

    let bad_protected =
        Line { guardrails: Guardrails { protected: vec!["/abs".to_owned()], ..line().guardrails }, ..line() };
    assert!(matches!(Line::decode(&bad_protected.encode()), Err(FormatError::Path { .. })));

    let blank_owner = Line { owner_decision: Some(" ".to_owned()), ..line() };
    assert_eq!(Line::decode(&blank_owner.encode()), Err(FormatError::Blank("owner_decision")));

    // An executor names one of the line's checks, and a relative path inside the runner directory.
    let mut stray = with_executor();
    let e = stray.executors.remove("test").expect("the executor");
    stray.executors.insert("lint".to_owned(), e.clone());
    assert_eq!(Line::decode(&stray.encode()), Err(FormatError::ExecutorWithoutCheck("lint".to_owned())));
    for script in ["/usr/bin/true", "../scripts/test-table", "scripts/../test-table", ""] {
        let mut outside = with_executor();
        outside.executors.insert("test".to_owned(), Executor { script: script.to_owned(), ..e.clone() });
        assert!(matches!(Line::decode(&outside.encode()), Err(FormatError::Path { .. })), "{script:?}");
    }
}

fn executor(kind: ExecutorKind, script: &str, hash: &[u8]) -> Executor {
    Executor { kind, script: script.to_owned(), script_hash: ContentHash::of(hash) }
}

/// [`line`] with its test check run in a box.
fn with_executor() -> Line {
    let mut l = line();
    l.executors.insert("test".to_owned(), executor(ExecutorKind::Box, "scripts/test-table", b"the table"));
    l
}

// Plan cc496dcee0e3, section 2: a line without executors encodes as before the field existed, so every
// recorded policy and the weave-v1 fixtures stay valid, and a line with one encodes the field, so a
// build that doesn't know it refuses the line rather than run its box check in the sandbox.
#[test]
fn executors_are_left_out_when_there_are_none() {
    let plain = line().encode();
    let value: Value = ciborium::from_reader(plain.as_slice()).expect("CBOR");
    let keys: Vec<String> =
        value.as_map().expect("a map").iter().map(|(k, _)| k.as_text().expect("text keys").to_owned()).collect();
    assert!(!keys.iter().any(|k| k == "executors"), "{keys:?}");

    let value: Value = ciborium::from_reader(with_executor().encode().as_slice()).expect("CBOR");
    assert!(value.as_map().expect("a map").iter().any(|(k, _)| k.as_text() == Some("executors")));
}

// An executor is part of what a result is for: its kind, script and pinned hash change the method.
#[test]
fn a_checks_definition_hash_changes_with_its_executor() {
    let argv = vec!["--weave-box".to_owned()];
    let plain = CheckDefinition { name: "table", argv: &argv, executor: None };
    let boxed = executor(ExecutorKind::Box, "scripts/test-table", b"one");
    let on = |e: &Executor| CheckDefinition { name: "table", argv: &argv, executor: Some(e) }.hash();
    assert_ne!(plain.hash(), on(&boxed), "the same argv run in a box is another check");
    assert_ne!(on(&boxed), on(&executor(ExecutorKind::Mac, "scripts/test-table", b"one")), "kind");
    assert_ne!(on(&boxed), on(&executor(ExecutorKind::Box, "scripts/other", b"one")), "script");
    assert_ne!(on(&boxed), on(&executor(ExecutorKind::Box, "scripts/test-table", b"two")), "pinned hash");
    assert_eq!(on(&boxed), on(&boxed.clone()));
    // Without one, the hash is the old one: [name, argv].
    assert_eq!(plain.hash(), ContentHash::of(&to_cbor(&("table", &argv))));
    // The line gives each check its own definition.
    let l = with_executor();
    assert_eq!(l.definition("test").and_then(|d| d.executor), l.executors.get("test"));
    assert_eq!(l.definition("fmt").and_then(|d| d.executor), None);
    assert_ne!(l.check_method("test"), line().check_method("test"));
    assert_eq!(l.check_method("fmt"), line().check_method("fmt"));
}

/// Finding 1873f4834edd: a decision may refer to at most MAX_REFERENCES contributions in all.
#[test]
fn a_decision_refers_to_at_most_max_references() {
    let d = decision();
    let others = 1 + d.proposals.len() + d.answers.len() + d.resolutions.len() + d.goals.len();
    let parents = |n: usize| -> Vec<ContributionId> {
        (0..n)
            .map(|i| {
                let mut bytes = [0; 32];
                bytes[..8].copy_from_slice(&(i as u64).to_be_bytes());
                ContributionId(bytes)
            })
            .collect()
    };
    let at_limit = Decision { parents: parents(MAX_REFERENCES - others), ..decision() };
    assert_eq!(Decision::decode(&at_limit.encode()), Ok(at_limit));
    let over = Decision { parents: parents(MAX_REFERENCES - others + 1), ..decision() };
    assert_eq!(Decision::decode(&over.encode()), Err(FormatError::TooManyReferences(MAX_REFERENCES + 1)));
}

#[test]
fn decision_rules() {
    for parents in [vec![id(3), id(2)], vec![id(2), id(2)]] {
        let d = Decision { parents, ..decision() };
        assert_eq!(Decision::decode(&d.encode()), Err(FormatError::Order("a decision's parents")));
    }

    let mut blank_reason = decision();
    blank_reason.answers[0].reason = "\n".to_owned();
    assert_eq!(Decision::decode(&blank_reason.encode()), Err(FormatError::Blank("an answer's reason")));

    let mut blank_check = decision();
    blank_check.breakage[0].check = String::new();
    assert_eq!(Decision::decode(&blank_check.encode()), Err(FormatError::Blank("a breakage entry's check")));

    let mut bad_path = decision();
    bad_path.breakage[0].paths = vec!["a/../b".to_owned()];
    assert!(matches!(Decision::decode(&bad_path.encode()), Err(FormatError::Path { .. })));

    // Dispositions are the four words of the spec.
    let words: Vec<String> =
        [Disposition::Fixed, Disposition::FellBack, Disposition::AcceptedBreakage, Disposition::Overridden]
            .iter()
            .map(|d| serde_json::to_string(d).expect("a disposition serializes"))
            .collect();
    assert_eq!(words, ["\"fixed\"", "\"fell_back\"", "\"accepted_breakage\"", "\"overridden\""]);
}

#[test]
fn resolution_rules() {
    let ours = Some(file_value(b"ours", Mode::File));
    let theirs = Some(file_value(b"theirs", Mode::File));
    let mut sides = [ours, theirs];
    sides.sort();
    sides.reverse();
    let unordered = Resolution { v: 1, path: "a".to_owned(), base: None, sides, result: None };
    assert_eq!(Resolution::decode(&unordered.encode()), Err(FormatError::Order("a resolution's sides")));

    let equal = Resolution { v: 1, path: "a".to_owned(), base: None, sides: [ours, ours], result: None };
    assert_eq!(Resolution::decode(&equal.encode()), Err(FormatError::Order("a resolution's sides")));

    // A deleted side sorts first, and the key compares modes too.
    let r = Resolution::new("a".to_owned(), None, [ours, None], ours);
    assert_eq!(r.sides, [None, ours]);
    let exec = Some(file_value(b"ours", Mode::Executable));
    assert_eq!(
        Resolution::decode(&Resolution::new("a".to_owned(), None, [ours, exec], ours).encode()).map(|r| r.sides),
        Ok([ours, exec])
    );

    let bad_path = Resolution::new(".bracken/x".to_owned(), None, [ours, theirs], None);
    assert!(matches!(Resolution::decode(&bad_path.encode()), Err(FormatError::Path { .. })));
}

#[test]
fn a_change_lists_the_paths_that_differ() {
    let base =
        Manifest::new(vec![entry("a", b"1", Mode::File), entry("b", b"1", Mode::File), entry("c", b"1", Mode::File)]);
    let new = Manifest::new(vec![
        entry("a", b"2", Mode::File),
        entry("b", b"1", Mode::Executable),
        entry("d", b"1", Mode::File),
    ]);
    let change = Change::between(&base, &new);
    let rows: Vec<(&str, Option<FileValue>, Option<FileValue>)> =
        change.entries().iter().map(|e| (e.path.as_str(), e.before, e.after)).collect();
    assert_eq!(
        rows,
        [
            ("a", Some(file_value(b"1", Mode::File)), Some(file_value(b"2", Mode::File))),
            ("b", Some(file_value(b"1", Mode::File)), Some(file_value(b"1", Mode::Executable))),
            ("c", Some(file_value(b"1", Mode::File)), None),
            ("d", None, Some(file_value(b"1", Mode::File))),
        ]
    );
    assert!(Change::between(&base, &base).entries().is_empty());
}

#[test]
fn a_rebase_keeps_the_change_only_if_every_file_change_is_the_same() {
    let base1 = Manifest::new(vec![entry("a", b"1", Mode::File), entry("b", b"1", Mode::File)]);
    let new1 = Manifest::new(vec![entry("a", b"2", Mode::File), entry("b", b"1", Mode::File)]);
    // The line moved: c was added. The rebased proposal changes a the same way.
    let base2 =
        Manifest::new(vec![entry("a", b"1", Mode::File), entry("b", b"1", Mode::File), entry("c", b"1", Mode::File)]);
    let new2 =
        Manifest::new(vec![entry("a", b"2", Mode::File), entry("b", b"1", Mode::File), entry("c", b"1", Mode::File)]);
    assert_eq!(Change::between(&base1, &new1).hash(), Change::between(&base2, &new2).hash());
    // The line moved under a itself: the change's "before" differs, so the key does.
    let base3 = Manifest::new(vec![entry("a", b"3", Mode::File), entry("b", b"1", Mode::File)]);
    assert_ne!(Change::between(&base1, &new1).hash(), Change::between(&base3, &new1).hash());
}

#[test]
fn a_checks_definition_hash_changes_with_its_argv() {
    let argv = |words: &[&str]| words.iter().map(|w| w.to_string()).collect::<Vec<_>>();
    let test = argv(&["cargo", "test", "--workspace"]);
    let fmt = argv(&["cargo", "fmt", "--check"]);
    let a = CheckDefinition { name: "test", argv: &test, executor: None };
    let b = CheckDefinition { name: "test", argv: &fmt, executor: None };
    let c = CheckDefinition { name: "fmt", argv: &fmt, executor: None };
    assert_ne!(a.hash(), b.hash(), "same name, different argv");
    assert_ne!(b.hash(), c.hash(), "same argv, different name");
    assert_eq!(a.hash(), CheckDefinition { name: "test", argv: &test.clone(), executor: None }.hash());
    // Splitting words differently is a different definition.
    let joined = argv(&["cargo", "test --workspace"]);
    assert_ne!(a.hash(), CheckDefinition { name: "test", argv: &joined, executor: None }.hash());

    assert_eq!(a.method(), format!("test {}", a.hash()));
    assert_eq!(parse_method(&a.method()), Some(("test", a.hash())));
    assert_eq!(parse_method("test"), None);
    assert_eq!(parse_method(&format!("test {}", a.hash().to_hex())), None, "a bare hex hash is refused");
    assert_eq!(parse_method(&format!(" {}", a.hash())), None);
    assert_eq!(parse_method(&format!("unit tests {}", a.hash())), Some(("unit tests", a.hash())));

    // A policy gives the method its results must carry.
    assert_eq!(line().check_method("test"), Some(a.method()));
    assert_eq!(line().check_method("clippy"), None);
    let mut other = line();
    other.checks.insert("test".to_owned(), fmt.clone());
    assert_ne!(other.check_method("test"), line().check_method("test"));
}

#[test]
fn a_manifest_with_canonically_equivalent_paths_is_invalid() {
    // Finding cc6570efb144's counterexample: a symlink and a directory that APFS
    // treats as one name.
    let colliding = Manifest::new(vec![
        entry("x\u{1f0}\u{323}", b"sub", Mode::Symlink),
        entry("xJ\u{323}\u{30c}/f", b"", Mode::File),
    ]);
    assert!(matches!(Manifest::decode(&colliding.encode()), Err(FormatError::Collision(_))));
}

#[test]
fn a_manifest_with_an_overlong_path_is_refused_before_folding() {
    // Finding d65749f2b45d's counterexample: an 80 KB payload holding one path of
    // 40,000 components.
    let deep = Manifest::new(vec![entry(&vec!["a"; 40_000].join("/"), b"", Mode::File)]);
    assert!(matches!(
        Manifest::decode(&deep.encode()),
        Err(FormatError::Path { error: bracken_tree::PathError::TooLong(_), .. })
    ));
}

// Guards G1 to G5 (docs/design/weave-freeze.md, section 2.4; task a61fb528dc18). Every value below is
// what today's code gives. A failure means a frozen format changed: that is protocol class, as the module
// comment says. Don't update a value to make it pass.

/// [`line`] with its test check run on the validator's Mac.
fn with_mac_executor() -> Line {
    let mut l = line();
    l.executors.insert("test".to_owned(), executor(ExecutorKind::Mac, "scripts/test-table", b"the table"));
    l
}

/// G1: (what, payload hex, content hash) for the line and marker shapes `GOLDEN` leaves out.
const GOLDEN_SHAPES: &[(&str, &str, &str)] = &[
    (
        "line with a box executor",
        "a9617601646e616d65646d61696e6b696e7465677261746f7273815820a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a16a76616c696461746f7273815820a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a26c676f616c5f73657474657273815820a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a366636865636b73a263666d748365636172676f63666d74672d2d636865636b64746573748365636172676f64746573746b2d2d776f726b7370616365696578656375746f7273a16474657374a3646b696e6463626f786673637269707472736372697074732f746573742d7461626c656b7363726970745f686173685820047f88e9dc91aec8d0181b288a3ce5e27386504a5585d05cfbb017397389ce876a67756172647261696c73a3696d61785f6b6e6f776e03676d61785f616765056970726f746563746564816f6372617465732f6d616e79726f6f746e6f776e65725f6465636973696f6e6c343662393165303165646332",
        "blake3:2784058528241c507b00fe2277b4a755685e2aa0adb7853126431184aa350252",
    ),
    (
        "line with a mac executor",
        "a9617601646e616d65646d61696e6b696e7465677261746f7273815820a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a16a76616c696461746f7273815820a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a26c676f616c5f73657474657273815820a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a366636865636b73a263666d748365636172676f63666d74672d2d636865636b64746573748365636172676f64746573746b2d2d776f726b7370616365696578656375746f7273a16474657374a3646b696e64636d61636673637269707472736372697074732f746573742d7461626c656b7363726970745f686173685820047f88e9dc91aec8d0181b288a3ce5e27386504a5585d05cfbb017397389ce876a67756172647261696c73a3696d61785f6b6e6f776e03676d61785f616765056970726f746563746564816f6372617465732f6d616e79726f6f746e6f776e65725f6465636973696f6e6c343662393165303165646332",
        "blake3:4c608d2571c0580d841ab81fb653e69651c0212237264a3c71275310071430c2",
    ),
    (
        "marker with in_place set",
        "a361760167707572706f736567646f67666f6f6468696e5f706c61636558200909090909090909090909090909090909090909090909090909090909090909",
        "blake3:61faee9a6c5291fc8f3c9be38bb0e5aeca1ba223542ffd9ab7eefb24ff063951",
    ),
];

#[test]
fn golden_vectors_for_executors_and_in_place() {
    let actual: Vec<(&str, String, String)> = [
        ("line with a box executor", with_executor().encode()),
        ("line with a mac executor", with_mac_executor().encode()),
        ("marker with in_place set", Marker { in_place: Some(id(9)), ..marker() }.encode()),
    ]
    .into_iter()
    .map(|(what, bytes)| (what, hex::encode(&bytes), ContentHash::of(&bytes).to_string()))
    .collect();
    for (what, hex, hash) in &actual {
        println!("    (\"{what}\", \"{hex}\", \"{hash}\"),");
    }
    let golden: Vec<(&str, String, String)> =
        GOLDEN_SHAPES.iter().map(|(w, h, c)| (*w, (*h).to_owned(), (*c).to_owned())).collect();
    assert_eq!(actual, golden, "a Weave payload format changed: this is protocol class");
}

/// G2: a check's definition hash and method, without an executor and with each kind.
#[test]
fn golden_definition_hashes_and_methods() {
    let argv: Vec<String> = ["cargo", "test", "--workspace"].iter().map(|w| w.to_string()).collect();
    let boxed = executor(ExecutorKind::Box, "scripts/test-table", b"the table");
    let mac = executor(ExecutorKind::Mac, "scripts/test-table", b"the table");
    let actual: Vec<(&str, String)> = [("argv only", None), ("box", Some(&boxed)), ("mac", Some(&mac))]
        .into_iter()
        .map(|(what, executor)| (what, CheckDefinition { name: "test", argv: &argv, executor }.method()))
        .collect();
    for (what, method) in &actual {
        println!("    (\"{what}\", \"{method}\"),");
    }
    let golden = [
        ("argv only", "test blake3:1f32a00f8d1d7b2e1f37df68163c3459e8a7c9dc710a89562c1fc3f4412f1ea6"),
        ("box", "test blake3:b8147a7a3da17c52de31fd27115304b066ade1a731c603a2590584418325d7b3"),
        ("mac", "test blake3:24e0f4396af62389b792b283f3680cdb0b034b963eda6f8f1af7c226ee204f9d"),
    ];
    assert_eq!(actual, golden.map(|(w, m)| (w, m.to_owned())), "a definition hash changed: this is protocol class");
    // The policies that hold these checks give the same methods.
    assert_eq!(line().check_method("test").as_deref(), Some(actual[0].1.as_str()));
    assert_eq!(with_executor().check_method("test").as_deref(), Some(actual[1].1.as_str()));
    assert_eq!(with_mac_executor().check_method("test").as_deref(), Some(actual[2].1.as_str()));
    for (_, method) in &actual {
        let (name, hash) = parse_method(method).expect("a method parses");
        assert_eq!((name, hash.to_string()), ("test", method["test ".len()..].to_owned()));
    }
}

/// G2: a change list with an added, a deleted, a changed and a mode-only row, which is a proposal's
/// `change`.
#[test]
fn golden_change_hash() {
    let base =
        Manifest::new(vec![entry("a", b"1", Mode::File), entry("b", b"1", Mode::File), entry("c", b"1", Mode::File)]);
    let new = Manifest::new(vec![
        entry("a", b"2", Mode::File),
        entry("b", b"1", Mode::Executable),
        entry("d", b"1", Mode::File),
    ]);
    let change = Change::between(&base, &new);
    let kinds: Vec<(&str, bool, bool)> =
        change.entries().iter().map(|e| (e.path.as_str(), e.before.is_some(), e.after.is_some())).collect();
    assert_eq!(kinds, [("a", true, true), ("b", true, true), ("c", true, false), ("d", false, true)]);
    assert_eq!(change.hash().to_string(), "blake3:7efec8382149409dfcfbbb82b4c9d60e96ea9eab0b17ecc42e3dabead4d4c44a");
}

/// G3: the limits, by value. paths.rs and the rule tests check behaviour relative to the constants, so a
/// changed constant would pass them.
#[test]
fn limits_are_pinned_by_value() {
    assert_eq!(MAX_REFERENCES, 1024);
    assert_eq!(bracken_tree::path::MAX_PATH_BYTES, 4096);
    assert_eq!(bracken_tree::path::MAX_COMPONENT_BYTES, 255);
}

/// G4: a result's details, as text, for each way a check ends, a withheld tail, and a check run outside
/// the sandbox; and `parse_details` reads the same manifest and attempt back from each.
#[test]
fn golden_details() {
    use bracken_tree::check::{Exit, RecordedTail};
    use bracken_tree::results::{details, details_with_run, parse_details};
    use bracken_tree::secrets::SecretRule;

    let manifest = manifest().content_hash();
    let argv: Vec<String> = ["cargo", "test", "--workspace"].iter().map(|w| w.to_string()).collect();
    let text = RecordedTail::Text("test result: ok\n".to_owned());
    let withheld = RecordedTail::Withheld {
        size: 1234,
        // A PEM private-key header, built at run time: the literal would make import refuse this file.
        hash: ContentHash::of(format!("-----BEGIN {}-----", "PRIVATE KEY").as_bytes()),
        rule: SecretRule::PemPrivateKey,
    };
    let run: Vec<String> = [
        "commit 89aa6f1a00000000000000000000000000000000",
        "tree 4b825dc642cb6eb9a060e54bf8d69288fbee4904",
        "bundle sha256:0000000000000000000000000000000000000000000000000000000000000000",
        "box bx-1",
    ]
    .iter()
    .map(|l| l.to_string())
    .collect();
    let actual: Vec<(&str, u64, String)> = vec![
        ("exit code", 1, details(&manifest, 1, &argv, &Exit::Code(0), &text)),
        ("signal", 2, details(&manifest, 2, &argv, &Exit::Signal(9), &text)),
        ("timed out", 3, details(&manifest, 3, &argv, &Exit::TimedOut, &text)),
        ("left group", 4, details(&manifest, 4, &argv, &Exit::LeftGroup(vec![4242, 4243]), &text)),
        ("held output", 5, details(&manifest, 5, &argv, &Exit::HeldOutput, &text)),
        ("withheld tail", 6, details(&manifest, 6, &argv, &Exit::Code(1), &withheld)),
        ("with run", 7, details_with_run(&manifest, 7, &argv, &Exit::Code(0), &run, &text)),
    ];
    for (what, _, details) in &actual {
        println!("    (\"{what}\", {details:?}),");
    }
    let golden: [(&str, &str); 7] = [
        (
            "exit code",
            "blake3:cf8a96e087621b97a95bff66994b6e31d29275b19b5e2ef390f101f88f923568\nattempt 1\nargv [\"cargo\", \"test\", \"--workspace\"]\nexit 0\n\ntest result: ok\n",
        ),
        (
            "signal",
            "blake3:cf8a96e087621b97a95bff66994b6e31d29275b19b5e2ef390f101f88f923568\nattempt 2\nargv [\"cargo\", \"test\", \"--workspace\"]\nkilled by signal 9\n\ntest result: ok\n",
        ),
        (
            "timed out",
            "blake3:cf8a96e087621b97a95bff66994b6e31d29275b19b5e2ef390f101f88f923568\nattempt 3\nargv [\"cargo\", \"test\", \"--workspace\"]\ntimed out\n\ntest result: ok\n",
        ),
        (
            "left group",
            "blake3:cf8a96e087621b97a95bff66994b6e31d29275b19b5e2ef390f101f88f923568\nattempt 4\nargv [\"cargo\", \"test\", \"--workspace\"]\nfailed: 2 of its processes left the process group, and were killed ([4242, 4243])\n\ntest result: ok\n",
        ),
        (
            "held output",
            "blake3:cf8a96e087621b97a95bff66994b6e31d29275b19b5e2ef390f101f88f923568\nattempt 5\nargv [\"cargo\", \"test\", \"--workspace\"]\nfailed: a process it started still held its output open after it ended\n\ntest result: ok\n",
        ),
        // "as it matched it holds ...": a known wording slip, kept on purpose (question 92508112ed80).
        (
            "withheld tail",
            "blake3:cf8a96e087621b97a95bff66994b6e31d29275b19b5e2ef390f101f88f923568\nattempt 6\nargv [\"cargo\", \"test\", \"--workspace\"]\nexit 1\n\noutput withheld: 1234 bytes, blake3:bccd14c333101b93c34b293ee47247d701e9baf78b20ccd7f01c2fafc598985e, as it matched it holds a PEM private-key header",
        ),
        (
            "with run",
            "blake3:cf8a96e087621b97a95bff66994b6e31d29275b19b5e2ef390f101f88f923568\nattempt 7\nargv [\"cargo\", \"test\", \"--workspace\"]\nexit 0\ncommit 89aa6f1a00000000000000000000000000000000\ntree 4b825dc642cb6eb9a060e54bf8d69288fbee4904\nbundle sha256:0000000000000000000000000000000000000000000000000000000000000000\nbox bx-1\n\ntest result: ok\n",
        ),
    ];
    let got: Vec<(&str, &str)> = actual.iter().map(|(w, _, d)| (*w, d.as_str())).collect();
    assert_eq!(got, golden, "a result's details changed: this is protocol class");
    for (what, attempt, details) in &actual {
        assert_eq!(parse_details(details), Some((manifest, *attempt)), "{what}");
    }
}

/// G5: a content hash's text form is `blake3:` and 64 lowercase hex digits; nothing else parses.
#[test]
fn content_hash_text_form() {
    use bracken_tree::hash::ParseHashError;

    let hex = "2d1ebfa706ba230165250f744796a92accba5e1b6fa357983b65319da33f8e93";
    let parsed: ContentHash = format!("blake3:{hex}").parse().expect("the labelled form parses");
    assert_eq!(parsed, ContentHash::of(FILE_BYTES));
    assert_eq!(parsed.to_string(), format!("blake3:{hex}"));
    assert_eq!(hex.parse::<ContentHash>(), Err(ParseHashError::Unlabelled), "bare");
    assert_eq!(hex.to_ascii_uppercase().parse::<ContentHash>(), Err(ParseHashError::Unlabelled), "bare uppercase");
    let upper = format!("blake3:{}", hex.to_ascii_uppercase());
    assert_eq!(upper.parse::<ContentHash>(), Err(ParseHashError::Malformed), "uppercase hex");
    assert_eq!(format!("BLAKE3:{hex}").parse::<ContentHash>(), Err(ParseHashError::Malformed), "uppercase label");
    assert_eq!(format!("blake3:{}", &hex[..63]).parse::<ContentHash>(), Err(ParseHashError::Malformed), "short");
    assert_eq!("blake3:".parse::<ContentHash>(), Err(ParseHashError::Malformed), "empty");
    assert_eq!(format!("blake3:{hex}0").parse::<ContentHash>(), Err(ParseHashError::Malformed), "long");
}

/// The Unicode tables folding depends on are pinned: unicode-normalization's
/// version, and a digest of `fold` over every scalar value, which also covers
/// icu_casemap's case-folding data and the default-ignorable list. A dependency
/// bump that changes a table changes which paths collide, so it fails here and is
/// reviewed as protocol class.
#[test]
fn unicode_tables_are_pinned() {
    assert_eq!(unicode_normalization::UNICODE_VERSION, (18, 0, 0));
    let mut hasher = blake3::Hasher::new();
    let mut buf = [0u8; 4];
    for c in (0..=0x10FFFFu32).filter_map(char::from_u32) {
        hasher.update(c.encode_utf8(&mut buf).as_bytes());
        hasher.update(bracken_tree::path::fold(c.encode_utf8(&mut buf)).as_bytes());
        hasher.update(&[0xff]);
    }
    assert_eq!(
        ContentHash(*hasher.finalize().as_bytes()).to_string(),
        "blake3:a2dfd498ba458a2f61f89b2bf8254ea1b9cf7d078a2bcac64aba0826cd4e258c"
    );
}

/// The line merge's results are part of what a merged tree means across replicas and versions
/// (task 7cbd0a0ce7d0, answer c8a511445626): these triples pin it, imara-diff's histogram diff included.
/// A failure here means a change to the algorithm or the crate, which is reviewed as protocol class.
/// Each is (base, ours, theirs, the content hash of the merge or "conflict").
#[test]
fn line_merge_golden_vectors() {
    let vectors: [(&str, &str, &str, &str); 8] = [
        (
            "a\nb\nc\nd\ne\n",
            "A\nb\nc\nd\ne\n",
            "a\nb\nc\nd\nE\n",
            "blake3:a697e21e31b7ce97c673b77be5edba07564d74b47512078da31f3d016eca72c2",
        ),
        ("a\nb\nc\nd\ne\n", "A\nb\nc\nd\ne\n", "a\nB\nc\nd\ne\n", "conflict"),
        (
            "a\nb\nc\nd\ne\n",
            "a\nb\nN\nc\nd\ne\n",
            "a\nb\nN\nc\nd\ne\n",
            "blake3:447360351726de625e35b0841a61a0612844ed88514abd77f7113b09a60caf9d",
        ),
        (
            "a\nb\nc\nd",
            "a\nb\nc\nd\n",
            "A\nb\nc\nd",
            "blake3:4554a460ebbdba85ac6879a08afc3507f9072b38589a143a00f05365719757a7",
        ),
        (
            "a\r\nb\r\nc\r\nd\r\n",
            "A\r\nb\r\nc\r\nd\r\n",
            "a\r\nb\r\nc\r\nD\r\n",
            "blake3:095a36f014816f2b99c86691cec0dd02acd3f3877c3a6e3de6cd8b0697e03120",
        ),
        (
            "fn f() {\n    a();\n}\n\nfn g() {\n    b();\n}\n",
            "fn f() {\n    a();\n    a2();\n}\n\nfn g() {\n    b();\n}\n",
            "fn f() {\n    a();\n}\n\nfn g() {\n    b();\n    b2();\n}\n",
            "blake3:f37bef4d6ea870740ee7c7127eeb61fd7eca70f960a5ad27d8a728cc712700e9",
        ),
        (
            "}\n\n    y();\n}\n    y();\n}\n    y();\n",
            "}\n\n    y();\n}\n}\n    y();\n",
            "}\n\n    y();\n    y();\n}\n    y();\n}\nw\n    y();\n",
            "blake3:645be9d60a08b1e5482e55fe7aad11d0d1e700d02d86838c22cc382961811ceb",
        ),
        ("", "x\n", "y\n", "conflict"),
    ];
    for (i, (base, ours, theirs, want)) in vectors.iter().enumerate() {
        let got = merge(base.as_bytes(), ours.as_bytes(), theirs.as_bytes())
            .map_or_else(|| "conflict".to_owned(), |b| ContentHash::of(&b).to_string());
        assert_eq!(&got, want, "vector {i}");
    }
}

/// The line merge's diff cap, 10,000 lines per side once common first and last lines are set aside
/// (finding 933f535c9476, answer 47a1c1088fd6), decides clean or not, so it is pinned: a rewrite of
/// exactly that many lines merges, to these bytes, and one line more is a conflict.
#[test]
fn line_merge_cap_vectors() {
    let file = |first: &str, tag: &str, n: usize| -> String {
        let middle: String = (0..n).map(|i| format!("{tag}{i}\n")).collect();
        format!("{first}\ngap\n{middle}tail\n")
    };
    for (n, want) in
        [(10_000, "blake3:8498c76b9a5b1c1900f92d292005694560b79b0823aa6802af457f1139d561b8"), (10_001, "conflict")]
    {
        let (base, ours, theirs) = (file("h", "b", n), file("h", "o", n), file("H", "b", n));
        let merged = merge(base.as_bytes(), ours.as_bytes(), theirs.as_bytes());
        if let Some(bytes) = &merged {
            assert_eq!(bytes, file("H", "o", n).as_bytes(), "both edits");
        }
        let got = merged.map_or_else(|| "conflict".to_owned(), |b| ContentHash::of(&b).to_string());
        assert_eq!(got, want, "{n} lines");
    }
    assert_eq!(bracken_tree::text_merge::MAX_DIFF_LINES, 10_000);
}

// The line merge against git merge-file (task 7cbd0a0ce7d0). Weave's line merge is its own rule, which
// agrees with git's in nearly all cases but not byte for byte (docs/design/weave.md, *Per-file merge*).

use std::process::Command;

use bracken_tree::text_merge::merge;

/// A small deterministic generator, so a failing triple can be found again from its seed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

const WORDS: [&str; 9] = ["a", "b", "c", "}", "", "fn x() {", "    y();", "z", "w"];

/// A side: `base` with a few random edits. A small alphabet repeats lines, which is where diffs differ.
fn edit(rng: &mut Rng, base: &[String]) -> Vec<String> {
    let mut lines = base.to_vec();
    for _ in 0..rng.below(4) {
        let at = rng.below(lines.len() as u64 + 1) as usize;
        let word = WORDS[rng.below(WORDS.len() as u64) as usize].to_owned();
        match rng.below(3) {
            0 => lines.insert(at, word),
            1 if at < lines.len() => {
                lines.remove(at);
            }
            _ if at < lines.len() => lines[at] = word,
            _ => {}
        }
    }
    lines
}

fn text(lines: &[String], final_newline: bool) -> String {
    let mut s = lines.join("\n");
    if final_newline && !lines.is_empty() {
        s.push('\n');
    }
    s
}

/// git merge-file's answer: the merged bytes, or None for a conflict. Its exit is the conflict count.
fn git_merge(dir: &std::path::Path, base: &str, ours: &str, theirs: &str) -> Option<Vec<u8>> {
    for (name, body) in [("base", base), ("ours", ours), ("theirs", theirs)] {
        std::fs::write(dir.join(name), body).expect("scratch write");
    }
    let out = Command::new("git")
        .args(["merge-file", "-p", "--diff-algorithm=histogram", "ours", "base", "theirs"])
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .expect("git runs");
    match out.status.code() {
        Some(0) => Some(out.stdout),
        Some(n) if (1..127).contains(&n) => None,
        other => panic!("git merge-file failed ({other:?}): {}", String::from_utf8_lossy(&out.stderr)),
    }
}

/// How often the line merge and git merge-file disagree, by kind, over `n` triples from `seed`: printed,
/// for measuring. `cargo test -p bracken-tree --test text_merge -- --ignored --nocapture tally`.
#[test]
#[ignore = "a measurement, not a check"]
fn tally() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let mut rng = Rng(7);
    let (mut agree, mut weave_only, mut git_only, mut differ, mut first) = (0, 0, 0, 0, Vec::new());
    for case in 0..20000 {
        let base: Vec<String> =
            (0..rng.below(12) + 1).map(|_| WORDS[rng.below(WORDS.len() as u64) as usize].to_owned()).collect();
        let (ours, theirs) = (edit(&mut rng, &base), edit(&mut rng, &base));
        let nl = [rng.below(4) != 0, rng.below(4) != 0, rng.below(4) != 0];
        let (b, x, y) = (text(&base, nl[0]), text(&ours, nl[1]), text(&theirs, nl[2]));
        let got = merge(b.as_bytes(), x.as_bytes(), y.as_bytes());
        let git = git_merge(dir.path(), &b, &x, &y);
        match (&got, &git) {
            _ if got == git => agree += 1,
            (Some(_), None) => weave_only += 1,
            (None, Some(_)) => git_only += 1,
            _ => differ += 1,
        }
        if got != git && first.len() < 3 {
            first.push(case);
        }
    }
    println!(
        "agree {agree}, clean only here {weave_only}, clean only in git {git_only}, both clean but different {differ}; first {first:?}"
    );
}

/// The cases of the oracle's seed where git and imara-diff's histogram diff align repeated lines
/// differently, so the two merges differ (question 49bd8e66c5f9, answer 55e0a67c68a2). Pinned in the
/// format guard: a change to imara-diff or to git that moves this list is a change to what Weave's line
/// merge gives, reviewed as protocol class. Don't edit it to make the test pass.
const KNOWN_DIFFERENT: [usize; 1] = [620];

/// git merge-file as an oracle: on 2,000 generated triples, both agree on whether a merge is clean,
/// and on its bytes when it is, but for [`KNOWN_DIFFERENT`].
#[test]
fn agrees_with_git_merge_file() {
    if Command::new("git").arg("--version").output().is_err() {
        eprintln!("skipped: no git");
        return;
    }
    let dir = tempfile::tempdir().expect("a scratch directory");
    let mut rng = Rng(0x7cbd_0a0c_e7d0_0001);
    let (mut clean, mut conflicted, mut different) = (0, 0, Vec::new());
    for case in 0..2000 {
        let base: Vec<String> =
            (0..rng.below(12) + 1).map(|_| WORDS[rng.below(WORDS.len() as u64) as usize].to_owned()).collect();
        let (ours, theirs) = (edit(&mut rng, &base), edit(&mut rng, &base));
        let nl = [rng.below(4) != 0, rng.below(4) != 0, rng.below(4) != 0];
        let (b, x, y) = (text(&base, nl[0]), text(&ours, nl[1]), text(&theirs, nl[2]));
        let got = merge(b.as_bytes(), x.as_bytes(), y.as_bytes());
        let git = git_merge(dir.path(), &b, &x, &y);
        if got != git {
            different.push(case);
            eprintln!("case {case}: base {b:?}, ours {x:?}, theirs {y:?}: here {got:?}, git {git:?}");
        }
        if git.is_some() {
            clean += 1;
        } else {
            conflicted += 1;
        }
    }
    assert!(clean > 200 && conflicted > 200, "the cases cover both: {clean} clean, {conflicted} conflicts");
    assert_eq!(different, KNOWN_DIFFERENT, "where the merges differ from git's");
}

/// The same inputs give the same bytes every time, and swapping the sides doesn't change whether a merge
/// is clean.
#[test]
fn deterministic_and_symmetric() {
    let mut rng = Rng(42);
    for _ in 0..500 {
        let base: Vec<String> = (0..rng.below(10) + 1).map(|_| format!("l{}", rng.below(5))).collect();
        let (x, y) = (text(&edit(&mut rng, &base), true), text(&edit(&mut rng, &base), true));
        let b = text(&base, true);
        let once = merge(b.as_bytes(), x.as_bytes(), y.as_bytes());
        assert_eq!(once, merge(b.as_bytes(), x.as_bytes(), y.as_bytes()));
        assert_eq!(once.is_some(), merge(b.as_bytes(), y.as_bytes(), x.as_bytes()).is_some(), "{b:?} {x:?} {y:?}");
    }
}
