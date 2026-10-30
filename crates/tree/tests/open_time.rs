//! Open time on a 50 MB fixture Ground (docs/design/weave.md, section 4; build
//! step 5). Opening decodes the whole history, so this measures what every
//! `weave` read pays as the dogfood Ground grows: about 45 MB in four days is the
//! spec's estimate. The test fails only if the fixture is under 50 MB or the
//! views are wrong; its timings are printed, never asserted.
//!
//! It writes 55 MB and takes about ten seconds in a debug build, so it is
//! ignored in the test table. Run it, with timings worth reading, as:
//!
//! ```text
//! cargo test --release -p bracken-tree --test open_time -- --ignored --nocapture
//! ```

use std::collections::BTreeMap;
use std::time::Instant;

use bracken_tree::format::{Decision, FILE_MEDIA_TYPE, Guardrails, Line, Manifest, Marker, Record, VERSION};
use bracken_tree::ground::Index;
use bracken_tree::lines::Lines;
use bracken_tree::{ContentHash, Entry, Mode};
use manyroot::{Body, Contribution, ContributionId, Store};

/// About this repository's shape: 290 tracked files of 24 KiB, two of them
/// changed by each of 650 decisions.
const PATHS: usize = 290;
const FILE_BYTES: usize = 24 * 1024;
const DECISIONS: usize = 650;
const CHANGED: usize = 2;

/// Deterministic bytes for one version of one path.
fn bytes(path: usize, version: usize) -> Vec<u8> {
    let mut out = vec![0; FILE_BYTES];
    let mut hasher = blake3::Hasher::new();
    hasher.update(&(path as u64).to_le_bytes()).update(&(version as u64).to_le_bytes());
    hasher.finalize_xof().fill(&mut out);
    out
}

fn artifact<R: Record>(record: &R) -> Body {
    Body::Artifact {
        name: record.artifact_name().to_owned(),
        media_type: R::MEDIA_TYPE.to_owned(),
        content: record.encode(),
    }
}

#[test]
#[ignore = "writes a 55 MB fixture; run with --ignored, see the module doc"]
fn a_fifty_megabyte_ground_opens_and_its_views_hold() {
    let tmp = tempfile::tempdir().expect("a temporary directory");
    let dir = tmp.path().join("g");
    let mut s = Store::init(&dir, "fixture").expect("a Ground");
    let who = s.create_identity("weave-integrator").expect("an identity");
    let ground = s.info().ground;

    let mut batch = Vec::new();
    let mut head: Option<ContributionId> = None;
    let mut push = |batch: &mut Vec<Contribution>, body: Body| -> ContributionId {
        // A linear history, each write listing the one before, as a lone writer's would.
        let c = Contribution::create(ground, &who, head, batch.len() as u64, body);
        let id = c.id();
        batch.push(c);
        head = Some(id);
        id
    };
    push(&mut batch, artifact(&Marker { v: VERSION, purpose: "fixture".into(), in_place: None }));
    let line = Line {
        v: VERSION,
        name: "main".into(),
        integrators: vec![who.agent_id()],
        validators: vec![],
        goal_setters: vec![who.agent_id()],
        checks: BTreeMap::new(),
        guardrails: Guardrails { max_known: 0, max_age: 0, protected: vec![] },
        owner_decision: None,
    };
    let line = push(&mut batch, artifact(&line));

    let mut versions = vec![0usize; PATHS];
    let mut entries: Vec<Entry> = Vec::with_capacity(PATHS);
    for (p, v) in versions.iter().enumerate() {
        let content = bytes(p, *v);
        let path = format!("crates/fixture/src/module_{p:04}.rs");
        entries.push(Entry {
            path: path.clone(),
            mode: Mode::File,
            hash: ContentHash::of(&content),
            size: content.len() as u64,
        });
        push(&mut batch, Body::Artifact { name: path, media_type: FILE_MEDIA_TYPE.into(), content });
    }
    let mut parent: Option<ContributionId> = None;
    let mut last_manifest = ContentHash::of(b"");
    for d in 0..DECISIONS {
        for k in 0..CHANGED {
            let p = (d * CHANGED + k) * 7 % PATHS;
            versions[p] += 1;
            let content = bytes(p, versions[p]);
            entries[p].hash = ContentHash::of(&content);
            push(
                &mut batch,
                Body::Artifact { name: entries[p].path.clone(), media_type: FILE_MEDIA_TYPE.into(), content },
            );
        }
        let manifest = Manifest::new(entries.clone());
        last_manifest = manifest.content_hash();
        push(&mut batch, artifact(&manifest));
        let decision = Decision {
            v: VERSION,
            line,
            manifest: last_manifest,
            parents: parent.into_iter().collect(),
            proposals: vec![],
            breakage: vec![],
            answers: vec![],
            resolutions: vec![],
            goals: vec![],
            source: Some(bracken_tree::Source { git: format!("{d:040x}") }),
            note: format!("decision {d}"),
        };
        parent = Some(push(&mut batch, artifact(&decision)));
    }
    let count = batch.len();
    let report = s.import(batch).expect("the fixture is written");
    assert!(report.is_clean());
    drop(s);

    let bytes_on_disk: u64 = walk(&dir);
    assert!(bytes_on_disk >= 50_000_000, "the fixture is {bytes_on_disk} bytes, under 50 MB");

    let t = Instant::now();
    let s = Store::open(&dir).expect("the fixture opens");
    let open = t.elapsed();
    let t = Instant::now();
    let ix = Index::build(s.graph());
    let index = t.elapsed();
    let t = Instant::now();
    let lines = Lines::build(s.graph(), &ix);
    let derive = t.elapsed();
    let t = Instant::now();
    let heads = lines.heads(&line);
    let log = lines.log(s.graph(), &line, None);
    let read = t.elapsed();

    assert_eq!(heads, [parent.expect("decisions were written")]);
    assert_eq!(lines.decision(&heads[0]).expect("a decision").decision.manifest, last_manifest);
    assert_eq!(log.len(), DECISIONS);
    assert!(ix.problems().is_empty(), "{:?}", &ix.problems()[..1]);
    println!(
        "fixture: {count} contributions, {:.1} MB on disk ({} build); Store::open {open:?}, Index::build {index:?}, \
         Lines::build {derive:?}, heads and log {read:?}",
        bytes_on_disk as f64 / 1e6,
        if cfg!(debug_assertions) { "debug" } else { "release" },
    );
}

/// The bytes of every file under `dir`.
fn walk(dir: &std::path::Path) -> u64 {
    let mut total = 0;
    for entry in std::fs::read_dir(dir).expect("readable") {
        let entry = entry.expect("an entry");
        let meta = entry.metadata().expect("metadata");
        total += if meta.is_dir() { walk(&entry.path()) } else { meta.len() };
    }
    total
}
