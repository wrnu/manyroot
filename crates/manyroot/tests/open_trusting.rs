//! `Store::open_trusting` against `Store::open`, the oracle: on generated replicas holding claims,
//! artifacts, evidence, relations (supersessions among them) and retractions, trusting any causally
//! closed set of contributions gives the same graph as the full open, and every way a trusted set
//! can fail to fit the replica is refused. The one thing it can't see, an overwritten trusted object,
//! is shown here as the documented caveat.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use manyroot::{Body, Contribution, ContributionId, Graph, Identity, RelationKind, Store, StoreError, Verdict};

struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 33) as usize) % n.max(1)
    }
}

/// A replica of about `n` contributions by three authors, each with up to three random parents.
fn replica(dir: &Path, seed: u64, n: usize) -> (Store, Vec<Identity>) {
    let mut store = Store::init(dir, "trusting").expect("a new replica");
    let people: Vec<Identity> = (0..3).map(|i| Identity::generate(&format!("p{i}")).expect("a key")).collect();
    let mut rng = Rng(seed);
    let mut ids: Vec<ContributionId> = Vec::new();
    for t in 0..n as u64 {
        let who = &people[rng.below(people.len())];
        let pick = |rng: &mut Rng| ids[rng.below(ids.len())];
        let body = match (ids.len(), rng.below(10)) {
            (0 | 1, _) | (_, 0..=2) => Body::Claim { statement: format!("claim {t}"), tags: vec!["t".into()] },
            (_, 3) => {
                Body::Artifact { name: format!("a{t}.rs"), media_type: "text/x-rust".into(), content: vec![t as u8] }
            }
            (_, 4 | 5) => Body::Evidence {
                target: pick(&mut rng),
                verdict: Verdict::Pass,
                method: "test".into(),
                details: String::new(),
            },
            (_, 6..=8) => {
                let (from, to) = (pick(&mut rng), pick(&mut rng));
                if from == to {
                    continue;
                }
                let kind =
                    [RelationKind::Supersedes, RelationKind::Implements, RelationKind::Contradicts][rng.below(3)];
                Body::Relation { from, kind, to, note: String::new() }
            }
            _ => {
                // A retraction of one of the author's own contributions, if they have one.
                let own: Vec<_> = ids
                    .iter()
                    .filter(|id| store.graph().get(id).expect("present").author() == who.agent_id())
                    .collect();
                if own.is_empty() {
                    continue;
                }
                Body::Retract { target: *own[rng.below(own.len())], reason: "no".into() }
            }
        };
        let parents: Vec<_> = if ids.is_empty() { vec![] } else { (0..rng.below(3)).map(|_| pick(&mut rng)).collect() };
        let c = Contribution::create(store.graph().ground(), who, parents, t, body);
        ids.push(c.id());
        store.add(c).expect("valid");
    }
    (store, people)
}

fn assert_same(a: &Graph, b: &Graph) {
    assert_eq!(a.topo_order(), b.topo_order(), "contributions and their order");
    assert_eq!(a.heads().collect::<Vec<_>>(), b.heads().collect::<Vec<_>>(), "heads");
    for id in a.topo_order() {
        assert_eq!(a.get(&id), b.get(&id), "{id}");
        assert_eq!(a.is_retracted(&id), b.is_retracted(&id), "{id} retracted");
        assert_eq!(a.children(&id), b.children(&id), "{id}'s children");
        let referrers = |g: &Graph| g.referrers(&id).map(Contribution::id).collect::<Vec<_>>();
        assert_eq!(referrers(a), referrers(b), "{id}'s referrers");
    }
}

/// The contributions of `ids` and all their ancestors, in causal order.
fn closed(g: &Graph, ids: &[ContributionId]) -> Vec<Contribution> {
    let mut keep: BTreeSet<ContributionId> = BTreeSet::new();
    let mut todo: Vec<ContributionId> = ids.to_vec();
    while let Some(id) = todo.pop() {
        if keep.insert(id) {
            todo.extend(g.get(&id).expect("present").parents());
        }
    }
    g.topo_order().into_iter().filter(|id| keep.contains(id)).map(|id| g.get(&id).expect("present").clone()).collect()
}

fn object(dir: &Path, id: &ContributionId) -> PathBuf {
    let hex = id.to_hex();
    dir.join("objects").join(&hex[..2]).join(&hex[2..])
}

fn write_object(dir: &Path, c: &Contribution, bytes: &[u8]) -> PathBuf {
    let path = object(dir, &c.id());
    fs::create_dir_all(path.parent().expect("a shard")).expect("made");
    fs::write(&path, bytes).expect("written");
    path
}

#[test]
fn trusting_any_closed_set_gives_store_opens_graph() {
    for seed in 1..=4 {
        let tmp = tempfile::tempdir().expect("a temp dir");
        let dir = tmp.path().join("g");
        replica(&dir, seed, 300);
        let full = Store::open(&dir).expect("valid");
        let g = full.graph();
        let has = |f: fn(&Body) -> bool| g.iter().any(|c| f(c.body()));
        assert!(has(|b| matches!(b, Body::Retract { .. })), "seed {seed} has retractions");
        assert!(has(|b| matches!(b, Body::Relation { kind: RelationKind::Supersedes, .. })), "and supersessions");
        let order = g.topo_order();
        let mut rng = Rng(seed * 7);
        let picks: Vec<_> = (0..5).map(|_| order[rng.below(order.len())]).collect();
        let sets = [
            Vec::new(),
            closed(g, &order[..order.len() / 3]),
            closed(g, &picks),
            closed(g, &g.heads().collect::<Vec<_>>()),
        ];
        for trusted in sets {
            let n = trusted.len();
            let opened = Store::open_trusting(&dir, trusted).expect("fits");
            assert_same(opened.graph(), g);
            assert_eq!(opened.info().ground, full.info().ground, "seed {seed}, {n} trusted");
        }
    }
}

#[test]
fn a_trusted_set_that_does_not_fit_is_refused() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    let dir = tmp.path().join("g");
    replica(&dir, 9, 120);
    let g = Store::open(&dir).expect("valid").graph().clone();
    let all = closed(&g, &g.heads().collect::<Vec<_>>());
    let untrusted = |r: Result<Store, StoreError>, why: &str| match r {
        Err(StoreError::Untrusted { reason, .. }) => assert!(reason.contains(why), "{reason}"),
        Err(e) => panic!("{why}: {e}"),
        Ok(_) => panic!("{why}: opened"),
    };

    let mut twice = all.clone();
    twice.push(all[5].clone());
    untrusted(Store::open_trusting(&dir, twice), "twice");

    let mut backwards = all.clone();
    backwards.reverse();
    untrusted(Store::open_trusting(&dir, backwards), "isn't trusted before it");

    let (other, _) = replica(&tmp.path().join("other"), 9, 5);
    let mut foreign = all.clone();
    foreign.push(other.graph().iter().next().expect("one").clone());
    untrusted(Store::open_trusting(&dir, foreign), "another Ground");

    // A trusted contribution whose object was deleted: only the full open can say what's left.
    let gone = all.last().expect("some").id();
    let bytes = fs::read(object(&dir, &gone)).expect("present");
    fs::remove_file(object(&dir, &gone)).expect("removed");
    untrusted(Store::open_trusting(&dir, all.clone()), "isn't in the replica");
    fs::write(object(&dir, &gone), &bytes).expect("restored");
    Store::open_trusting(&dir, all.clone()).expect("fits again");

    // Its object replaced by something that isn't a regular file: a dangling link, a link to its own
    // bytes elsewhere, a directory. The listing's file type, without following the link, says so.
    let path = object(&dir, &gone);
    let elsewhere = tmp.path().join("elsewhere");
    fs::write(&elsewhere, &bytes).expect("written");
    #[cfg(unix)]
    for target in [tmp.path().join("nothing"), elsewhere.clone()] {
        fs::remove_file(&path).expect("removed");
        std::os::unix::fs::symlink(&target, &path).expect("linked");
        untrusted(Store::open_trusting(&dir, all.clone()), "isn't a regular file");
    }
    let _ = fs::remove_file(&path);
    fs::create_dir(&path).expect("a directory in its place");
    untrusted(Store::open_trusting(&dir, all.clone()), "isn't a regular file");
    assert!(Store::open(&dir).is_err(), "the full open refuses a directory there too");
    fs::remove_dir(&path).expect("removed");
    fs::write(&path, bytes).expect("restored");
    Store::open_trusting(&dir, all).expect("fits again");
}

#[test]
fn new_objects_are_checked_as_store_open_checks_them() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    let dir = tmp.path().join("g");
    let (store, people) = replica(&dir, 11, 120);
    let g = store.graph().clone();
    let trusted = closed(&g, &g.heads().collect::<Vec<_>>());
    let heads: Vec<_> = g.heads().collect();
    let check = |name: &str, want: &str| {
        let full = Store::open(&dir).err().unwrap_or_else(|| panic!("{name}: Store::open accepts it"));
        let trusting = Store::open_trusting(&dir, trusted.clone()).err().unwrap_or_else(|| panic!("{name}: accepted"));
        let (full, trusting) = (format!("{full}: {full:?}"), format!("{trusting}: {trusting:?}"));
        assert_eq!(trusting, full, "{name}: the same refusal");
        assert!(full.contains(want), "{name}: {full}");
    };
    let claim = |t: u64, statement: &str| {
        Contribution::create(
            g.ground(),
            &people[0],
            heads.clone(),
            t,
            Body::Claim { statement: statement.into(), tags: vec![] },
        )
    };

    // A bad signature on a new object: the encoding's last byte is the signature's.
    let c = claim(1, "x");
    let mut bytes = c.encode();
    *bytes.last_mut().expect("not empty") ^= 1;
    let path = write_object(&dir, &c, &bytes);
    check("bad signature", "signature is not valid");
    fs::remove_file(&path).expect("removed");

    // A blank statement, in Unicode whitespace only.
    let c = claim(2, "\u{3000}\n");
    let path = write_object(&dir, &c, &c.encode());
    check("blank", "claim statement");
    fs::remove_file(&path).expect("removed");

    // A retraction of another author's work.
    let theirs = g.iter().find(|c| c.author() != people[0].agent_id()).expect("someone else's").id();
    let c = Contribution::create(
        g.ground(),
        &people[0],
        heads.clone(),
        3,
        Body::Retract { target: theirs, reason: "no".into() },
    );
    let path = write_object(&dir, &c, &c.encode());
    check("retract by another", "only the author");
    fs::remove_file(&path).expect("removed");

    // A new object whose parent is missing from the replica.
    let lost = claim(4, "never written");
    let c = Contribution::create(
        g.ground(),
        &people[0],
        [lost.id()],
        5,
        Body::Claim { statement: "orphan".into(), tags: vec![] },
    );
    let path = write_object(&dir, &c, &c.encode());
    check("missing parent", "ancestor is missing");
    fs::remove_file(&path).expect("removed");

    // An object under a name other than its id's lowercase hex.
    let id = g.topo_order()[3];
    let lower = object(&dir, &id);
    let upper = lower.with_file_name(lower.file_name().expect("named").to_string_lossy().to_uppercase());
    assert_ne!(lower, upper);
    fs::rename(&lower, &upper).expect("renamed");
    let err = Store::open_trusting(&dir, trusted.clone()).err().expect("refused").to_string();
    assert!(err.contains("not named by a contribution id"), "{err}");
    assert!(Store::open(&dir).is_err(), "Store::open refuses it too");
    fs::rename(&upper, &lower).expect("renamed back");

    Store::open_trusting(&dir, trusted).expect("valid again");
}

/// The trade `open_trusting` makes, by design: a trusted object's bytes aren't read, so a later
/// overwrite goes unnoticed. `Store::open` catches it.
#[test]
fn an_overwritten_trusted_object_goes_unnoticed_where_store_open_catches_it() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    let dir = tmp.path().join("g");
    replica(&dir, 13, 60);
    let g = Store::open(&dir).expect("valid").graph().clone();
    let trusted = closed(&g, &g.heads().collect::<Vec<_>>());
    let order = g.topo_order();
    let other = g.get(&order[1]).expect("present").encode();
    fs::write(object(&dir, &order[0]), other).expect("overwritten");
    assert!(Store::open(&dir).is_err(), "the full open sees it");
    let opened = Store::open_trusting(&dir, trusted).expect("the trusting open doesn't");
    assert_same(opened.graph(), &g);
}
