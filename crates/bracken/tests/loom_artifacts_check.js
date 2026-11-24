#!/usr/bin/env node
// Deterministic check for Loom artifact rendering, hit-testing, detail panel, legend filter, and jump.
// Runs crates/bracken/assets/view/{common,loom}.js in Node's vm module with a minimal
// mock DOM and recording 2D canvas context — no browser or external dependency.
// Note: run by hand (not by scripts/test-table, since Node is not in the test table or boat box).
//
// Run: node crates/bracken/tests/loom_artifacts_check.js
"use strict";

const assert = require("assert");
const fs = require("fs");
const path = require("path");
const vm = require("vm");

const VIEW_DIR = path.resolve(__dirname, "../assets/view");
const COMMON_SRC = fs.readFileSync(path.join(VIEW_DIR, "common.js"), "utf8");
const LOOM_SRC = fs.readFileSync(path.join(VIEW_DIR, "loom.js"), "utf8");

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
    this.style = {
      setProperty: (k, v) => { this.style[k] = v; },
    };
    this.hidden = false;
    this.disabled = false;
    this.className = "";
    this.parentElement = null;
    this._text = "";
  }
  get firstChild() { return this.children[0] || null; }
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
  addEventListener(ev, fn) {
    (this.listeners[ev] ||= []).push(fn);
  }
  dispatch(ev, event = {}) {
    for (const fn of this.listeners[ev] || []) fn(event);
  }
  querySelector(sel) {
    const cls = sel.startsWith(".") ? sel.slice(1) : null;
    const walk = (n) => {
      for (const c of n.children) {
        if (cls && c.className.split(/\s+/).includes(cls)) return c;
        if (!cls && c.tag === sel) return c;
        const found = walk(c);
        if (found) return found;
      }
      return null;
    };
    return walk(this);
  }
  querySelectorAll(sel) {
    const out = [];
    const cls = sel.startsWith(".") ? sel.slice(1) : null;
    const walk = (n) => {
      for (const c of n.children) {
        if (cls && c.className.split(/\s+/).includes(cls)) out.push(c);
        else if (!cls && c.tag === sel) out.push(c);
        walk(c);
      }
    };
    walk(this);
    return out;
  }
  getBoundingClientRect() {
    if (this.attrs.id === "loom-canvas") return { left: 0, top: 0, width: 1000, height: 760 };
    if (this.attrs.id === "gauge-canvas") return { left: 0, top: 0, width: 92, height: 760 };
    return { left: 0, top: 0, width: 48, height: 18 };
  }
  static text(s) {
    const n = new MockNode("#text");
    n._text = String(s);
    return n;
  }
}

function makeCanvas(id) {
  const el = new MockNode("canvas");
  el.attrs.id = id;
  const ops = [];
  const stack = [];
  let tx = 0, ty = 0, sx = 1, sy = 1;
  const ctx = {
    fillStyle: "",
    strokeStyle: "",
    lineWidth: 1,
    lineCap: "butt",
    globalAlpha: 1,
    font: "",
    textBaseline: "",
    setTransform() { tx = 0; ty = 0; sx = 1; sy = 1; },
    clearRect() { ops.length = 0; },
    save() { stack.push({ tx, ty, sx, sy }); },
    restore() { ({ tx, ty, sx, sy } = stack.pop() || { tx: 0, ty: 0, sx: 1, sy: 1 }); },
    translate(x, y) { tx += x * sx; ty += y * sy; },
    scale(x, y) { sx *= x; sy *= y; },
    beginPath() {},
    moveTo() {},
    lineTo() {},
    quadraticCurveTo() { ops.push({ op: "quad" }); },
    closePath() { ops.push({ op: "close" }); },
    bezierCurveTo() {},
    arc(x, y, r) { ops.push({ op: "arc", x: tx + x * sx, y: ty + y * sy, r: r * sx }); },
    setLineDash() {},
    fill(p) {
      if (p && p._d) ops.push({ op: "fillPath", d: p._d, x: tx, y: ty, r: sx, fillStyle: this.fillStyle });
      else ops.push({ op: "fill", fillStyle: this.fillStyle });
    },
    stroke(p) {
      if (p && p._d) ops.push({ op: "strokePath", d: p._d, x: tx, y: ty, r: sx, strokeStyle: this.strokeStyle });
      else ops.push({ op: "stroke", strokeStyle: this.strokeStyle });
    },
    fillRect(x, y, w, h) { ops.push({ op: "fillRect", x: tx + x, y: ty + y, w, h, fillStyle: this.fillStyle }); },
    strokeRect() {},
    fillText() {},
  };
  el.getContext = () => ctx;
  el._ops = ops;
  return el;
}

async function runLoom(fixture, hash = "") {
  const byId = new Map();
  const reg = (id, tag = "div") => {
    const n = tag === "canvas" ? makeCanvas(id) : new MockNode(tag);
    n.attrs.id = id;
    byId.set(id, n);
    return n;
  };
  const gaugeParent = new MockNode("section");
  const gaugeCanvas = reg("gauge-canvas", "canvas");
  gaugeParent.append(gaugeCanvas);

  const loomCanvas = reg("loom-canvas", "canvas");
  for (const id of [
    "ground-name", "data-note", "agents", "labels", "tether", "hover-label", "side",
    "window", "surface", "surface-time", "flag", "to-flag", "play", "speed",
    "axis-time", "axis-causal", "key",
  ]) reg(id);
  const nav = new MockNode("nav");
  const docListeners = {};
  const winListeners = {};
  const navigated = [];

  const sandbox = {
    console,
    Math,
    Number,
    String,
    Array,
    Object,
    Set,
    Map,
    Float32Array,
    Int32Array,
    Uint8Array,
    RegExp,
    Date,
    Intl,
    JSON,
    Promise,
    Error,
    encodeURIComponent,
    Node: MockNode,
    Path2D: class Path2D { constructor(d) { this._d = d; } },
    CustomEvent: class CustomEvent { constructor(type, init) { this.type = type; this.detail = init?.detail; } },
    performance: { now: () => 1000 },
    setTimeout: () => 0,
    requestAnimationFrame: (fn) => { fn(1016); return 1; },
    cancelAnimationFrame: () => {},
    localStorage: { getItem: () => null, setItem: () => {} },
    getComputedStyle: () => ({ getPropertyValue: (name) => `var(${name})` }),
    EventSource: class EventSource { addEventListener() {} close() {} },
    location: {
      hash,
      assign(url) { navigated.push(url); },
    },
    fetch: async (url) => {
      const data = fixture.route(url);
      if (data === undefined) return { ok: false, status: 404, json: async () => ({ error: "not found" }) };
      return { ok: true, status: 200, json: async () => data };
    },
    document: {
      documentElement: new MockNode("html"),
      visibilityState: "visible",
      fonts: { ready: Promise.resolve() },
      getElementById: (id) => byId.get(id) || null,
      querySelector: (sel) => (sel === ".masthead nav" ? nav : null),
      createElement: (tag) => new MockNode(tag),
      createElementNS: (_ns, tag) => new MockNode(tag),
      createTextNode: (s) => MockNode.text(s),
      addEventListener: (ev, fn) => (docListeners[ev] ||= []).push(fn),
      dispatchEvent: (ev) => { for (const fn of docListeners[ev.type] || []) fn(ev); },
    },
    window: {
      devicePixelRatio: 1,
      matchMedia: () => ({ matches: true }),
      addEventListener: (ev, fn) => (winListeners[ev] ||= []).push(fn),
    },
  };
  sandbox.globalThis = sandbox;
  const ctx = vm.createContext(sandbox);
  vm.runInContext(COMMON_SRC, ctx, { filename: "common.js" });
  vm.runInContext(LOOM_SRC, ctx, { filename: "loom.js" });

  // Flush the async startup microtasks.
  for (let k = 0; k < 20; k++) await new Promise((r) => setImmediate(r));
  return { byId, loomCanvas, navigated, Bracken: vm.runInContext("Bracken", ctx) };
}

function makeDenseFixture() {
  const N = 5050;
  const ids = [], kind = [], author = [], parents = [], time = [], depth = [], tags = [];
  const t0 = 1793000000000;
  // Four artifacts on author 0 at distinct vertical bands, each sharing its exact timestamp with claims.
  const artifacts = new Map([
    [100, { name: "docs/pending.md", media_type: "text/markdown", size: 16055, status: "pending" }],
    [1500, { name: "docs/accepted.md", media_type: "text/markdown", size: 2048, status: "accepted" }],
    [3000, { name: "docs/rejected.md", media_type: "text/plain", size: 512, status: "rejected" }],
    [4500, { name: "docs/retracted.md", media_type: "text/plain", size: 99, status: "retracted" }],
  ]);
  const statuses = {};
  const retracts = [];
  for (let i = 0; i < N; i++) {
    const id = hexId(i + 1);
    ids.push(id);
    const art = artifacts.get(i);
    // Keep claims around each artifact at the exact same timestamp on author 0 to test overdraw + hit preference.
    let ts = t0 + i * 10000;
    let a = i % 2;
    for (const artIdx of artifacts.keys()) {
      if (Math.abs(i - artIdx) <= 5) {
        ts = t0 + artIdx * 10000;
        a = 0;
      }
    }
    if (art) {
      kind.push("artifact");
      if (art.status === "accepted") statuses[id] = { status: "accepted", passes: 1, fails: 0, superseded_by: [] };
      else if (art.status === "rejected") statuses[id] = { status: "rejected", passes: 0, fails: 1, superseded_by: [] };
    } else if (i === 4501) {
      kind.push("retract");
      retracts.push([i, 4500]);
    } else {
      kind.push("claim");
      if (i % 10 === 0) statuses[id] = { status: "accepted", passes: 1, fails: 0, superseded_by: [] };
    }
    author.push(a);
    parents.push(i === 0 ? [] : [i - 1]);
    time.push(ts);
    depth.push(i);
    tags.push([]);
  }
  const graph = { cursor: ids[N - 1], more: false, ids, kind, author, parents, time, depth, tags, refs: [], retracted: [4500], failed: [], retracts };
  const ground = {
    ground: hexId(99999),
    name: "dense-check",
    now_ms: t0 + N * 10000,
    contributions: N,
    heads: [ids[N - 1]],
    cursor: ids[N - 1],
    policy: { min_passes: 1, validators: [] },
    agents: [
      { agent: hexId(10001), name: "manager", contributions: 2525, first_ms: t0, last_ms: t0 + N * 10000 },
      { agent: hexId(10002), name: "builder", contributions: 2525, first_ms: t0, last_ms: t0 + N * 10000 },
    ],
  };
  return {
    artifacts,
    ids,
    route(url) {
      if (url === "/api/ground") return ground;
      if (url === "/api/graph") return graph;
      if (url === "/api/status") return { policy: ground.policy, statuses, conflicts: [] };
      if (url.startsWith("/api/contribution/")) {
        const id = url.slice("/api/contribution/".length);
        const idx = ids.indexOf(id);
        if (idx < 0) return undefined;
        const art = artifacts.get(idx);
        if (art) {
          return {
            id,
            kind: "artifact",
            author: ground.agents[0].agent,
            author_name: "manager",
            parents: [ids[idx - 1]],
            timestamp_ms: time[idx],
            retracted: art.status === "retracted",
            body: { name: art.name, media_type: art.media_type, size: art.size, text: "sample" },
            assessment: {
              id,
              status: art.status,
              passes: art.status === "accepted" ? [hexId(88888)] : [],
              fails: art.status === "rejected" ? [hexId(88889)] : [],
              superseded_by: [],
            },
            referrers: [],
            intents: [],
          };
        }
        return {
          id,
          kind: kind[idx],
          author: ground.agents[author[idx]].agent,
          author_name: ground.agents[author[idx]].name,
          parents: idx ? [ids[idx - 1]] : [],
          timestamp_ms: time[idx],
          retracted: false,
          body: kind[idx] === "retract" ? { target: ids[4500], reason: "old" } : { statement: `claim ${idx}`, tags: [] },
          assessment: { id, status: "pending", passes: [], fails: [], superseded_by: [] },
          referrers: [],
          intents: [],
        };
      }
      return undefined;
    },
  };
}

function makeSmallOverlapFixture() {
  const t0 = 1793000000000;
  // Earlier artifact and later claim at identical timestamp and author.
  const ids = [hexId(1), hexId(2), hexId(3)];
  const graph = {
    cursor: ids[2],
    more: false,
    ids,
    kind: ["claim", "artifact", "claim"],
    author: [0, 0, 0],
    parents: [[], [0], [1]],
    time: [t0, t0 + 10000, t0 + 10000],
    depth: [0, 1, 2],
    tags: [[], [], []],
    refs: [],
    retracted: [],
    failed: [],
    retracts: [],
  };
  const ground = {
    ground: hexId(77777),
    name: "small-overlap",
    now_ms: t0 + 20000,
    contributions: 3,
    heads: [ids[2]],
    cursor: ids[2],
    policy: { min_passes: 1, validators: [] },
    agents: [{ agent: hexId(10001), name: "manager", contributions: 3, first_ms: t0, last_ms: t0 + 10000 }],
  };
  return {
    ids,
    route(url) {
      if (url === "/api/ground") return ground;
      if (url === "/api/graph") return graph;
      if (url === "/api/status") return { policy: ground.policy, statuses: {}, conflicts: [] };
      if (url.startsWith("/api/contributions")) return { contributions: [], cursor: ids[2], more: false };
      if (url.startsWith("/api/contribution/")) {
        const id = url.slice("/api/contribution/".length);
        const idx = ids.indexOf(id);
        return {
          id,
          kind: graph.kind[idx],
          author: ground.agents[0].agent,
          author_name: "manager",
          parents: idx ? [ids[idx - 1]] : [],
          timestamp_ms: graph.time[idx],
          retracted: false,
          body: graph.kind[idx] === "artifact"
            ? { name: "docs/vision.md", media_type: "text/markdown", size: 1234, text: "hi" }
            : { statement: `claim ${idx}`, tags: [] },
          assessment: { id, status: "pending", passes: [], fails: [], superseded_by: [] },
          referrers: [],
          intents: [],
        };
      }
      return undefined;
    },
  };
}

async function main() {
  // 1. Dense mode: all 4 artifact statuses drawn with artifact nodule shape, readable size, AFTER blooms.
  const dense = makeDenseFixture();
  const { byId, loomCanvas, navigated, Bracken } = await runLoom(dense);
  const ops = loomCanvas._ops;
  const artShape = Bracken.SHAPES.artifact;
  const bloomIndices = ops.map((o, idx) => (o.op === "fillRect" && o.w === 2 && o.h === 2 ? idx : -1)).filter((i) => i >= 0);
  const artDraws = ops
    .map((o, idx) => ({ ...o, idx }))
    .filter((o) => o.op === "strokePath" && o.d === artShape);

  assert(bloomIndices.length > 0, "expected settled claim blooms in dense mode");
  assert.strictEqual(
    artDraws.length,
    4,
    `expected all 4 artifacts (pending, accepted, rejected, retracted) drawn with artifact shape in dense mode, got ${artDraws.length}`
  );
  const lastBloom = bloomIndices[bloomIndices.length - 1];
  for (const d of artDraws) {
    assert(d.r >= 2.5, `artifact radius ${d.r} too small to read at default zoom`);
    assert(d.idx > lastBloom, `artifact drawn at op ${d.idx} before last bloom at op ${lastBloom}`);
  }

  // 2. Hit-testing prefers the artifact over the 10 claims at the exact same (x, y).
  const pendingDraw = artDraws[0];
  const win = byId.get("window");
  win.dispatch("pointermove", { clientX: pendingDraw.x, clientY: pendingDraw.y });
  for (let k = 0; k < 10; k++) await new Promise((r) => setImmediate(r));
  const hoverText = byId.get("hover-label").textContent;
  assert(
    hoverText.includes("artifact") && hoverText.includes("docs/pending.md"),
    `expected hover over overlapped artifact to pick artifact, got: ${JSON.stringify(hoverText)}`
  );

  // 3. Clicking the artifact opens its detail panel with name, media type, size, status, and Root view link.
  win.dispatch("click", {});
  for (let k = 0; k < 15; k++) await new Promise((r) => setImmediate(r));
  const side = byId.get("side");
  const sideText = side.textContent;
  assert(sideText.includes("artifact · pending"), `missing artifact status in side panel: ${sideText}`);
  assert(sideText.includes("docs/pending.md"), `missing artifact name in side panel: ${sideText}`);
  assert(sideText.includes("text/markdown"), `missing media type in side panel: ${sideText}`);
  assert(sideText.includes("16,055") || sideText.includes("15.7 KiB"), `missing artifact size in side panel: ${sideText}`);
  const rootBtn = side.querySelector(".open-root");
  assert(rootBtn, "missing Root view button in artifact detail panel");
  rootBtn.dispatch("click", {});
  assert.strictEqual(navigated[0], `/root.html#${dense.ids[100]}`, `unexpected Root link target: ${navigated[0]}`);

  // 4. Non-dense mode: artifact is drawn after a later claim at the same position.
  const small = makeSmallOverlapFixture();
  const s = await runLoom(small);
  const sOps = s.loomCanvas._ops;
  const claimIdx = sOps.findLastIndex((o) => o.op === "strokePath" && o.d === s.Bracken.SHAPES.claim);
  const artIdx = sOps.findLastIndex((o) => o.op === "strokePath" && o.d === s.Bracken.SHAPES.artifact);
  assert(artIdx > claimIdx, `non-dense artifact (op ${artIdx}) must be drawn after overlapping claim (op ${claimIdx})`);

  // 5. Dense mode with >400 artifacts exercises cell-bucketed dense artifact drawing after all blooms.
  const many = makeDenseFixture();
  const manyGraph = many.route("/api/graph");
  for (let i = 200; i < 700; i++) manyGraph.kind[i] = "artifact";
  const m = await runLoom(many);
  const mOps = m.loomCanvas._ops;
  const mLastBloom = mOps.findLastIndex((o) => o.op === "fillRect" && o.w === 2 && o.h === 2);
  const mArtsAfter = mOps.slice(mLastBloom + 1).filter((o) => o.op === "strokePath" && o.d === m.Bracken.SHAPES.artifact).length;
  assert(mArtsAfter > 0, "expected bucketed artifact nodules drawn after blooms when >400 artifacts exist");

  // 6. Interactive legend filter: artifact count badge, toggle filter on/off, stitched labels, hit filtering.
  const keyEl = byId.get("key");
  const keyBtns = keyEl.querySelectorAll(".key-btn");
  assert.strictEqual(keyBtns.length, 8, `expected 8 interactive legend filter buttons, got ${keyBtns.length}`);
  const artBtn = keyBtns.find((b) => b.textContent.includes("artifact"));
  assert(artBtn, "missing artifact filter button in key");
  assert(artBtn.textContent.includes("4"), `expected artifact count badge 4, got: ${artBtn.textContent}`);

  // Toggle artifact filter ON -> aria-pressed=true, data-note announces filter, labels render for the 4 artifacts.
  artBtn.dispatch("click", {});
  for (let k = 0; k < 15; k++) await new Promise((r) => setImmediate(r));
  assert.strictEqual(artBtn.getAttribute("aria-pressed"), "true", "artifact filter button should be pressed");
  assert(byId.get("data-note").textContent.includes("showing artifacts"), `expected filter in data-note: ${byId.get("data-note").textContent}`);
  assert.strictEqual(byId.get("labels").querySelectorAll(".knot-label").length, 4, "filtering to 4 artifacts should show their stitched labels even on a dense Ground");

  // Toggle artifact filter OFF -> aria-pressed=false, labels clear again on dense Ground.
  artBtn.dispatch("click", {});
  for (let k = 0; k < 10; k++) await new Promise((r) => setImmediate(r));
  assert.strictEqual(artBtn.getAttribute("aria-pressed"), "false", "artifact filter button should unpress");
  assert.strictEqual(byId.get("labels").querySelectorAll(".knot-label").length, 0, "unfiltered dense Ground should hide inline labels");

  // 7. Artifact prev/next jump buttons cycle through artifacts and open each specimen card.
  const jumpBtns = keyEl.querySelectorAll(".key-jump");
  assert.strictEqual(jumpBtns.length, 2, `expected prev/next artifact jump buttons, got ${jumpBtns.length}`);
  const [prevBtn, nextBtn] = jumpBtns;
  assert.strictEqual(prevBtn.disabled, false, "prev artifact button should be enabled when artifacts exist");
  assert.strictEqual(nextBtn.disabled, false, "next artifact button should be enabled when artifacts exist");
  // Currently selected is idx 100 (docs/pending.md); clicking Next should jump to idx 1500 (docs/accepted.md).
  nextBtn.dispatch("click", {});
  for (let k = 0; k < 15; k++) await new Promise((r) => setImmediate(r));
  assert(byId.get("side").textContent.includes("docs/accepted.md"), `expected Next jump to open docs/accepted.md, got: ${byId.get("side").textContent}`);
  prevBtn.dispatch("click", {});
  for (let k = 0; k < 15; k++) await new Promise((r) => setImmediate(r));
  assert(byId.get("side").textContent.includes("docs/pending.md"), `expected Prev jump to return to docs/pending.md, got: ${byId.get("side").textContent}`);

  console.log("OK: dense artifact draw, bucketed >400 draw, z-order, hit preference, detail panel, legend filter, and artifact jump all pass.");
}

main().catch((err) => {
  console.error("FAIL:", err.message);
  process.exit(1);
});
