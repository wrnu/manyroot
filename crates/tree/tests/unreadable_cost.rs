//! What an undecodable line or decision artifact costs a reader (finding 3832a3c04d2d): its top-level `v`
//! and `line` are read without building a value of the rest, so any writer's crafted payload costs each
//! command about its own size, not a generic value tree many times larger. G8 still counts it.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::collections::BTreeMap;
use std::rc::Rc;

use bracken_tree::ContentHash;
use bracken_tree::format::{
    Breakage, DECISION_MEDIA_TYPE, Decision, FormatError, Guardrails, LINE_MEDIA_TYPE, Line, Marker, Record, VERSION,
    to_cbor,
};
use bracken_tree::ground::{Index, Problem};
use bracken_tree::lines::Lines;
use manyroot::{Body, Contribution, ContributionId, Graph, GroundId, Identity, RelationKind};

const GROUND: GroundId = GroundId([11; 32]);

/// The system allocator, counting the bytes each thread asks for.
struct Counting;

thread_local! {
    static ALLOCATED: Cell<u64> = const { Cell::new(0) };
}

fn count(bytes: usize) {
    let _ = ALLOCATED.try_with(|a| a.set(a.get() + bytes as u64));
}

fn allocated() -> u64 {
    ALLOCATED.with(Cell::get)
}

// SAFETY: each call passes its arguments to the system allocator unchanged.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count(layout.size());
        // SAFETY: as for System.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count(layout.size());
        // SAFETY: as for System.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: as for System.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        count(new_size);
        // SAFETY: as for System.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// How many one-byte empty arrays the crafted payload holds: each was a generic value of tens of bytes.
const ITEMS: usize = 1 << 20;

/// A Ground with alice's marker and `main` line, alice its integrator.
struct World {
    g: Graph,
    clock: u64,
    alice: Rc<Identity>,
    line: ContributionId,
}

impl World {
    fn new() -> World {
        let alice = Rc::new(Identity::generate("alice").expect("a valid name"));
        let mut w = World { g: Graph::new(GROUND), clock: 1_700_000_000_000, alice, line: ContributionId([0; 32]) };
        let marker = Marker { v: VERSION, purpose: "test".into(), in_place: None };
        w.add(&w.alice.clone(), artifact(marker.artifact_name(), Marker::MEDIA_TYPE, marker.encode()));
        let policy = Line {
            v: VERSION,
            name: "main".into(),
            integrators: vec![w.alice.agent_id()],
            validators: vec![],
            goal_setters: vec![],
            checks: BTreeMap::new(),
            executors: BTreeMap::new(),
            guardrails: Guardrails { max_known: 0, max_age: 0, protected: vec![] },
            owner_decision: Some("decision 0123456789ab".into()),
        };
        w.line = w.add(&w.alice.clone(), artifact("main", LINE_MEDIA_TYPE, policy.encode()));
        w
    }

    fn add(&mut self, who: &Identity, body: Body) -> ContributionId {
        self.clock += 1;
        let c = Contribution::create(GROUND, who, self.g.heads().collect::<Vec<_>>(), self.clock, body);
        let id = c.id();
        self.g.insert(c).expect("a valid contribution");
        id
    }

    /// The index and lines built from this Ground, as every command builds them, and the bytes building
    /// both allocated.
    fn lines(&self) -> (Lines, u64) {
        let ((_, lines), cost) = self.built();
        (lines, cost)
    }

    fn built(&self) -> ((Index, Lines), u64) {
        let before = allocated();
        let ix = Index::build(&self.g, Some(self.alice.agent_id()));
        let lines = Lines::build(&self.g, &ix);
        ((ix, lines), allocated() - before)
    }
}

fn artifact(name: &str, media_type: &str, content: Vec<u8>) -> Body {
    Body::Artifact { name: name.into(), media_type: media_type.into(), content }
}

/// `{v: 2, line: <line>, x: [[], [], ...]}`: a version this build doesn't read, on `line`, padded with
/// `ITEMS` empty arrays.
fn crafted(line: ContributionId) -> Vec<u8> {
    let mut head = to_cbor(&ciborium::Value::Map(vec![
        (ciborium::Value::Text("v".into()), ciborium::Value::Integer(2.into())),
        (ciborium::Value::Text("line".into()), ciborium::Value::Bytes(line.0.to_vec())),
        (ciborium::Value::Text("x".into()), ciborium::Value::Array(vec![])),
    ]));
    head[0] = 0xa3;
    head.pop(); // the empty array's header, rewritten below with its length
    head.extend([0x9a]);
    head.extend((ITEMS as u32).to_be_bytes());
    head.extend(std::iter::repeat_n(0x80, ITEMS));
    head
}

#[test]
fn a_crafted_unreadable_decision_costs_about_its_own_size_and_still_counts() {
    let mut w = World::new();
    let (_, baseline) = w.lines();
    let payload = crafted(w.line);
    let decision = w.add(&w.alice.clone(), artifact("d", DECISION_MEDIA_TYPE, payload.clone()));
    let (lines, cost) = w.lines();
    let extra = cost.saturating_sub(baseline);
    // A generic value tree took over 30 bytes per item; reading the head now takes none per item.
    assert!(
        extra < payload.len() as u64 / 4,
        "building lines allocated {extra} bytes for a {}-byte payload",
        payload.len()
    );
    // G8: it still counts as unreadable on its line, naming its version.
    let unreadable: Vec<_> = lines.unreadable().iter().map(|u| (u.id, u.line, u.why.clone())).collect();
    assert_eq!(unreadable, vec![(decision, w.line, FormatError::Version(2))]);
}

#[test]
fn a_crafted_unreadable_policy_version_costs_about_its_own_size_and_still_counts() {
    let mut w = World::new();
    let (_, baseline) = w.lines();
    let payload = crafted(w.line);
    let version = w.add(&w.alice.clone(), artifact("main", LINE_MEDIA_TYPE, payload.clone()));
    let relation = Body::Relation { from: version, kind: RelationKind::Supersedes, to: w.line, note: String::new() };
    w.add(&w.alice.clone(), relation);
    let (lines, cost) = w.lines();
    let extra = cost.saturating_sub(baseline);
    assert!(
        extra < payload.len() as u64 / 4,
        "building lines allocated {extra} bytes for a {}-byte payload",
        payload.len()
    );
    let unreadable: Vec<_> = lines.unreadable().iter().map(|u| (u.id, u.line, u.why.clone())).collect();
    assert_eq!(unreadable, vec![(version, w.line, FormatError::Version(2))]);
}

/// A policy line for `line`'s policy in v1's field types, at version 2, its `protected` padded with `ITEMS`
/// empty paths: the shape every field of which a typed decode builds (finding 8b99e0d6d326).
fn typed_line(alice: &Identity) -> Vec<u8> {
    to_cbor(&Line {
        v: 2,
        name: "main".into(),
        integrators: vec![alice.agent_id()],
        validators: vec![],
        goal_setters: vec![],
        checks: BTreeMap::new(),
        executors: BTreeMap::new(),
        guardrails: Guardrails { max_known: 0, max_age: 0, protected: vec![String::new(); ITEMS] },
        owner_decision: Some("decision 0123456789ab".into()),
    })
}

/// A decision on `line` in v1's field types, at version 2, with one breakage of `ITEMS` empty paths
/// (finding 8b99e0d6d326's counterexample).
fn typed_decision(line: ContributionId) -> Vec<u8> {
    to_cbor(&Decision {
        v: 2,
        line,
        manifest: ContentHash::of(b""),
        parents: vec![],
        proposals: vec![],
        breakage: vec![Breakage { check: String::new(), detail: String::new(), paths: vec![String::new(); ITEMS] }],
        answers: vec![],
        resolutions: vec![],
        goals: vec![],
        source: None,
        note: String::new(),
    })
}

/// What `weave check` would list for `id`: its invalid payload's error.
fn invalid(ix: &Index, id: ContributionId) -> Option<FormatError> {
    ix.problems().iter().find_map(|p| match p {
        Problem::InvalidPayload { id: of, error, .. } if *of == id => Some(error.clone()),
        _ => None,
    })
}

#[test]
fn a_typed_unreadable_decision_costs_about_its_own_size_and_still_counts() {
    let mut w = World::new();
    let (_, baseline) = w.built();
    let payload = typed_decision(w.line);
    let decision = w.add(&w.alice.clone(), artifact("d", DECISION_MEDIA_TYPE, payload.clone()));
    let ((ix, lines), cost) = w.built();
    let extra = cost.saturating_sub(baseline);
    // A typed decode built a String for each path, over 60 bytes per byte of payload; reading the head
    // first decodes nothing of a version this build doesn't read.
    assert!(
        extra < payload.len() as u64 / 4,
        "building the index and lines allocated {extra} bytes for a {}-byte payload",
        payload.len()
    );
    let unreadable: Vec<_> = lines.unreadable().iter().map(|u| (u.id, u.line, u.why.clone())).collect();
    assert_eq!(unreadable, vec![(decision, w.line, FormatError::Version(2))]);
    assert_eq!(invalid(&ix, decision), Some(FormatError::Version(2)));
}

#[test]
fn a_typed_unreadable_policy_version_costs_about_its_own_size_and_still_counts() {
    let mut w = World::new();
    let (_, baseline) = w.built();
    let payload = typed_line(&w.alice);
    let version = w.add(&w.alice.clone(), artifact("main", LINE_MEDIA_TYPE, payload.clone()));
    let relation = Body::Relation { from: version, kind: RelationKind::Supersedes, to: w.line, note: String::new() };
    w.add(&w.alice.clone(), relation);
    let ((ix, lines), cost) = w.built();
    let extra = cost.saturating_sub(baseline);
    assert!(
        extra < payload.len() as u64 / 4,
        "building the index and lines allocated {extra} bytes for a {}-byte payload",
        payload.len()
    );
    let unreadable: Vec<_> = lines.unreadable().iter().map(|u| (u.id, u.line, u.why.clone())).collect();
    assert_eq!(unreadable, vec![(version, w.line, FormatError::Version(2))]);
    assert_eq!(invalid(&ix, version), Some(FormatError::Version(2)));
}
