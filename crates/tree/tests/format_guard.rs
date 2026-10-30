//! Format guard: Weave's payload formats are pinned like the protocol
//! (docs/design/weave.md, section 5). One record of every Weave media type is
//! checked against its exact bytes and content hash. If a test here fails, a
//! change altered a format that existing Weave Grounds hold: that is protocol
//! class, with an independent review, the manager's go and a note in
//! docs/releases.md. Don't update a golden vector to make it pass.

use std::collections::BTreeMap;

use bracken_tree::format::{
    self, DECISION_MEDIA_TYPE, FILE_MEDIA_TYPE, LINE_MEDIA_TYPE, MANIFEST_MEDIA_TYPE, MARKER_MEDIA_TYPE,
    MAX_REFERENCES, PROPOSAL_MEDIA_TYPE, RESOLUTION_MEDIA_TYPE, from_cbor, parse_method, to_cbor,
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
    let a = CheckDefinition { name: "test", argv: &test };
    let b = CheckDefinition { name: "test", argv: &fmt };
    let c = CheckDefinition { name: "fmt", argv: &fmt };
    assert_ne!(a.hash(), b.hash(), "same name, different argv");
    assert_ne!(b.hash(), c.hash(), "same argv, different name");
    assert_eq!(a.hash(), CheckDefinition { name: "test", argv: &test.clone() }.hash());
    // Splitting words differently is a different definition.
    let joined = argv(&["cargo", "test --workspace"]);
    assert_ne!(a.hash(), CheckDefinition { name: "test", argv: &joined }.hash());

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
