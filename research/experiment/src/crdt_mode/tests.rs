use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use automerge::sync::{self, SyncDoc};
use automerge::{ActorId, AutoCommit, ChangeHash, ObjType, ROOT, ReadDoc, transaction::Transactable};
use serde_json::{Value, json};

use super::wire::{self, Bounds, Rules};
use super::*;

fn store(dir: &Path, identity: &str) -> CrdtStore {
    std::fs::create_dir_all(dir).expect("a store dir");
    CrdtStore {
        config: StoreConfig {
            task: Task::Phase1,
            dir: dir.to_path_buf(),
            bracken: PathBuf::from("/nonexistent"),
            git: PathBuf::from("/nonexistent"),
            identity: identity.into(),
            trace: false,
            accept_rule: Default::default(),
            limit: Default::default(),
        },
        doc: None,
    }
}

/// A hub and copies of it for `agent-0`, `agent-1`, ...; and the copies' keys.
fn setup(root: &Path, copies: usize) -> (CrdtStore, Vec<CrdtStore>, Vec<String>) {
    let mut hub = store(&root.join("hub"), "setter");
    hub.setup(None).expect("the hub");
    let export = hub.export().expect("the export");
    let mut out = Vec::new();
    let mut keys = Vec::new();
    for i in 0..copies {
        let mut c = store(&root.join(format!("a{i}")), &format!("agent-{i}"));
        let reply = c.setup(Some(&export)).expect("a copy");
        keys.push(reply["key"].as_str().expect("a key").to_string());
        out.push(c);
    }
    (hub, out, keys)
}

/// One sync session between `copy` and `hub` through two sockets and a relay, as the driver runs
/// it; the hub's and the copy's results.
fn sync(
    root: &Path,
    hub: &mut CrdtStore,
    copy: &mut CrdtStore,
    peer: &str,
    peers: &[String],
) -> (Result<Value, String>, Result<Value, String>) {
    let (h, c) = (root.join("h.sock"), root.join("c.sock"));
    let _ = std::fs::remove_file(&h);
    let _ = std::fs::remove_file(&c);
    let (hl, cl) = (UnixListener::bind(&h).expect("bind"), UnixListener::bind(&c).expect("bind"));
    let mut ordered: Vec<String> = vec![peer.to_string()];
    ordered.extend(peers.iter().filter(|p| *p != peer).cloned());
    let session = Session { agent: 0, peers: ordered, ..Session::default() };
    std::thread::scope(|scope| {
        let relay = scope.spawn(move || {
            let (a, _) = hl.accept().expect("hub end");
            let (b, _) = cl.accept().expect("copy end");
            let pipe = |from: UnixStream, to: UnixStream| {
                std::thread::spawn(move || {
                    let _ = std::io::copy(&mut &from, &mut &to);
                    let _ = to.shutdown(std::net::Shutdown::Write);
                })
            };
            let up = pipe(b.try_clone().expect("clone"), a.try_clone().expect("clone"));
            let down = pipe(a, b);
            let _ = up.join();
            let _ = down.join();
        });
        let served = scope.spawn(|| hub.sync_server(&h, &session));
        let synced = copy.sync_client(&c, Path::new("/nonexistent"));
        let served = served.join().expect("the hub's side");
        let _ = relay.join();
        (served, synced)
    })
}

/// A test peer's end, held open until the hub closes its own: a session ends on what the peer
/// sent or on its deadline, never because the peer went first.
fn hold(mut peer: UnixStream) {
    use std::io::Read;
    let _ = peer.read_to_end(&mut Vec::new());
}

fn text(s: &mut CrdtStore, file: &str) -> String {
    s.text(file).expect("text")
}

fn heads(s: &mut CrdtStore) -> Vec<ChangeHash> {
    s.doc().expect("doc").get_heads()
}

fn files_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("dir")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn edits_read_replace_and_check_work_on_the_copy_alone() {
    let tmp = tempfile::tempdir().expect("tmp");
    let (mut hub, mut copies, _) = setup(tmp.path(), 1);
    let c = &mut copies[0];
    c.tool("edit", &json!({"file": "base.stk", "start": 1, "end": 0, "text": "def inc 1 : a 1 +"})).expect("edit");
    assert_eq!(text(c, "base.stk"), "def inc 1 : a 1 +\n");
    c.tool("replace", &json!({"file": "base.stk", "old": "a 1 +", "new": "a 2 +"})).expect("replace");
    let read = c.tool("read", &json!({"file": "base.stk"})).expect("read");
    assert_eq!(read["text"], "   1| def inc 1 : a 2 +\n");
    // check resolves in the copy's own text, which is wrong now, before any sync.
    assert_eq!(c.tool("check", &json!({"function": "inc"})).expect("check")["ok"], false);
    c.tool("replace", &json!({"file": "base.stk", "old": "a 2 +", "new": "a 1 +"})).expect("replace");
    assert_eq!(c.tool("check", &json!({"function": "inc"})).expect("check")["ok"], true);
    assert!(hub.integrated().expect("integrated").iter().all(|(_, t)| t.is_empty()), "nothing synced yet");
    // An edit is the smallest splice: one character changed is one delete and one insert.
    let doc = c.doc().expect("doc");
    let before = doc.get_heads();
    c.tool("replace", &json!({"file": "base.stk", "old": "a 1 +", "new": "a 3 +"})).expect("replace");
    let change = c.doc().expect("doc").get_changes(&before).pop().expect("a change");
    assert_eq!(change.decode().operations.len(), 2);
}

#[test]
fn a_copy_and_the_hub_converge_and_concurrent_edits_merge() {
    let tmp = tempfile::tempdir().expect("tmp");
    let (mut hub, mut copies, keys) = setup(tmp.path(), 2);
    let edit = |c: &mut CrdtStore, line: &str| {
        let n = c.text("base.stk").expect("text").lines().count() as i64;
        c.tool("edit", &json!({"file": "base.stk", "start": n + 1, "end": n, "text": line})).expect("edit");
    };
    edit(&mut copies[0], "def inc 1 : a 1 +");
    edit(&mut copies[1], "def neg 1 : 0 a -");
    for _ in 0..2 {
        for (i, c) in copies.iter_mut().enumerate() {
            let (served, synced) = sync(tmp.path(), &mut hub, c, &keys[i], &keys);
            served.expect("hub");
            synced.expect("copy");
        }
    }
    let hub_text = text(&mut hub, "base.stk");
    assert_eq!(hub_text.lines().count(), 2, "{hub_text}");
    for c in &mut copies {
        assert_eq!(text(c, "base.stk"), hub_text);
        assert_eq!(heads(c), heads(&mut hub));
    }
    // The change requests reach the copies through TASKS.md, and check uses them.
    hub.release_changes(1).expect("release");
    let (served, synced) = sync(tmp.path(), &mut hub, &mut copies[0], &keys[0], &keys);
    served.and(synced).expect("sync");
    assert_eq!(
        Task::Phase1.version_of(&text(&mut copies[0], TASKS)),
        Task::Phase1.changes().iter().fold(Version::ORIGINAL, |v, c| v.with(c.change))
    );
}

/// Values that name a path, an option or a look-alike, which no file-name rule may let through.
fn hostile_names() -> Vec<String> {
    vec![
        "-x".into(),
        "--receive-pack=sh".into(),
        "--upload-pack=sh".into(),
        "--output=/tmp/x".into(),
        "--exec=sh".into(),
        "-C".into(),
        "..".into(),
        "../base.stk".into(),
        "/".into(),
        "/etc/passwd".into(),
        "base.stk\0".into(),
        "base.stk\n".into(),
        "x".repeat(20_000),
        "bаse.stk".into(), // Cyrillic а
        ".git".into(),
        "doc.automerge".into(),
        "notes.txt".into(),
    ]
}

/// Every tool's hostile calls: with the checks on, each is refused, the document is unchanged and
/// no file appears in the store.
fn hostile_calls() -> Vec<(&'static str, Value)> {
    let mut calls = Vec::new();
    for name in hostile_names() {
        calls.push(("read", json!({"file": name})));
        calls.push(("edit", json!({"file": name, "start": 1, "end": 0, "text": "x"})));
        calls.push(("replace", json!({"file": name, "old": "a", "new": "b"})));
        calls.push(("check", json!({"function": name})));
    }
    for file in ["base.stk", "NOTES.md"] {
        calls.extend([
            ("edit", json!({"file": file, "start": 1, "end": 0, "text": "a\0b"})),
            ("edit", json!({"file": file, "start": 1, "end": 0, "text": "x".repeat(17 * 1024)})),
            ("edit", json!({"file": file, "start": 0, "end": 0, "text": "x"})),
            ("edit", json!({"file": file, "start": 3, "end": 2, "text": "x"})),
            ("edit", json!({"file": file, "start": "1", "end": 0, "text": "x"})),
            ("edit", json!({"file": file, "start": 1, "end": 0, "text": "x", "path": "/tmp"})),
            ("replace", json!({"file": file, "old": "", "new": "x"})),
            ("replace", json!({"file": file, "old": "absent", "new": "x"})),
        ]);
    }
    calls.extend([
        ("edit", json!({"file": TASKS, "start": 1, "end": 1, "text": "Done."})),
        ("replace", json!({"file": TASKS, "old": "stacklib", "new": "x"})),
        ("read", json!({"file": "base.stk", "start": 0})),
        ("read", json!({"file": "base.stk", "count": -1})),
        ("nonexistent", json!({})),
        ("read", json!(["base.stk"])),
    ]);
    calls
}

#[test]
fn hostile_arguments_are_refused_and_change_nothing() {
    let tmp = tempfile::tempdir().expect("tmp");
    let (_hub, mut copies, _) = setup(tmp.path(), 1);
    let c = &mut copies[0];
    let (before, files) = (heads(c), files_in(&c.config.dir));
    for (tool, args) in hostile_calls() {
        assert!(c.tool(tool, &args).is_err(), "{tool} {args}");
        assert_eq!(heads(c), before, "{tool} {args}");
        assert_eq!(files_in(&c.config.dir), files, "{tool} {args}");
    }
}

#[test]
fn with_the_checks_off_file_names_still_select_only_fixed_objects() {
    // The placement here: a file name is only ever a key into the document's fixed objects, never
    // a path or an argument, so with every rule check off the same values are still refused.
    let tmp = tempfile::tempdir().expect("tmp");
    let (_hub, mut copies, _) = setup(tmp.path(), 1);
    let c = &mut copies[0];
    let (before, files) = (heads(c), files_in(&c.config.dir));
    for name in hostile_names() {
        for (tool, args) in [
            ("read", json!({"file": name})),
            ("edit", json!({"file": name, "start": 1, "end": 0, "text": "x"})),
            ("replace", json!({"file": name, "old": "a", "new": "b"})),
            ("check", json!({"function": name})),
        ] {
            let args = Args::unchecked(&args).expect("an object");
            assert!(c.apply(tool, &args).is_err(), "{tool} {name:?}");
            assert_eq!(heads(c), before);
            assert_eq!(files_in(&c.config.dir), files);
        }
    }
    let args = json!({"file": TASKS, "start": 1, "end": 1, "text": "Done."});
    assert!(c.apply("edit", &Args::unchecked(&args).expect("an object")).is_err(), "TASKS.md stays the setter's");
}

/// A copy whose document is changed directly, outside the tools: what a hostile worker could send.
fn forged(c: &mut CrdtStore, actor: Option<ActorId>, f: impl FnOnce(&mut AutoCommit)) {
    let doc = c.doc().expect("doc");
    if let Some(a) = actor {
        doc.set_actor(a);
    }
    f(doc);
    doc.commit();
}

fn refused_at_the_hub(f: impl FnOnce(&mut CrdtStore)) -> String {
    let tmp = tempfile::tempdir().expect("tmp");
    let (mut hub, mut copies, keys) = setup(tmp.path(), 2);
    let before = (heads(&mut hub), hub.export().expect("export"));
    f(&mut copies[0]);
    let (served, _) = sync(tmp.path(), &mut hub, &mut copies[0], &keys[0], &keys);
    assert_eq!(heads(&mut hub), before.0, "the hub kept nothing");
    // And from disk, as the next worker would load it.
    let mut reopened = store(&tmp.path().join("hub"), "setter");
    assert_eq!(heads(&mut reopened), before.0);
    served.expect_err("the hub refuses")
}

#[test]
fn the_hub_refuses_forged_changes() {
    let tasks = |c: &mut CrdtStore| {
        let obj = c.object(TASKS).expect("tasks");
        forged(c, None, |d| d.splice_text(&obj, 0, 0, "Done. ").map(drop).expect("splice"));
    };
    assert!(refused_at_the_hub(tasks).contains("may not change"));
    let as_other = |c: &mut CrdtStore| {
        let obj = c.object("base.stk").expect("base");
        forged(c, Some(actor_of("agent-1")), |d| d.splice_text(&obj, 0, 0, "x").map(drop).expect("splice"));
    };
    assert!(refused_at_the_hub(as_other).contains("actor"));
    let as_setter = |c: &mut CrdtStore| {
        let obj = c.object("base.stk").expect("base");
        forged(c, Some(actor_of("setter")), |d| d.splice_text(&obj, 0, 0, "x").map(drop).expect("splice"));
    };
    assert!(refused_at_the_hub(as_setter).contains("actor"));
    let new_key = |c: &mut CrdtStore| forged(c, None, |d| d.put(ROOT, "extra", 1).expect("put"));
    assert!(refused_at_the_hub(new_key).contains("root"));
    let nested = |c: &mut CrdtStore| {
        forged(c, None, |d| {
            let mut obj = d.put_object(ROOT, "deep", ObjType::Map).expect("map");
            for _ in 0..2000 {
                obj = d.put_object(&obj, "x", ObjType::Map).expect("map");
            }
        })
    };
    // Its ops on the nested maps come first: each names an object no agent may change.
    let e = refused_at_the_hub(nested);
    assert!(e.contains("root") || e.contains("may not change"), "{e}");
    let mark = |c: &mut CrdtStore| {
        let obj = c.object("base.stk").expect("base");
        forged(c, None, |d| {
            d.splice_text(&obj, 0, 0, "abc").expect("splice");
            d.mark(&obj, automerge::marks::Mark::new("bold".into(), true, 0, 2), automerge::marks::ExpandMark::None)
                .expect("mark");
        })
    };
    assert!(refused_at_the_hub(mark).contains("text insert or delete"));
}

#[test]
fn a_huge_history_is_refused_whole() {
    let msg = refused_at_the_hub(|c| {
        let obj = c.object("base.stk").expect("base");
        let doc = c.doc().expect("doc");
        for i in 0..(wire::MAX_CHANGES + 1) {
            doc.splice_text(&obj, 0, if i == 0 { 0 } else { 1 }, "x").expect("splice");
            doc.commit();
        }
    });
    // An honest copy's first message is its heads and a bloom filter of its history, which is
    // already over the document's cap.
    assert!(msg.contains("over the limit"), "{msg}");
}

/// The hub's rules for agent-0, and its document.
fn hub_doc() -> (tempfile::TempDir, CrdtStore, CrdtStore, Rules) {
    let tmp = tempfile::tempdir().expect("tmp");
    let (mut hub, mut copies, keys) = setup(tmp.path(), 1);
    let rules = hub.rules(&keys, true).expect("rules");
    let copy = copies.remove(0);
    (tmp, hub, copy, rules)
}

/// Messages an honest copy sends the hub, after an edit: the frames of a whole session.
fn honest_frames(hub: &mut CrdtStore, copy: &mut CrdtStore, rules: &Rules) -> Vec<Vec<u8>> {
    copy.tool(
        "edit",
        &json!({"file": "base.stk", "start": 1, "end": 0, "text": "def inc 1 : a 1 +\ndef neg 1 : 0 a -"}),
    )
    .expect("edit");
    let mut probe = AutoCommit::load(&wire::save(hub.doc().expect("doc")).expect("save")).expect("load");
    let (mut cs, mut hs) = (sync::State::new(), sync::State::new());
    let mut frames = Vec::new();
    for _ in 0..10 {
        let Some(m) = copy.doc().expect("doc").sync().generate_sync_message(&mut cs) else { break };
        let bytes = m.encode();
        frames.push(bytes.clone());
        wire::receive(&mut probe, &mut hs, &bytes, rules).expect("honest");
        if let Some(r) = probe.sync().generate_sync_message(&mut hs) {
            copy.doc().expect("doc").sync().receive_sync_message(&mut cs, r).expect("reply");
            cs.their_capabilities = None;
        }
    }
    frames
}

fn refused(doc: &mut AutoCommit, frame: &[u8], rules: &Rules) -> String {
    let before = doc.get_heads();
    let e = wire::receive(doc, &mut sync::State::new(), frame, rules).expect_err("refused");
    assert_eq!(doc.get_heads(), before, "nothing applied: {e}");
    e
}

/// A V1 message: the given parts, then the changes.
fn message(version: u8, heads: &[[u8; 32]], haves: &[Vec<u8>], changes: &[Vec<u8>]) -> Vec<u8> {
    let mut out = vec![version];
    let uleb = |out: &mut Vec<u8>, mut v: u64| loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        out.push(if v == 0 { b } else { b | 0x80 });
        if v == 0 {
            break;
        }
    };
    uleb(&mut out, heads.len() as u64);
    heads.iter().for_each(|h| out.extend(h));
    uleb(&mut out, 0); // need
    uleb(&mut out, haves.len() as u64);
    for bloom in haves {
        uleb(&mut out, 0); // last_sync
        uleb(&mut out, bloom.len() as u64);
        out.extend(bloom);
    }
    uleb(&mut out, changes.len() as u64);
    for c in changes {
        uleb(&mut out, c.len() as u64);
        out.extend(c);
    }
    out
}

#[test]
fn crash_path_1_compressed_chunks_and_v2_documents_are_refused_before_automerge_inflates_them() {
    let (_tmp, mut hub, mut copy, rules) = hub_doc();
    // A change big enough that Automerge compresses it (over 256 bytes): a deflate bomb's shape.
    let obj = copy.object("base.stk").expect("base");
    forged(&mut copy, None, |d| d.splice_text(&obj, 0, 0, &"x".repeat(4000)).map(drop).expect("splice"));
    let mut change = copy.doc().expect("doc").get_last_local_change().expect("a change");
    let compressed = change.bytes().into_owned();
    assert_eq!(compressed[8], 2, "Automerge compressed it");
    let doc = hub.doc().expect("doc");
    assert!(refused(doc, &message(0x42, &[], &[], &[compressed]), &rules).contains("uncompressed"));
    // A whole document, in a V1 message and as V2's.
    let saved = copy.doc().expect("doc").save();
    let doc = hub.doc().expect("doc");
    assert!(refused(doc, &message(0x42, &[], &[], std::slice::from_ref(&saved)), &rules).contains("uncompressed"));
    assert!(refused(doc, &message(0x43, &[], &[], &[saved]), &rules).contains("V1"));
}

#[test]
fn crash_path_2_hostile_bloom_filters_are_refused() {
    let (_tmp, mut hub, _copy, rules) = hub_doc();
    let bloom = |entries: u8, bits: u8, probes: &[u8], len: usize| {
        let mut b = vec![entries, bits];
        b.extend(probes);
        b.extend(vec![0xffu8; len]);
        b
    };
    // Automerge itself panics on this one (a modulus of 0 in get_probes), shown on a scratch copy.
    let zero_bits = bloom(1, 0, &[7], 0);
    let mut scratch = AutoCommit::load(&wire::save(hub.doc().expect("doc")).expect("save")).expect("load");
    scratch.splice_text(&scratch.get(ROOT, "base.stk").expect("get").expect("base").1, 0, 0, "x").expect("splice");
    scratch.commit();
    let raw =
        sync::Message::decode(&message(0x42, &[[9; 32]], std::slice::from_ref(&zero_bits), &[])).expect("decodes");
    let mut state = sync::State::new();
    let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = scratch.sync().receive_sync_message(&mut state, raw);
        scratch.sync().generate_sync_message(&mut state)
    }));
    assert!(crashed.is_err(), "unchecked, Automerge panics");
    let doc = hub.doc().expect("doc");
    for b in [
        zero_bits,
        bloom(1, 10, &[0xff, 0xff, 0xff, 0xff, 0x0f], 2), // 4 billion probes: a 16 GiB allocation per lookup
        bloom(1, 10, &[7], 1),                            // one byte short
        bloom(1, 10, &[7], 3),                            // one byte long
        bloom(0, 10, &[7], 0),
        vec![0xff; 12],
    ] {
        assert!(refused(doc, &message(0x42, &[], std::slice::from_ref(&b), &[]), &rules).contains("bloom"), "{b:?}");
    }
    // An honest filter's shape passes.
    let honest = bloom(1, 10, &[7], 2);
    wire::receive(doc, &mut sync::State::new(), &message(0x42, &[], &[honest], &[]), &rules).expect("honest");
}

#[test]
fn crash_path_3_hashes_it_doesnt_hold_end_a_session_without_a_panic() {
    // Generation `.expect()`s on its own consistency; heads, needs and last-sync hashes it doesn't
    // hold must reach it only as ordinary unknown hashes.
    let (_tmp, mut hub, _copy, rules) = hub_doc();
    let mut rng = bracken_demo::rng::Rng::derive(11, 3);
    for _ in 0..200 {
        let heads: Vec<[u8; 32]> = (0..rng.below(4)).map(|_| [rng.below(256) as u8; 32]).collect();
        let doc = hub.doc().expect("doc");
        let mut state = sync::State::new();
        let frame = message(0x42, &heads, &[], &[]);
        wire::receive(doc, &mut state, &frame, &rules).expect("unknown heads are fine");
        wire::guard(|| doc.sync().generate_sync_message(&mut state)).expect("no panic");
    }
}

#[test]
fn a_change_whose_deps_never_arrive_is_dropped_at_the_end_of_the_session() {
    let (_tmp, mut hub, mut copy, rules) = hub_doc();
    let obj = copy.object("base.stk").expect("base");
    forged(&mut copy, None, |d| d.splice_text(&obj, 0, 0, "a").map(drop).expect("splice"));
    forged(&mut copy, None, |d| d.splice_text(&obj, 1, 0, "b").map(drop).expect("splice"));
    let orphan = copy.doc().expect("doc").get_last_local_change().expect("the second change").raw_bytes().to_vec();
    let before = heads(&mut hub);
    let (mut ours, mut theirs) = UnixStream::pair().expect("a pair");
    let peer = std::thread::spawn(move || {
        use std::io::Write;
        let frame = message(0x42, &[], &[], &[orphan]);
        let _ = theirs.write_all(&(frame.len() as u32).to_be_bytes()).and_then(|()| theirs.write_all(&frame));
        let _ = theirs.write_all(&0u32.to_be_bytes());
        hold(theirs);
    });
    let result = hub.session(&mut ours, &rules, false, Bounds::default());
    drop(ours);
    let _ = peer.join();
    assert!(result.expect_err("refused").contains("deps never arrived"));
    assert_eq!(heads(&mut hub), before);
    assert!(hub.doc().expect("doc").get_missing_deps(&[]).is_empty(), "no orphan is kept");
    let saved = wire::save(hub.doc().expect("doc")).expect("save");
    assert_eq!(AutoCommit::load(&saved).expect("load").get_heads(), before);
}

#[test]
fn held_changes_and_their_dependents_are_dropped_however_the_session_ends() {
    let tmp = tempfile::tempdir().expect("tmp");
    let (mut hub, mut copies, keys) = setup(tmp.path(), 2);
    let rules = hub.rules(&keys, true).expect("rules");
    // A chain a <- b <- c by agent-0; the hub is sent b and c, never a. The copy drops them too.
    let obj = copies[0].object("base.stk").expect("base");
    let mut chain = Vec::new();
    for (i, ch) in ["a", "b", "c"].into_iter().enumerate() {
        forged(&mut copies[0], None, |d| d.splice_text(&obj, i, 0, ch).map(drop).expect("splice"));
        chain.push(copies[0].doc().expect("doc").get_last_local_change().expect("a change").raw_bytes().to_vec());
    }
    copies[0].doc = None;
    let held = message(0x42, &[], &[], &chain[1..]);
    let frame = |m: &[u8]| [(m.len() as u32).to_be_bytes().as_slice(), m].concat();
    let empty = message(0x42, &[], &[], &[]);
    // Only the deadline cases wait on the deadline; the others end on what the peer sent.
    let (fast, slow) = (Bounds { deadline: Duration::from_secs(1), ..Bounds::default() }, Bounds::default());
    let before = heads(&mut hub);
    // What follows the held changes: each way a session can end.
    let endings: Vec<(&str, Vec<u8>, Bounds, bool)> = vec![
        ("deps never arrived", 0u32.to_be_bytes().to_vec(), slow, false),
        ("V1", frame(b"garbage"), slow, false),
        ("deadline", [100u32.to_be_bytes().as_slice(), b"short"].concat(), fast, false),
        ("deadline", vec![], fast, false),
        ("", vec![], slow, true),
        ("budget", frame(&empty).repeat(20), Bounds { budget: (held.len() + 40) as u64, ..slow }, false),
        ("sync messages", frame(&empty).repeat(20), Bounds { frames: 3, ..slow }, false),
    ];
    for (want, tail, bounds, close) in endings {
        let (mut ours, mut theirs) = UnixStream::pair().expect("a pair");
        let bytes = [frame(&held), tail].concat();
        let peer = std::thread::spawn(move || {
            use std::io::Write;
            let _ = theirs.write_all(&bytes);
            if !close {
                hold(theirs);
            }
        });
        let e = hub.session(&mut ours, &rules, false, bounds).expect_err("the session fails");
        drop(ours);
        let _ = peer.join();
        assert!(e.contains(want), "{want}: {e}");
        let doc = hub.doc().expect("doc");
        assert!(doc.get_missing_deps(&[]).is_empty(), "{want}: nothing is held");
        assert_eq!(doc.get_heads(), before, "{want}");
        assert_eq!(store(&tmp.path().join("hub"), "setter").doc().expect("doc").get_heads(), before, "{want}");
    }
    // Nothing carries over: an honest copy then syncs whole, and gets none of the chain.
    let (served, synced) = sync(tmp.path(), &mut hub, &mut copies[1], &keys[1], &keys);
    served.expect("hub");
    synced.expect("copy");
    assert!(copies[1].doc().expect("doc").get_missing_deps(&[]).is_empty());
    assert_eq!(heads(&mut copies[1]), before);
    assert_eq!(text(&mut copies[1], "base.stk"), "");
}

#[test]
fn oversized_malformed_slow_and_over_budget_peers_end_the_session() {
    let (_tmp, mut hub, _copy, rules) = hub_doc();
    let run = |hub: &mut CrdtStore, bytes: Vec<u8>, bounds: Bounds| {
        let (mut ours, mut theirs) = UnixStream::pair().expect("a pair");
        let peer = std::thread::spawn(move || {
            use std::io::Write;
            let _ = theirs.write_all(&bytes);
            hold(theirs);
        });
        let before = heads(hub);
        let start = Instant::now();
        let r = hub.session(&mut ours, &rules, false, bounds);
        let took = start.elapsed();
        drop(ours);
        let _ = peer.join();
        assert_eq!(heads(hub), before);
        (r.expect_err("the session ends with an error"), took)
    };
    // Only the deadline cases wait on the deadline; the others end on what the peer sent.
    let (fast, slow) = (Bounds { deadline: Duration::from_secs(1), ..Bounds::default() }, Bounds::default());
    // A frame over the message limit is refused from its length: the peer never sends a body, so
    // reading one would end at the deadline instead.
    let (e, _) = run(&mut hub, ((wire::MAX_MESSAGE + 1) as u32).to_be_bytes().to_vec(), slow);
    assert!(e.contains("over the limit"), "{e}");
    let (e, _) = run(&mut hub, u32::MAX.to_be_bytes().to_vec(), slow);
    assert!(e.contains("over the limit"), "{e}");
    // Garbage, a truncated frame, and silence: refused, or cut off at the deadline.
    let mut garbage = 5u32.to_be_bytes().to_vec();
    garbage.extend(b"hello");
    assert!(run(&mut hub, garbage, slow).0.contains("V1"));
    let (e, took) = run(&mut hub, [100u32.to_be_bytes().as_slice(), b"short"].concat(), fast);
    assert!(e.contains("deadline") && took < Duration::from_secs(30), "{e}");
    let (e, took) = run(&mut hub, vec![], fast);
    assert!(e.contains("deadline") && took < Duration::from_secs(30), "{e}");
    // Frames past the budget or the frame count, counted before each is read.
    let empty = message(0x42, &[], &[], &[]);
    let mut many = Vec::new();
    for _ in 0..50 {
        many.extend((empty.len() as u32).to_be_bytes());
        many.extend(&empty);
    }
    let (e, _) = run(&mut hub, many.clone(), Bounds { budget: 100, ..slow });
    assert!(e.contains("budget"), "{e}");
    let (e, _) = run(&mut hub, many, Bounds { frames: 10, ..slow });
    assert!(e.contains("sync messages"), "{e}");
}

#[test]
fn mutated_honest_messages_are_refused_whole_or_applied_without_a_panic() {
    let (_tmp, mut hub, mut copy, rules) = hub_doc();
    let frames = honest_frames(&mut hub, &mut copy, &rules);
    assert!(frames.len() >= 2 && frames.iter().any(|f| f.len() > 100), "a session with changes");
    let base = AutoCommit::load(&wire::save(hub.doc().expect("doc")).expect("save")).expect("load");
    let mut rng = bracken_demo::rng::Rng::derive(7, 1);
    let (mut refused_n, mut applied_n) = (0, 0);
    for _ in 0..2000 {
        let mut f = frames[rng.below(frames.len())].clone();
        match rng.below(5) {
            0 => {
                let i = rng.below(f.len());
                f[i] ^= 1 << rng.below(8);
            }
            1 => f.truncate(rng.below(f.len())),
            2 => {
                let i = rng.below(f.len() + 1);
                f.insert(i, rng.below(256) as u8);
            }
            3 => {
                let i = rng.below(f.len());
                f[i] = [0x00, 0x7f, 0x80, 0xff][rng.below(4)];
            }
            _ => {
                let i = rng.below(f.len());
                f.splice(i..i, [0xff, 0xff, 0xff, 0xff, 0x0f]);
            }
        }
        let mut doc = base.clone();
        let before = doc.get_heads();
        let mut state = sync::State::new();
        match wire::receive(&mut doc, &mut state, &f, &rules) {
            Err(e) => {
                assert!(!e.contains("Automerge failed"), "the checks let through what crashed Automerge: {e}");
                assert_eq!(doc.get_heads(), before, "a refused message changed nothing");
                refused_n += 1;
            }
            Ok(_) => {
                wire::guard(|| doc.sync().generate_sync_message(&mut state)).expect("no panic after");
                let saved = wire::save(&mut doc).expect("it saves");
                assert_eq!(AutoCommit::load(&saved).expect("it loads").get_heads(), doc.get_heads());
                applied_n += 1;
            }
        }
    }
    // A mutant can still be a well-formed message from the allowed actor (a flipped head hash, or a
    // different character in a change: changes are hashed, not signed), and is then applied.
    assert!(refused_n > 1000 && applied_n > 0, "{refused_n} refused, {applied_n} applied");
}

#[test]
fn a_hostile_hub_export_is_refused() {
    let tmp = tempfile::tempdir().expect("tmp");
    let (mut hub, _, _) = setup(tmp.path(), 0);
    let mut extra = AutoCommit::load(&hub.export().expect("export")).expect("load");
    extra.put(ROOT, "extra", 1).expect("put");
    let mut by_other = AutoCommit::load(&hub.export().expect("export")).expect("load");
    by_other.set_actor(actor_of("agent-0"));
    let notes = by_other.get(ROOT, NOTES).expect("get").expect("notes").1;
    by_other.splice_text(&notes, 0, 0, "x").expect("splice");
    for (i, bad) in
        [extra.save(), by_other.save(), vec![0x85, 0x6f, 0x4a, 0x83, 0, 0], vec![0; wire::MAX_DOC_BYTES + 1]]
            .into_iter()
            .enumerate()
    {
        let mut c = store(&tmp.path().join(format!("bad{i}")), "agent-0");
        assert!(c.setup(Some(&bad)).is_err(), "case {i}");
    }
}

/// A change like `honest`, rewritten by `f`, as its new bytes.
fn rewrite(honest: &automerge::Change, f: impl FnOnce(&mut automerge::ExpandedChange)) -> Vec<u8> {
    let mut expanded = honest.decode();
    expanded.hash = None;
    f(&mut expanded);
    automerge::Change::from(expanded).raw_bytes().to_vec()
}

/// A session the hub serves for `keys[0]` through its own `sync_server`, with a peer that sends
/// `frames` and then nothing more; the hub's result.
fn hostile_session(root: &Path, hub: &mut CrdtStore, keys: &[String], frames: Vec<Vec<u8>>) -> Result<Value, String> {
    use std::io::{Read, Write};
    let path = root.join("hostile.sock");
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).expect("bind");
    let peer = std::thread::spawn(move || {
        let (mut s, _) = listener.accept().expect("accept");
        for f in frames.iter().chain([&Vec::new()]) {
            let _ = s.write_all(&(f.len() as u32).to_be_bytes()).and_then(|()| s.write_all(f));
        }
        let _ = s.shutdown(std::net::Shutdown::Write);
        let _ = s.read_to_end(&mut Vec::new());
    });
    let r = hub.sync_server(&path, &Session { agent: 0, peers: keys.to_vec(), ..Session::default() });
    let _ = peer.join();
    r
}

/// The hub's document in memory and on disk, the same and loadable; its heads.
fn hub_is_sound(root: &Path, hub: &mut CrdtStore) -> Vec<ChangeHash> {
    let now = heads(hub);
    let mut reopened = store(&root.join("hub"), "setter");
    assert_eq!(heads(&mut reopened), now, "the saved document is the one in memory");
    for file in files(Task::Phase1) {
        reopened.text(file).expect("every text reads");
    }
    let saved = wire::save(hub.doc().expect("doc")).expect("it saves");
    assert_eq!(AutoCommit::load(&saved).expect("it loads").get_heads(), now);
    now
}

/// A hub, and agent-0's first change (one character into `base.stk`) unsent.
fn hostile_setup(root: &Path) -> (CrdtStore, Vec<CrdtStore>, Vec<String>, automerge::Change) {
    let (hub, mut copies, keys) = setup(root, 2);
    copies[0].tool("edit", &json!({"file": "base.stk", "start": 1, "end": 0, "text": "a"})).expect("edit");
    let honest = copies[0].doc().expect("doc").get_last_local_change().expect("a change").clone();
    (hub, copies, keys, honest)
}

#[test]
fn after_an_automerge_panic_or_a_bad_save_the_hub_reloads_its_last_save() {
    use automerge::legacy::{ElementId, Key, OpId, OpType};
    let tmp = tempfile::tempdir().expect("tmp");
    let (mut hub, mut copies, keys, honest) = hostile_setup(tmp.path());
    let setter = actor_of("setter");
    let before = hub_is_sound(tmp.path(), &mut hub);
    let key = |op: OpId| move |c: &mut automerge::ExpandedChange| c.operations[0].key = Key::Seq(ElementId::Id(op));
    let cases = [
        // Automerge panics halfway through applying these (findings f28fcc5b25ac, 70cba757d87f):
        // a key with an actor it doesn't know, or a counter no op has.
        rewrite(&honest, key(OpId(1, ActorId::from(vec![7u8; 32])))),
        rewrite(&honest, key(OpId(999_999, setter.clone()))),
        // Automerge applies this one, a delete in base.stk of an element of TASKS.md, and then
        // saves a document that doesn't load (finding 0357e515ecaa).
        rewrite(&honest, |c| {
            let op = &mut c.operations[0];
            op.action = OpType::Delete;
            op.insert = false;
            op.key = Key::Seq(ElementId::Id(OpId(10, setter.clone())));
            op.pred = vec![OpId(10, setter.clone())].into();
        }),
    ];
    for (i, bytes) in cases.into_iter().enumerate() {
        let e =
            hostile_session(tmp.path(), &mut hub, &keys, vec![message(0x42, &[], &[], &[bytes])]).expect_err("dropped");
        assert!(e.contains("Automerge failed") || e.contains("load back"), "case {i}: {e}");
        assert_eq!(hub_is_sound(tmp.path(), &mut hub), before, "case {i}");
    }
    // An honest agent then syncs as usual.
    let (served, synced) = sync(tmp.path(), &mut hub, &mut copies[1], &keys[1], &keys);
    served.expect("hub");
    synced.expect("copy");
    hub_is_sound(tmp.path(), &mut hub);
}

#[test]
fn a_message_over_the_op_cap_is_refused() {
    let tmp = tempfile::tempdir().expect("tmp");
    let (_hub, mut copies, _) = setup(tmp.path(), 1);
    let obj = copies[0].object("base.stk").expect("base");
    let doc = copies[0].doc().expect("doc");
    let mut changes = Vec::new();
    for _ in 0..2 {
        doc.splice_text(&obj, 0, 0, &"x".repeat(wire::MAX_MESSAGE_OPS / 2 + 1)).expect("insert");
        doc.commit();
        changes.push(doc.get_last_local_change().expect("a change").raw_bytes().to_vec());
    }
    let e = wire::check_message(&message(0x42, &[], &[], &changes)).err().expect("refused");
    assert!(e.contains("a message of over"), "{e}");
    assert!(wire::check_message(&message(0x42, &[], &[], &changes[..1])).is_ok());
}

#[test]
fn a_document_that_wouldnt_load_back_is_never_saved() {
    use automerge::legacy::{ElementId, Key, OpId, OpType};
    // Automerge panics halfway through this change, a delete in base.stk of an element of
    // TASKS.md, and the half-applied document saves as one that doesn't load (findings
    // 70cba757d87f, 0357e515ecaa). Here the panic is caught but the document kept, to show that
    // the save still refuses it.
    let tmp = tempfile::tempdir().expect("tmp");
    let (mut hub, _copies, _keys, honest) = hostile_setup(tmp.path());
    let before = hub_is_sound(tmp.path(), &mut hub);
    let setter = actor_of("setter");
    let bad = rewrite(&honest, |c| {
        let op = &mut c.operations[0];
        op.action = OpType::Delete;
        op.insert = false;
        op.key = Key::Seq(ElementId::Id(OpId(10, setter.clone())));
        op.pred = vec![OpId(10, setter.clone())].into();
    });
    let change = automerge::Change::from_bytes(bad).expect("a change");
    let doc = hub.doc().expect("doc");
    assert!(wire::guard(|| doc.apply_changes([change])).is_err(), "Automerge panics");
    let e = hub.save().expect_err("not saved");
    assert!(e.contains("load back"), "{e}");
    assert!(hub.doc.is_none(), "the document is thrown away");
    assert_eq!(hub_is_sound(tmp.path(), &mut hub), before);
}
