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
const HOSTILE = "</script><img src=x onerror=alert(1)> & \u001b[2K\u202etxt.exe \"quoted\"";

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
  assert.ok(!page.textContent.includes("\u202e"), "no raw bidi control reaches the page");
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

check("a goal whose binding is unknown says so, not that it doesn't bind (round 2, note 2)", async () => {
  const goal = { goal: id(80), link: id(81), statement: "keep it fast", author: id(9), author_name: "alice", linker: id(9), linker_name: "alice", binds: null };
  const { page } = await load(`#line/${LINE}`, answers([
    [`/api/weave/line/${LINE}`, lineAnswer({ goals: list([goal]) })],
    [`/api/weave/log/${LINE}`, { list: "decisions", ...list([]) }],
  ]));
  assert.ok(page.textContent.includes("binding unknown"), page.textContent);
  assert.ok(!page.textContent.includes("doesn't bind"));
});

check("a diverged policy's current versions page past the first 50 (round 2, note 4)", async () => {
  const versions = Array.from({ length: 50 }, (_, i) => policy(id(100 + i), "main"));
  const { page, fetched } = await load(`#line/${LINE}`, answers([
    [`/api/weave/line/${LINE}?list=current`, { list: "current", ...list([policy(id(150), "main")]) }],
    [`/api/weave/line/${LINE}`, lineAnswer({ diverged: true, current: list(versions, 51), policy: { checks: null, integrators: null, validators: null, goal_setters: null, protected: null } })],
    [`/api/weave/log/${LINE}`, { list: "decisions", ...list([]) }],
  ]));
  const more = buttons(page).find((b) => b.textContent === "More (51 in all)");
  assert.ok(more, "a More button for the current versions");
  await more.on.click[0]();
  assert.ok(fetched.some((p) => p.includes("list=current") && p.includes(`after=${id(149)}`)), fetched.join("\n"));
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

// ---- Trees and files (PR (c)) ----

const MANIFEST = `blake3:${id(200)}`;
const fileRow = (name, extra = {}) => ({ name, dir: false, path: name, mode: 0o100644, symlink: false, size: 12, hash: `blake3:${id(201)}`, held: true, ...extra });
const treeAnswer = (children, extra = {}) => ({ manifest: MANIFEST, dir: "", files: children.length, complete: true, children: list(children), missing: list([]), ...extra });
const fileAnswer = (path, extra) => ({ manifest: MANIFEST, path, mode: 0o100644, symlink: false, size: 12, hash: `blake3:${id(201)}`, ...extra });
const rows = (page) => [...page.walk()].filter((e) => e.tagName === "UL" && e.className.includes("source")).flatMap((ul) => ul.children);

check("a decision's tree lists directories then files; a path goes out encoded and comes back as text", async () => {
  const odd = "odd dir/x‮y & ?#.txt";
  const { page, fetched, location } = await load(`#tree/${D2}/`, answers([
    [`/api/weave/tree/${D2}?dir=odd%20dir`, treeAnswer([fileRow("x‮y & ?#.txt", { path: odd })], { dir: "odd dir" })],
    [`/api/weave/file/${D2}?path=${encodeURIComponent(odd)}`, fileAnswer(odd, { shown: "text", text: "one\n", lines: 1 })],
    [`/api/weave/tree/${D2}`, treeAnswer([{ name: "odd dir", dir: true, files: 1 }, fileRow("README.md")])],
  ]));
  assert.ok(page.textContent.includes("odd dir/") && page.textContent.includes("README.md"), page.textContent);
  const dir = buttons(page).find((b) => b.textContent === "odd dir/");
  dir.on.click[0]();
  assert.strictEqual(location.hash, `#tree/${D2}/odd%20dir`);
  await new Promise((r) => setTimeout(r, 20));
  assert.ok(fetched.includes(`/api/weave/tree/${D2}?dir=odd%20dir`), fetched.join("\n"));
  assert.ok(marks(page).includes("U+202E"), `the file's name is marked: ${marks(page)}`);
  const f = buttons(page).find((b) => b.textContent.includes(".txt"));
  f.on.click[0]();
  assert.strictEqual(location.hash, `#file/${D2}/${encodeURIComponent(odd)}`);
  await new Promise((r) => setTimeout(r, 20));
  assert.ok(fetched.includes(`/api/weave/file/${D2}?path=${encodeURIComponent(odd)}`), fetched.join("\n"));
  safe(page, "tree");
});

check("a file's text is numbered rows of text: markup is text, and invisibles are marked", async () => {
  const text = "<script>alert(1)</script>\n‮reversed\n\tindented\n";
  const { page } = await load(`#file/${D2}/page.html`, answers([[`/api/weave/file/${D2}?path=page.html`, fileAnswer("page.html", { shown: "text", text, lines: 3 })]]));
  const r = rows(page);
  assert.strictEqual(r.length, 3, "3 lines, the final newline ends the last");
  assert.strictEqual(r[0].children[1].textContent, "<script>alert(1)</script>");
  assert.strictEqual(r[2].children[0].textContent, "3");
  assert.ok(marks(page).includes("U+202E"));
  safe(page, "file");
});

check("a megabyte of newlines renders 5,000 lines at a time, from the text already held (finding f3ec9bbabd28)", async () => {
  const text = "\n".repeat(1 << 20);
  const { page, fetched } = await load(`#file/${D2}/nl.txt`, answers([[`/api/weave/file/${D2}?path=nl.txt`, fileAnswer("nl.txt", { size: 1 << 20, shown: "text", text, lines: 1 << 20 })]]));
  assert.strictEqual(rows(page).length, 5000);
  const more = buttons(page).find((b) => b.textContent.startsWith("Show 5000 more lines"));
  assert.ok(more, "a button for the next 5,000");
  more.on.click[0]();
  assert.strictEqual(rows(page).length, 10000);
  assert.strictEqual(fetched.length, 1, "more lines need no new request");
  const elements = [...page.walk()].length;
  assert.ok(elements < 30100, `the page holds only what it shows: ${elements} elements`);
});

check("files that can't be shown are named with why, never shown", async () => {
  for (const [shownAs, extra, says] of [
    ["binary", {}, "isn't text"],
    ["binary", { symlink: true, mode: 0o120000 }, "Its target isn't text"],
    ["oversized", { size: 2 << 20 }, "1 MiB cap"],
    ["missing", {}, "doesn't hold this file"],
  ]) {
    const { page } = await load(`#file/${D2}/f`, answers([[`/api/weave/file/${D2}?path=f`, fileAnswer("f", { shown: shownAs, ...extra })]]));
    assert.ok(page.textContent.includes(says), `${shownAs}: ${page.textContent}`);
    assert.strictEqual(rows(page).length, 0);
  }
  const { page } = await load(`#file/${D2}/link`, answers([[`/api/weave/file/${D2}?path=link`, fileAnswer("link", { symlink: true, mode: 0o120000, shown: "symlink", text: "src/lib.rs" })]]));
  assert.ok(page.textContent.includes("Symlink to") && page.textContent.includes("src/lib.rs"));
});

check("a tree route with a bad record or a path that doesn't decode is the home page, unfetched", async () => {
  for (const hash of [`#tree/../../etc/x`, `#file/${D2}/%E0%A4%A`, `#tree/blake3:zz/`]) {
    const { fetched } = await load(hash, answers([["/api/weave", home()]]));
    assert.deepStrictEqual(fetched, ["/api/weave"], `${hash}: ${fetched}`);
  }
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
