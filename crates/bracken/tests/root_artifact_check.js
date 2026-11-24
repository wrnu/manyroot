#!/usr/bin/env node
// Deterministic check for Root view rendering when opened on an artifact vs a claim.
// Runs crates/bracken/assets/view/{common,root}.js in Node's vm module with a minimal
// mock DOM — no browser, server, or external dependency.
// Note: run by hand (not by scripts/test-table, since Node is not in the test table or boat box).
//
// Run: node crates/bracken/tests/root_artifact_check.js
"use strict";

const assert = require("assert");
const fs = require("fs");
const path = require("path");
const vm = require("vm");

const VIEW_DIR = path.resolve(__dirname, "../assets/view");
const COMMON_SRC = fs.readFileSync(path.join(VIEW_DIR, "common.js"), "utf8");
const ROOT_SRC = fs.readFileSync(path.join(VIEW_DIR, "root.js"), "utf8");

function hexId(n) {
  return n.toString(16).padStart(64, "0");
}

class MockNode {
  constructor(tag) {
    this.tag = tag;
    this.children = [];
    this.attrs = {};
    this.listeners = {};
    this.dataset = {};
    this.style = { setProperty() {} };
    this.className = "";
    this.classList = { toggle() {} };
    this.disabled = false;
    this.parentElement = null;
    this._text = "";
  }
  get textContent() {
    if (this.tag === "#text") return this._text;
    return this.children.length ? this.children.map((c) => c.textContent).join("") : this._text;
  }
  set textContent(v) {
    this.children = [];
    this._text = String(v);
  }
  setAttribute(k, v) { this.attrs[k] = String(v); }
  getAttribute(k) { return this.attrs[k]; }
  append(...nodes) {
    for (const n of nodes) {
      const node = typeof n === "string" ? MockNode.text(n) : n;
      node.parentElement = this;
      this.children.push(node);
    }
  }
  replaceChildren(...nodes) {
    this.children = [];
    this._text = "";
    this.append(...nodes);
  }
  addEventListener(ev, fn) { (this.listeners[ev] ||= []).push(fn); }
  *walk() {
    yield this;
    for (const c of this.children) yield* c.walk();
  }
  static text(s) {
    const n = new MockNode("#text");
    n._text = String(s);
    return n;
  }
}

function makeTrackCanvas() {
  const el = new MockNode("canvas");
  el.attrs.id = "track-canvas";
  el.getBoundingClientRect = () => ({ left: 0, top: 0, width: 400, height: 34 });
  el.getContext = () => ({
    setTransform() {},
    clearRect() {},
    beginPath() {},
    moveTo() {},
    lineTo() {},
    arc() {},
    stroke() {},
    fill() {},
    save() {},
    restore() {},
    translate() {},
    scale() {},
    setLineDash() {},
    strokeStyle: "",
    fillStyle: "",
    lineWidth: 1,
    globalAlpha: 1,
  });
  return el;
}

async function runRoot(subjectKind) {
  const byId = new Map();
  const reg = (id, tag = "div") => {
    const n = tag === "canvas" ? makeTrackCanvas() : new MockNode(tag);
    n.attrs.id = id;
    byId.set(id, n);
    return n;
  };
  const stageWrap = new MockNode("main");
  const stage = reg("stage", "svg");
  stageWrap.append(stage);
  for (const id of ["ground-name", "data-note", "statement", "byline", "verdict", "story", "replay", "asof", "asof-label", "to-loom"]) {
    reg(id);
  }
  reg("track-canvas", "canvas");
  const nav = new MockNode("nav");

  const agentKey = hexId(1001);
  const subId = hexId(42);
  const evId = hexId(43);
  const t0 = 1793000000000;
  const ground = {
    ground: hexId(999),
    name: "root-check",
    now_ms: t0 + 60000,
    contributions: 2,
    heads: [evId],
    cursor: evId,
    policy: { min_passes: 1, validators: [] },
    agents: [{ agent: agentKey, name: "builder", contributions: 2, first_ms: t0, last_ms: t0 + 10000 }],
  };
  const rootResp = {
    now_ms: t0 + 60000,
    claim: {
      id: subId,
      kind: subjectKind,
      author: agentKey,
      author_name: "builder",
      timestamp_ms: t0,
      retracted: false,
      body: subjectKind === "artifact"
        ? { name: "docs/vision.md", media_type: "text/markdown", size: 1200, text: "# Vision" }
        : { statement: "A test claim", tags: [] },
      assessment: { id: subId, status: "accepted", passes: [{ id: evId }], fails: [], superseded_by: [] },
    },
    intents: [],
    implementations: [],
    evidence: [
      { id: evId, author: agentKey, author_name: "builder", timestamp_ms: t0 + 10000, verdict: "pass", method: "check", counted: true, retracted: false },
    ],
    refinements: [],
    rivals: [],
    lineage: { replaces: [], replaced_by: [] },
  };

  const sandbox = {
    console, Math, Number, String, Array, Object, Set, Map, RegExp, Date, Intl, JSON, Promise, Error,
    encodeURIComponent,
    Node: MockNode,
    Path2D: class Path2D { constructor(d) { this._d = d; } },
    CustomEvent: class CustomEvent { constructor(type, init) { this.type = type; this.detail = init?.detail; } },
    setTimeout: () => 0,
    clearTimeout: () => {},
    localStorage: { getItem: () => null, setItem: () => {} },
    getComputedStyle: () => ({ getPropertyValue: (name) => `var(${name})` }),
    EventSource: class EventSource { addEventListener() {} close() {} },
    location: { hash: `#${subId}`, assign() {} },
    fetch: async (url) => {
      if (url === "/api/ground") return { ok: true, status: 200, json: async () => ground };
      if (url === `/api/root/${subId}`) return { ok: true, status: 200, json: async () => rootResp };
      return { ok: false, status: 404, json: async () => ({ error: "not found" }) };
    },
    document: {
      documentElement: new MockNode("html"),
      getElementById: (id) => byId.get(id) || null,
      querySelector: (sel) => (sel === ".masthead nav" ? nav : null),
      createElement: (tag) => new MockNode(tag),
      createElementNS: (_ns, tag) => new MockNode(tag),
      createTextNode: (s) => MockNode.text(s),
      addEventListener() {},
      dispatchEvent() {},
    },
    window: {
      devicePixelRatio: 1,
      matchMedia: () => ({ matches: true }),
      addEventListener() {},
    },
  };
  sandbox.globalThis = sandbox;
  const ctx = vm.createContext(sandbox);
  vm.runInContext(COMMON_SRC, ctx, { filename: "common.js" });
  vm.runInContext(ROOT_SRC, ctx, { filename: "root.js" });
  for (let k = 0; k < 20; k++) await new Promise((r) => setImmediate(r));
  return { byId, Bracken: vm.runInContext("Bracken", ctx) };
}

async function main() {
  // 1. Artifact in Root view: central knot is artifact nodule, story says "added the artifact" + "on the artifact",
  // and "nothing implements it yet" is omitted.
  const art = await runRoot("artifact");
  const artStage = art.byId.get("stage");
  const artStory = art.byId.get("story");
  const artShapes = [...artStage.walk()].filter((n) => n.tag === "path" && n.attrs.d).map((n) => n.attrs.d);
  assert(artShapes.includes(art.Bracken.SHAPES.artifact), "central knot in Root view must use artifact nodule shape");
  assert(!artShapes.includes(art.Bracken.SHAPES.claim), "central knot in Root view for an artifact must not use claim seed shape");
  assert(!artStage.textContent.includes("nothing implements it yet"), `artifact Root view must omit 'nothing implements it yet', got: ${artStage.textContent}`);
  const firstArtLi = artStory.children[0];
  assert(firstArtLi.textContent.includes("added the artifact"), `expected initial artifact story entry 'added the artifact', got: ${firstArtLi.textContent}`);
  const firstArtPaths = [...firstArtLi.walk()].filter((n) => n.tag === "path" && n.attrs.d).map((n) => n.attrs.d);
  assert(firstArtPaths.includes(art.Bracken.SHAPES.artifact), "initial story icon for artifact must use artifact nodule shape");
  assert(artStory.textContent.includes("on the artifact"), `expected evidence story entry 'on the artifact', got: ${artStory.textContent}`);

  // 2. Claim in Root view (regression guard): central knot is claim seed, story says "wrote it" + "on the claim",
  // and "nothing implements it yet" still appears when implementations is empty.
  const cl = await runRoot("claim");
  const clStage = cl.byId.get("stage");
  const clStory = cl.byId.get("story");
  const clShapes = [...clStage.walk()].filter((n) => n.tag === "path" && n.attrs.d).map((n) => n.attrs.d);
  assert(clShapes.includes(cl.Bracken.SHAPES.claim), "central knot in Root view for a claim must use claim seed shape");
  assert(clStage.textContent.includes("nothing implements it yet"), "claim Root view with no implementations must still show 'nothing implements it yet'");
  assert(clStory.children[0].textContent.includes("wrote it"), "claim initial story entry must stay 'wrote it'");
  assert(clStory.textContent.includes("on the claim"), "claim evidence story entry must stay 'on the claim'");

  console.log("OK: Root view renders artifacts with nodule shape, artifact story wording, and no 'nothing implements it yet' (claims unchanged).");
}

main().catch((err) => {
  console.error("FAIL:", err.message);
  process.exit(1);
});
