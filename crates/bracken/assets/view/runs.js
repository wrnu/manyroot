// Runs (docs/design/visualizer.md section 3.6): one experiment run as a growth view. Each mode's shared
// library is a stem climbing with its correct functions, checkpoint by checkpoint (decision
// 288a16ce8cb2), and each wave of spec changes falls across every stem at once. Below, the ranking
// measures side by side, and apart from them the per-delivery diagnostics, which never compare
// across modes.
//
// Everything shown is /api/runs: each run's report.json, as `bracken-experiment report --json` wrote
// it, and its plan's waves. Values are formatted as that command's table formats them.
"use strict";

(() => {
  const B = Bracken;
  const $ = (id) => document.getElementById(id);
  const S = B.s;
  const ORDER = ["bracken", "git", "crdt"];

  let runs = [];

  async function load() {
    const r = await fetch("/api/runs", { headers: { Accept: "application/json" } });
    if (r.status === 404) return empty("This view was started without --runs. Start it with bracken view --runs <run directory>.");
    if (!r.ok) throw new Error(`${r.status}`);
    runs = (await r.json()).runs.slice().sort((a, b) => rank(a) - rank(b));
    B.text($("run-name"), runs.length === 1 ? runs[0].name : `${runs.length} modes`);
    const turns = Math.max(0, ...runs.map((x) => (cp(x) ? cp(x).turns.length : 0)));
    B.text($("data-note"), `${runs.map((x) => x.measures.mode).join(", ")} · ${turns} checkpoints · read once from report.json`);
    draw();
    cards();
  }
  const rank = (x) => (ORDER.includes(x.measures.mode) ? ORDER.indexOf(x.measures.mode) : ORDER.length);
  const cp = (x) => x.measures.checkpoints || null;
  // Frond for bracken (a fern), forget-me-not for git, heather for crdt: far apart, and none a status.
  const inkOf = (k) => B.ink([2, 4, 5][k] ?? 8);

  function empty(text) {
    B.text($("data-note"), "");
    $("field").replaceChildren();
    $("field").parentElement.parentElement.hidden = true;
    $("ranking").replaceChildren(B.h("p", { class: "empty" }, text));
    $("diagnostics").replaceChildren();
  }

  // ---- The growth view ----
  function draw() {
    const field = $("field");
    const w = field.clientWidth || 1100, h = field.clientHeight || 416;
    B.attr(field, "viewBox", `0 0 ${w} ${h}`);
    const withCp = runs.filter(cp);
    if (!withCp.length) {
      field.replaceChildren(S("text", { x: 24, y: 40 }, "No checkpoints: these runs weren't graded checkpoint by checkpoint (Phase 1)."));
      return;
    }
    const L = 56, R = 120, T = 36, Bm = 40;
    const lastTurn = Math.max(1, ...withCp.flatMap((x) => cp(x).turns.map((t) => t.turn)));
    const total = cp(withCp[0]).total;
    const most = Math.max(1, ...withCp.flatMap((x) => cp(x).turns.map((t) => t.correct)));
    const top = Math.max(4, Math.ceil(most * 1.25));
    const x = (turn) => L + ((turn - 0.5) / lastTurn) * (w - L - R);
    const y = (n) => h - Bm - (n / top) * (h - T - Bm);
    const g = [];
    // Grid and axes.
    const step = Math.max(1, Math.ceil(top / 5));
    for (let n = 0; n <= top; n += step) {
      g.push(S("path", { d: `M${L},${y(n)} L${w - R},${y(n)}`, class: "grid" }));
      g.push(S("text", { x: L - 10, y: y(n) + 4, "text-anchor": "end" }, String(n)));
    }
    for (let t = 1; t <= lastTurn; t++) g.push(S("text", { x: x(t), y: h - Bm + 20, "text-anchor": "middle" }, String(t)));
    g.push(S("text", { x: (L + w - R) / 2, y: h - 6, "text-anchor": "middle" }, "turn (each checkpoint is the end of a turn index)"));
    g.push(S("text", { x: 8, y: 18 }, `correct of ${total}`));
    // Waves: before the first checkpoint that counts one more released.
    const turns = cp(withCp[0]).turns;
    for (let k = 1; k < turns.length; k++) {
      if (turns[k].waves > turns[k - 1].waves) {
        const at = (x(turns[k - 1].turn) + x(turns[k].turn)) / 2;
        g.push(S("path", { d: `M${at},${T - 8} L${at},${h - Bm}`, class: "wave" }));
        g.push(S("text", { x: at + 5, y: T - 12, class: "wave-label" }, `wave ${turns[k].waves}`));
      }
    }
    // Stems, nudged apart a little so equal counts stay visible.
    const ends = [];
    withCp.forEach((run, k) => {
      const ink = inkOf(rank(run));
      const dx = (k - (withCp.length - 1) / 2) * 5;
      const pts = cp(run).turns.map((t) => [x(t.turn) + dx, y(t.correct), t]);
      const d = pts.map(([px, py], i) => `${i ? "L" : "M"}${px.toFixed(1)},${py.toFixed(1)}`).join(" ");
      g.push(S("path", { d, class: "stem", stroke: ink }));
      pts.forEach(([px, py, t], i) => {
        const fell = i > 0 && t.correct < pts[i - 1][2].correct;
        const leaf = S("path", { d: B.SHAPES.claim, transform: `translate(${px.toFixed(1)} ${py.toFixed(1)}) rotate(${i % 2 ? 40 : -40}) scale(5.5)`, fill: ink, stroke: "var(--ground-raised)", "stroke-width": 0.25, class: "leaf" });
        leaf.addEventListener("pointerenter", (ev) => tip(ev, run, t));
        leaf.addEventListener("pointerleave", () => tip(null));
        g.push(leaf);
        // A count that fell: a nick in the stem.
        if (fell) g.push(S("path", { d: `M${px - 5},${py - 9} L${px + 5},${py - 15}`, class: "broken" }));
      });
      ends.push({ y: pts[pts.length - 1][1], x: pts[pts.length - 1][0], run, ink });
    });
    // Each stem named at its end, pushed apart so names don't overlap.
    ends.sort((a, b) => a.y - b.y);
    for (let i = 1; i < ends.length; i++) ends[i].y = Math.max(ends[i].y, ends[i - 1].y + 17);
    for (const e of ends) g.push(S("text", { x: e.x + 14, y: e.y + 5, class: "mode-label", fill: e.ink }, `${e.run.measures.mode} ${cp(e.run).correct}/${cp(e.run).total}`));
    field.replaceChildren(...g);
  }

  function tip(ev, run, t) {
    const el = $("tip");
    if (!ev) {
      el.hidden = true;
      return;
    }
    el.replaceChildren(`${run.measures.mode}, turn ${t.turn}: ${t.correct} correct in the library, ${t.standalone} standalone · ${t.waves} wave${t.waves === 1 ? "" : "s"} released before it`);
    el.hidden = false;
    const box = el.parentElement.getBoundingClientRect();
    el.style.left = `${Math.min(box.width - 300, ev.clientX - box.left + 14)}px`;
    el.style.top = `${ev.clientY - box.top + 14}px`;
  }

  // ---- The measures, as `bracken-experiment report` formats them ----
  const f = (v) => (v === null || v === undefined ? "n/a" : v.toFixed(2));
  const pct = (v) => (v === null || v === undefined ? "n/a" : `${(v * 100).toFixed(1)}%`);
  const stage = (s) => `${s.reached} (${f(s.median_seconds)}s, ${f(s.median_turns)} turns)`;

  const RANKING = [
    ["final correct", (c) => `${c.correct}/${c.total}`],
    ["median turn to correct", (c) => f(c.median_turn_correct)],
    ["recovery per wave (checkpoints)", (c) => c.recovery.map((r) => (r.checkpoints === null ? "never" : String(r.checkpoints))).join(", ")],
    ["breakage: after a wave, other", (c) => `${c.breakage.after_wave}, ${c.breakage.other}`],
    ["tokens per correct function", (c) => f(c.tokens_per_correct)],
  ];
  const DIAGNOSTICS = [
    ["Stopped early by", (m) => m.stopped || "-"],
    ["Tokens (output)", (m) => `${m.tokens} (${m.output_tokens})`],
    ["Graded snapshots", (m) => String(m.snapshots)],
    ["Improvements: net (+, -)", (m) => `${m.improvements.net} (+${m.improvements.gained}, -${m.improvements.lost})`],
    ["per million tokens", (m) => f(m.improvements.per_million_tokens)],
    ["per million output tokens", (m) => f(m.improvements.per_million_output)],
    ["Deliveries", (m) => String(m.duplication.deliveries)],
    ["Duplicated: visible, blind", (m) => `${m.duplication.visible}, ${m.duplication.blind}`],
    ["share of output tokens", (m) => pct(m.duplication.output_share)],
    ["Overhead tokens: integrate, validate", (m) => `${pct(m.overhead.integrate_tokens)}, ${pct(m.overhead.validate_tokens)}`],
    ["Overhead time: integrate, validate", (m) => `${pct(m.overhead.integrate_time)}, ${pct(m.overhead.validate_time)}`],
    ["Failed integrations", (m) => String(m.overhead.failed_integrations)],
    ["Useful discoveries (bug reports)", (m) => `${m.discovery.discoveries} (${m.discovery.bug_reports})`],
    ["available", (m) => stage(m.discovery.available)],
    ["integrated", (m) => stage(m.discovery.integrated)],
    ["taken up", (m) => stage(m.discovery.taken_up)],
    ["Broken in library (changes)", (m) => `${m.incompatible.broken_in_library} (${m.incompatible.changes})`],
    ["Modules broken", (m) => String(m.incompatible.unparsable)],
    ["Semantic-conflict events", (m) => (m.incompatible.semantic_conflicts === null || m.incompatible.semantic_conflicts === undefined ? "n/a" : String(m.incompatible.semantic_conflicts))],
    ["Final: correct, wrong, missing", (m) => `${m.quality.correct}/${m.quality.total}, ${m.quality.wrong}, ${m.quality.missing}`],
  ];

  function cards() {
    const card = (run, rows, value) => {
      const el = B.h(
        "article",
        { class: "mode" },
        B.h("h3", {}, run.measures.mode, run.name === run.measures.mode ? null : [" ", B.h("span", { class: "dir" }, B.bdi(run.name))]),
        B.h("dl", {}, rows.map(([label, fn]) => [B.h("dt", {}, label), B.h("dd", { class: "num" }, value(fn))]))
      );
      el.style.setProperty("--mode-ink", inkOf(rank(run)));
      return el;
    };
    $("ranking").replaceChildren(
      ...runs.map((run) => (cp(run) ? card(run, RANKING, (fn) => fn(cp(run))) : B.h("article", { class: "mode" }, B.h("h3", {}, run.measures.mode), B.h("p", { class: "none" }, "No checkpoints: not ranked (Phase 1)."))))
    );
    $("diagnostics").replaceChildren(...runs.map((run) => card(run, DIAGNOSTICS, (fn) => fn(run.measures))));
  }

  document.addEventListener("bracken-theme", () => runs.length && draw());
  window.addEventListener("resize", () => runs.length && draw());
  document.querySelector(".masthead nav").append(B.themeButton());
  load().catch((e) => B.text($("data-note"), `couldn't read the run: ${e.message}`));
})();
