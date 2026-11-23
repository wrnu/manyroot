#!/usr/bin/env node
// Deterministic check of the Weave pages (docs/design/weave-view.md section 8): runs
// crates/bracken/assets/view/{common,weave}.js in Node's vm module with a minimal mock DOM and a mock
// fetch that answers from fixed /api/weave answers, with no browser, server or dependency.
//
// Run: node crates/bracken/tests/weave_view_check.js
"use strict";

const assert = require("assert");
const fs = require("fs");
const path = require("path");
const vm = require("vm");

const VIEW_DIR = path.resolve(__dirname, "../assets/view");
const SOURCES = ["common.js", "weave.js"].map((f) => [f, fs.readFileSync(path.join(VIEW_DIR, f), "utf8")]);

// A distinct 64-hex id whose short form (its first 12 digits) is distinct too.
const id = (n) => n.toString(16).padStart(4, "0").repeat(16);
const HOSTILE = "</script><img src=x onerror=alert(1)> & \u001b[2K‮txt.exe \"quoted\"";

// ---- A minimal DOM ----

class Node {}
class Text extends Node {
  constructor(t) { super(); this.data = String(t); }
  get textContent() { return this.data; }
}
class El extends Node {
  constructor(tag) { super(); this.tagName = tag.toUpperCase(); this.children = []; this.attrs = {}; this.on = {}; this.className = ""; this.disabled = false; }
  append(...cs) { for (const c of cs) this.children.push(c instanceof Node ? c : new Text(c)); }
  appendChild(c) { this.append(c); return c; }
  replaceChildren(...cs) { this.children = []; this.append(...cs); }
  replaceWith(...cs) { this.replacedWith = cs; }
  remove() { this.removed = true; }
  setAttribute(k, v) { this.attrs[k] = String(v); }
  getAttribute(k) { return this.attrs[k] ?? null; }
  removeAttribute(k) { delete this.attrs[k]; }
  addEventListener(ev, fn) { (this.on[ev] ||= []).push(fn); }
  get textContent() { return this.children.map((c) => c.textContent).join(""); }
  set textContent(t) { this.children = []; if (t !== "") this.append(new Text(t)); }
  set innerHTML(_) { throw new Error("innerHTML is never set"); }
  get classList() {
    const el = this;
    return { add(c) { el.className = `${el.className} ${c}`.trim(); }, remove(c) { el.className = el.className.split(" ").filter((x) => x !== c).join(" "); }, toggle() {}, contains(c) { return el.className.split(" ").includes(c); } };
  }
  get style() { return {}; }
  *walk() { yield this; for (const c of this.children) if (c instanceof El) yield* c.walk(); }
}

// ---- One page load, against `answer(path)` ----

/**
 * Loads the scripts with location.hash `hash`; `answer(path)` gives [status, body, headers] for each fetch.
 * Resolves when the page has rendered, with the page element and what was fetched.
 */
async function load(hash, answer) {
  const ids = { "data-note": new El("span"), page: new El("main") };
  const listeners = {};
  const fetched = [];
  const errors = [];
  const location = {
    _hash: hash,
    get hash() { return this._hash; },
    set hash(v) { this._hash = v.startsWith("#") ? v : `#${v}`; (listeners.hashchange || []).forEach((f) => f()); },
  };
  const ctx = vm.createContext({
    window: { matchMedia: () => ({ matches: true, addEventListener() {} }), addEventListener: (ev, fn) => (listeners[ev] ||= []).push(fn), scrollTo() {}, location },
    document: {
      documentElement: new El("html"),
      createElement: (t) => new El(t),
      createElementNS: (_, t) => new El(t),
      createTextNode: (t) => new Text(t),
      getElementById: (i) => ids[i] ?? null,
      addEventListener() {},
      dispatchEvent() {},
      querySelector: () => null,
      querySelectorAll: () => [],
    },
    location,
    Node,
    console: { ...console, error: (...a) => errors.push(a.join(" ")) },
    fetch: async (p) => {
      fetched.push(p);
      const [status, body, headers = {}] = answer(p);
      return { ok: status === 200, status, headers: { get: (k) => headers[k] ?? null }, json: async () => JSON.parse(JSON.stringify(body)) };
    },
    URLSearchParams, encodeURIComponent, setTimeout, clearTimeout, queueMicrotask, Date, Array, String, Number, Object, Math, RegExp, JSON, Promise, Error,
  });
  ctx.globalThis = ctx;
  for (const [file, src] of SOURCES) new vm.Script(src, { filename: file }).runInContext(ctx);
  const page = ids.page;
  const start = Date.now();
  while (!page.children.length) {
    if (Date.now() - start > 20000) throw new Error(`never rendered ${hash}`);
    await new Promise((r) => setTimeout(r, 5));
  }
  assert.deepStrictEqual(errors, [], `${hash}: script errors`);
  return { page, note: ids["data-note"], fetched, location };
}

/** Every element: never a link, script or image; every attribute set through h(). */
function safe(page, name) {
  for (const el of page.walk()) {
    assert.strictEqual(el.attrs.href, undefined, `${name}: an href`);
    assert.ok(!["A", "SCRIPT", "IMG", "IFRAME"].includes(el.tagName), `${name}: a <${el.tagName}>`);
  }
}
const marks = (page) => [...page.walk()].filter((e) => e.className.includes("invisible")).map((e) => e.textContent);
const buttons = (page) => [...page.walk()].filter((e) => e.tagName === "BUTTON");

// ---- Fixtures, shaped as the API answers ----

const LINE = id(1);
const D1 = id(2);
const D2 = id(3);
const V1 = id(4);
const list = (items, total = items.length) => ({ items, total, more: total > items.length });
const policy = (version, name) => ({
  version, author: id(9), author_name: "alice", name,
  integrators: [{ agent: id(9), name: "alice" }], integrators_total: 1,
  validators: [], validators_total: 0, goal_setters: [], goal_setters_total: 0,
  checks: [{ check: "test", argv: ["cargo", "test"], argv_total: 2, method: "cargo test", executor: null }], checks_total: 1,
  guardrails: { max_known: 3, max_age: 5, protected: [], protected_total: 0 }, owner_decision: id(10),
});
const head = (d, checks) => ({ decision: d, checks });
const counted = (check, extra = {}) => ({ check, method: check, green: true, passes: [{ result: id(20), author: id(9), author_name: "val", attempt: null, passed: true }], passes_total: 1, fails: [], fails_total: 0, replaced: [], replaced_total: 0, ...extra });
const decisionRow = (d, note, extra = {}) => ({
  decision: d, author: id(9), author_name: "alice", line: LINE, manifest: `blake3:${id(30)}`,
  parents: [], parents_total: 0, proposals: [], proposals_total: 0, resolutions: [], resolutions_total: 0,
  goals: [], goals_total: 0, answers: [], answers_total: 0, breakage: [], breakage_total: 0, note, source: null, ...extra,
});
const home = (extra = {}) => ({
  weave: true, marker: id(40), root: id(9), root_name: "alice", marker_author: id(9), default_line: LINE,
  default: { line: LINE, name: "main", heads: [{ decision: D2, manifest: `blake3:${id(30)}` }], forked: false, green: { state: "one", heads: [] }, open_objections: 0 },
  lines: list([{ line: LINE, name: "main", author: id(9), author_name: "alice", default: true, reach: "name", decisions: 2 }]),
  problems: list([]), unreadable: list([]), ignored_versions: list([]),
  ...extra,
});
const lineAnswer = (extra = {}) => ({
  line: LINE, name: "main", author: id(9), author_name: "alice", default: true, decisions: 2, diverged: false,
  heads: [{ decision: D2, manifest: `blake3:${id(30)}` }], forked: false,
  green: { state: "one", heads: [head(D2, [counted("test")])] }, open_objections: 0,
  current: list([policy(V1, "main")]), versions: list([policy(V1, "main")]), goals: list([]),
  policy: { checks: list(policy(V1).checks), integrators: list(policy(V1).integrators), validators: list([]), goal_setters: list([]), protected: list([]) },
  ...extra,
});
const decisionAnswer = (extra = {}) => ({
  ...decisionRow(D2, HOSTILE), parents: [D1], verdict: "qualifies", line_name: "main", policy: V1,
  counted: list([counted("test")]), reviews: list([]), ...extra,
});
const answers = (table) => (p) => {
  for (const [prefix, reply] of table) if (p.startsWith(prefix)) return typeof reply === "function" ? reply(p) : [200, reply];
  return [404, { error: `no fixture for ${p}` }];
};

// ---- Checks ----

const checks = [];
const check = (name, fn) => checks.push([name, fn]);

check("home shows what weave check lists: problems, unreadable records and ignored versions (finding 48a5414cb94e)", async () => {
  const unreadable = { id: id(50), kind: "decision", line: LINE, why: "its payload doesn't decode" };
  const { page, note } = await load("", answers([["/api/weave", home({ unreadable: list([unreadable]), ignored_versions: list([id(51)]) })]]));
  const text = page.textContent;
  assert.ok(text.includes("What weave check would list"), text);
  assert.ok(text.includes("Records that can't be read") && text.includes("its payload doesn't decode"), text);
  assert.ok(text.includes("Ignored policy versions") && text.includes(id(51).slice(0, 12)), text);
  assert.strictEqual(note.textContent, "");
  safe(page, "home");
});

check("home with only ignored versions still lists them", async () => {
  const { page } = await load("", answers([["/api/weave", home({ ignored_versions: list([id(52)]) })]]));
  assert.ok(page.textContent.includes(id(52).slice(0, 12)));
});

check("a line's decision list marks a bidi control in a note (finding 2a8558fd62c8)", async () => {
  const { page } = await load(`#line/${LINE}`, answers([
    [`/api/weave/line/${LINE}`, lineAnswer()],
    [`/api/weave/log/${LINE}`, { list: "decisions", ...list([decisionRow(D2, HOSTILE), decisionRow(D1, "first")]) }],
  ]));
  const m = marks(page);
  assert.ok(m.includes("U+202E"), `marks: ${m}`);
  assert.ok(m.includes("\\u{1b}"), `marks: ${m}`);
  assert.ok(page.textContent.includes("</script><img src=x onerror=alert(1)>"), "the note is text");
  assert.ok(!page.textContent.includes("‮"), "no raw bidi control reaches the page");
  safe(page, "line");
});

check("a merge's count is its parents' total, not the 5 a row carries (independent note 3)", async () => {
  const parents = [id(60), id(61), id(62), id(63), id(64)];
  const { page } = await load(`#line/${LINE}`, answers([
    [`/api/weave/line/${LINE}`, lineAnswer()],
    [`/api/weave/log/${LINE}`, { list: "decisions", ...list([decisionRow(D2, "a merge", { parents, parents_total: 7 })]) }],
  ]));
  assert.ok(page.textContent.includes("merges 7"), page.textContent);
});

check("a forked line shows each head's checks; a diverged one each current version (independent notes 1 and 2)", async () => {
  const other = id(70);
  const { page } = await load(`#line/${LINE}`, answers([
    [`/api/weave/line/${LINE}`, lineAnswer({
      forked: true, diverged: true, heads: [{ decision: D2 }, { decision: other }],
      green: { state: "undefined", heads: [head(D2, null), head(other, null)] },
      current: list([policy(V1, "main"), policy(id(71), "main, the other")]), policy: { checks: null, integrators: null, validators: null, goal_setters: null, protected: null },
      open_objections: null,
    })],
    [`/api/weave/log/${LINE}`, { list: "decisions", ...list([]) }],
  ]));
  const text = page.textContent;
  assert.ok(text.includes(`Checks on head ${D2.slice(0, 12)}`) && text.includes(`Checks on head ${other.slice(0, 12)}`), text);
  assert.ok(text.includes(V1.slice(0, 12)) && text.includes(id(71).slice(0, 12)), text);
  assert.ok(text.includes("No current policy, so nothing is counted."), text);
});

check("a line's one current policy pages its checks with More, by check name", async () => {
  const many = Array.from({ length: 50 }, (_, i) => ({ check: `c${i}`, argv: ["run"], argv_total: 1, method: `c${i}`, executor: null }));
  const { page, fetched } = await load(`#line/${LINE}`, answers([
    [`/api/weave/line/${LINE}?list=checks`, { list: "checks", ...list([{ check: "c50", argv: ["run"], argv_total: 1, method: "c50", executor: null }]) }],
    [`/api/weave/line/${LINE}`, lineAnswer({ policy: { checks: list(many, 51), integrators: list([]), validators: list([]), goal_setters: list([]), protected: list([]) } })],
    [`/api/weave/log/${LINE}`, { list: "decisions", ...list([]) }],
  ]));
  const more = buttons(page).find((b) => b.textContent === "More (51 in all)");
  assert.ok(more, "a More button for the checks");
  await more.on.click[0]();
  assert.ok(fetched.some((p) => p.includes("list=checks") && p.includes("after=c49")), fetched.join("\n"));
  assert.ok(page.textContent.includes("c50"));
});

check("a decision with no current policy says nothing is counted (finding 08c43e704dff)", async () => {
  const { page } = await load(`#decision/${D2}`, answers([[`/api/weave/decision/${D2}`, decisionAnswer({ counted: null })]]));
  assert.ok(page.textContent.includes("No current policy, so nothing is counted."), page.textContent);
  assert.ok(!page.textContent.includes("The policy names no checks."));
});

check("a decision's counted checks page past the first 50 (independent note 4), and a shortened row says so", async () => {
  const first = Array.from({ length: 50 }, (_, i) => counted(`c${i}`, i === 0 ? { cut: true } : {}));
  const { page, fetched } = await load(`#decision/${D2}`, answers([
    [`/api/weave/decision/${D2}?list=counted`, { list: "counted", ...list([counted("c50")]) }],
    [`/api/weave/decision/${D2}`, decisionAnswer({ counted: list(first, 51) })],
  ]));
  assert.ok(page.textContent.includes("(shortened)"));
  const more = buttons(page).find((b) => b.textContent === "More (51 in all)");
  assert.ok(more, "a More button for counted");
  await more.on.click[0]();
  assert.ok(fetched.some((p) => p.includes("list=counted") && p.includes("after=c49")), fetched.join("\n"));
});

check("the decision page shows hostile text as text, with invisibles marked", async () => {
  const { page } = await load(`#decision/${D2}`, answers([[`/api/weave/decision/${D2}`, decisionAnswer()]]));
  assert.ok(page.textContent.includes("</script><img src=x onerror=alert(1)>"));
  assert.ok(marks(page).includes("U+202E"));
  safe(page, "decision");
});

check("while the view reads its Weave records the page waits, then shows them", async () => {
  let n = 0;
  const { page, fetched } = await load("", (p) =>
    ++n <= 2 ? [503, { error: "reading the Weave records: try again in a moment" }, { "Retry-After": "1" }] : [200, home()]);
  assert.ok(page.textContent.includes("Every line"), page.textContent);
  assert.strictEqual(fetched.length, 3);
});

check("a Weave state that failed says why at once, and the status note is cleared (independent note 5)", async () => {
  const why = "the replica changed and couldn't be read again, so the Weave pages are unavailable until it can: x";
  const { page, note, fetched } = await load("", () => [503, { error: why }, { "Retry-After": "2" }]);
  assert.ok(page.textContent.includes(why), page.textContent);
  assert.strictEqual(fetched.length, 1, "not retried");
  assert.strictEqual(note.textContent, "");
});

check("a replica without Weave says so and clears the status note", async () => {
  const { page, note } = await load("", answers([["/api/weave", { weave: false, why: "this replica has no Weave root (weave-root)" }]]));
  assert.ok(page.textContent.includes("No Weave records here"));
  assert.strictEqual(note.textContent, "");
});

(async () => {
  let failed = 0;
  for (const [name, fn] of checks) {
    try {
      await fn();
      console.log(`ok   ${name}`);
    } catch (e) {
      failed++;
      console.log(`FAIL ${name}\n     ${String(e && e.stack || e).split("\n").slice(0, 3).join("\n     ")}`);
    }
  }
  console.log(failed ? `${failed} of ${checks.length} failed` : `all ${checks.length} passed`);
  process.exit(failed ? 1 : 0);
})();
