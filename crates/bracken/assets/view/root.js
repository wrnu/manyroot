// Root: one claim's life (docs/design/visualizer.md section 3.2). The claim is a seed at the soil line.
// Below it, each artifact that implements it is a root, and evidence climbs the root toward the seed.
// Above it, the stem carries the claim's own evidence, its intents (buds whose arc shrinks as the time
// they claimed runs out) and refinements, and the flower opens when the policy accepts it. Rivals stand
// beside it, joined by a bramble.
//
// Everything drawn comes from one /api/root/<id> response: the replica now, or as of a cursor when the
// life is replayed. Statuses, counts and retractions are the server's, as `show --json` gives them; the
// page never works them out from times.
"use strict";

(() => {
  const B = Bracken;
  const $ = (id) => document.getElementById(id);
  const S = B.s;
  const SOIL = 430; // the soil line, in the stage's 1200 x 760 units
  const CX = 560; // the seed
  const HEX = /^[0-9a-f]{64}$/;
  const MAX_CURSOR = 200; // ids in one as_of cursor, so a request head stays small

  let ground = null;
  let minPasses = 1;
  const agentIndex = new Map(); // agent key -> its index in /api/ground, for its ink
  let id = null;
  let current = null; // /api/root/<id> now
  let life = []; // what happened to the claim, in order of the authors' clocks
  let stops = []; // the entries of `life` the replay stops at (those with ids)
  let then = null; // when replayed: the life as told by the response of the stop shown
  let at = -1; // the stop shown, or -1 for now
  const past = new Map(); // as_of cursor -> response

  async function api(path) {
    const r = await fetch(path, { headers: { Accept: "application/json" } });
    if (!r.ok) {
      let message = `${r.status}`;
      try {
        message = (await r.json()).error || message;
      } catch (_) {}
      throw new Error(message);
    }
    return r.json();
  }

  const nameOf = (key, name) => name || `~${String(key).slice(0, 8)}`;
  const inkOf = (key) => B.ink(agentIndex.has(key) ? agentIndex.get(key) : 0);
  const statusOf = (a) => (a ? a.status : "pending");

  // ---- The claim's life: every contribution the response names, with the ids that bring it in ----
  // `what` says how each one stands in the response it was built from; `plain` says only what happened,
  // for a step after the one shown, whose standing then the page doesn't have.
  function buildLife(r) {
    const c = r.claim;
    const ev = [{ t: c.timestamp_ms, ids: [c.id], who: nameOf(c.author, c.author_name), kind: "claim", what: "wrote it" }];
    for (const i of r.intents) {
      ev.push({ t: i.from_ms, ids: [i.id], who: nameOf(i.author, i.author_name), kind: "intent", what: "intends to work on it" });
      if (i.ended_ms !== null) ev.push({ t: i.ended_ms, ids: [], who: nameOf(i.author, i.author_name), kind: "intent-end", what: `intent ended: ${i.ended_by}`, plain: "intent ended" });
    }
    for (const im of r.implementations) {
      const a = im.artifact;
      ev.push({ t: a.timestamp_ms, ids: [a.id, im.relation], who: nameOf(a.author, a.author_name), kind: "artifact", what: im.relation_retracted ? "implemented it, relation since retracted, with" : "implements it with", plain: "implements it with", text: a.body.name, status: statusOf(im.assessment) });
      for (const e of im.evidence) ev.push(evidenceEntry(e, `on ${a.body.name}`));
    }
    for (const e of r.evidence) ev.push(evidenceEntry(e, "on the claim"));
    for (const rf of r.refinements) ev.push({ t: rf.timestamp_ms, ids: [rf.id], who: nameOf("", rf.author_name), kind: "claim", what: "refines it:", text: rf.statement });
    for (const rv of r.rivals) ev.push({ t: rv.timestamp_ms, ids: [rv.id, rv.relation].filter(Boolean), who: nameOf("", rv.author_name), kind: "claim", what: rv.how === "contradicts" ? "contradicts it:" : "replaces the same claim:", text: rv.statement });
    for (const rp of r.lineage.replaces) ev.push({ t: rp.timestamp_ms, ids: [rp.id], who: nameOf("", rp.author_name), kind: "claim", what: "wrote what it replaces:", text: rp.statement });
    for (const rb of r.lineage.replaced_by)
      ev.push({ t: rb.timestamp_ms, ids: [rb.id], who: nameOf("", rb.author_name), kind: "claim", what: rb.counts ? "would replace it:" : "would replace it, which doesn't count:", plain: "would replace it:", text: rb.statement });
    ev.sort((a, b) => a.t - b.t || (a.ids[0] || "").localeCompare(b.ids[0] || ""));
    return ev;
  }
  function evidenceEntry(e, on) {
    const what = `${e.verdict} ${on}${e.counted ? "" : ", not counted"}${e.retracted ? ", since retracted" : ""}`;
    return { t: e.timestamp_ms, ids: [e.id], who: nameOf(e.author, e.author_name), kind: "evidence", verdict: e.verdict, what, plain: `${e.verdict} ${on}`, text: e.method };
  }

  /** The cursor for stop k: the claim and everything the life brought in up to it, keeping the claim and
   * the latest ids when that's more than one cursor holds. */
  function cursorFor(k) {
    const ids = new Set();
    for (let j = 0; j <= k; j++) for (const x of stops[j].ids) if (x !== id) ids.add(x);
    return [id, ...[...ids].slice(1 - MAX_CURSOR)].join(",");
  }

  // ---- Loading ----
  async function open() {
    const want = location.hash.slice(1).toLowerCase();
    stopReplay();
    past.clear();
    if (!HEX.test(want)) return hint("Open a claim's Root view from its label in the Loom.");
    id = want;
    try {
      if (!ground) await loadGround();
      current = await api(`/api/root/${id}`);
    } catch (e) {
      return hint(`This replica can't show that claim: ${e.message}`);
    }
    at = -1;
    rebuild();
    render(current, true);
    drawTrack();
  }

  async function loadGround() {
    ground = await api("/api/ground");
    minPasses = ground.policy.min_passes;
    agentIndex.clear();
    ground.agents.forEach((a, i) => agentIndex.set(a.agent, i));
    B.text($("ground-name"), ground.name);
  }

  function rebuild() {
    life = buildLife(current);
    stops = life.filter((e) => e.ids.length);
    const slider = $("asof");
    slider.max = String(stops.length);
    slider.value = String(at < 0 ? stops.length : at);
    slider.disabled = false;
    $("replay").disabled = false;
    $("to-loom").disabled = false;
  }

  function hint(text) {
    id = null;
    current = null;
    $("stage").replaceChildren();
    $("story").replaceChildren();
    B.text($("statement"), "");
    B.text($("byline"), "");
    $("verdict").replaceChildren(B.h("span", { class: "hint" }, text));
    B.text($("data-note"), "");
    for (const c of ["replay", "asof", "to-loom"]) $(c).disabled = true;
    drawTrack();
  }

  /** The response for stop k (or now), fetched once per cursor. */
  async function responseAt(k) {
    if (k < 0 || k >= stops.length) return current;
    const cursor = cursorFor(k);
    if (!past.has(cursor)) past.set(cursor, api(`/api/root/${id}?as_of=${encodeURIComponent(cursor)}`));
    return past.get(cursor);
  }

  let asked = 0;
  async function show(k) {
    const ticket = ++asked;
    at = k >= stops.length ? -1 : k;
    drawTrack();
    $("stage").parentElement.classList.toggle("waiting", true);
    B.text($("data-note"), at < 0 ? "" : "reading the replica as of that step…");
    let r;
    try {
      r = await responseAt(at);
    } catch (e) {
      if (ticket === asked) B.text($("data-note"), `couldn't read that step: ${e.message}`);
      return;
    } finally {
      if (ticket === asked) $("stage").parentElement.classList.toggle("waiting", false);
    }
    if (ticket !== asked) return;
    render(r, true);
    drawTrack();
  }

  // ---- Drawing ----
  const stage = $("stage");
  function render(r, entering) {
    const status = statusOf(r.claim.assessment);
    then = at < 0 ? null : buildLife(r);
    const g = [];
    // Air and soil, past the edges so a wide or tall window has no seam.
    g.push(S("rect", { x: -3000, y: -3000, width: 7200, height: 3000 + SOIL, fill: "var(--ground-raised)" }));
    g.push(S("rect", { x: -3000, y: SOIL, width: 7200, height: 3000, fill: "var(--ground)" }));
    for (let k = 0; k < 6; k++) g.push(S("rect", { x: -3000, y: SOIL + 48 + k * 52, width: 7200, height: 22, fill: "var(--stratum)", opacity: 0.28 - k * 0.03 }));
    g.push(S("path", { d: soilEdge(), fill: "none", stroke: "var(--stratum-line)", "stroke-width": 1.5 }));
    drawLineage(g, r);
    drawRivals(g, r);
    drawRoots(g, r, entering);
    drawStem(g, r, status, entering);
    // The seed itself, at the soil line.
    g.push(B.svgKnot("claim", CX, SOIL, 11, B.look("claim", { status, retracted: r.claim.retracted, agent: agentIndex.get(r.claim.author) || 0 })));
    stage.replaceChildren(...g);
    renderTitle(r, status);
    renderStory();
    B.text($("data-note"), at < 0 ? "live" : `as of step ${at + 1} of ${stops.length}: the replica as it stood then`);
  }

  function soilEdge() {
    let d = `M-40,${SOIL}`;
    for (let x = -40; x <= 1240; x += 20) d += ` L${x},${SOIL + Math.sin(x * 0.05) * 1.2 + Math.sin(x * 0.013) * 1.8}`;
    return d;
  }

  /** Up to `max` lines of at most `width` characters, broken between words; the last ends in "…" if cut. */
  function wrap(value, width, max) {
    const words = B.firstLine(value, 400).split(/\s+/);
    const lines = [""];
    for (const w of words) {
      const cur = lines[lines.length - 1];
      if (!cur) lines[lines.length - 1] = w;
      else if ((cur + " " + w).length <= width) lines[lines.length - 1] = cur + " " + w;
      else if (lines.length < max) lines.push(w);
      else {
        lines[lines.length - 1] = cur.slice(0, width - 1) + "…";
        break;
      }
    }
    return lines;
  }

  function label(x, y, lines, opts = {}) {
    // Specimen-label text: a small-caps line, then an italic line. Text nodes only.
    const anchor = opts.anchor || "start";
    const grp = S("g", { class: opts.air ? "air" : null });
    lines.forEach((ln, k) => {
      if (!ln || !ln.text) return;
      grp.append(S("text", { x, y: y + k * 17, "text-anchor": anchor, class: `halo ${ln.cls || ""}` }, ln.text));
    });
    return grp;
  }

  function drawRoots(g, r, entering) {
    const impls = r.implementations;
    if (!impls.length) {
      g.push(label(CX, SOIL + 80, [{ text: "nothing implements it yet", cls: "faint italic" }], { anchor: "middle" }));
      return;
    }
    const n = impls.length;
    const born = r.claim.timestamp_ms;
    impls.forEach((im, k) => {
      const art = im.artifact;
      // Spread the roots under the seed; older roots have grown deeper.
      const spread = Math.min(520, 180 + n * 120);
      const ex = n === 1 ? CX + 40 : CX - spread / 2 + (k * spread) / (n - 1);
      const age = (r.now_ms - art.timestamp_ms) / Math.max(1, r.now_ms - born);
      const ey = SOIL + 120 + 170 * Math.min(1, Math.max(0.15, age));
      const st = statusOf(im.assessment);
      const d = `M${CX},${SOIL + 8} C${CX + (ex - CX) * 0.1},${SOIL + 70} ${ex - (ex - CX) * 0.15},${ey - 90} ${ex},${ey}`;
      const ink = inkOf(art.author);
      const cut = im.relation_retracted || art.retracted;
      g.push(S("path", { d, fill: "none", stroke: st === "superseded" || cut ? "var(--superseded)" : ink, "stroke-width": st === "accepted" ? 3.2 : 2, "stroke-linecap": "round", "stroke-dasharray": st === "superseded" || cut ? "5 5" : null, class: entering ? "appear" : null }));
      // Root hairs: a few short side strokes, for the eye, not for data.
      for (let h = 1; h < 5; h++) {
        const f = h / 5;
        const hx = CX + (ex - CX) * f, hy = SOIL + 8 + (ey - SOIL - 8) * Math.pow(f, 1.3);
        g.push(S("path", { d: `M${hx},${hy} l${(h % 2 ? -1 : 1) * 9},${7}`, stroke: ink, "stroke-width": 0.8, opacity: 0.5 }));
      }
      // The artifact at the root's tip.
      g.push(B.svgKnot("artifact", ex, ey, 9, B.look("artifact", { status: st, retracted: art.retracted, agent: agentIndex.get(art.author) || 0 })));
      const why = im.relation_retracted ? "its link is retracted" : art.retracted ? "retracted" : st;
      g.push(label(ex + (ex < CX ? -18 : 18), ey - 4, [
        { text: `${nameOf(art.author, art.author_name)} · ${why}`, cls: "sc dim" },
        { text: B.firstLine(art.body.name, 40), cls: "italic" },
      ], { anchor: ex < CX ? "end" : "start" }));
      // Its evidence climbs the root: counted passes send spores to the seed, fails stay where they landed.
      im.evidence.forEach((e, j) => {
        const f = 0.78 - (j % 5) * 0.16;
        const px = CX + (ex - CX) * f, py = SOIL + 8 + (ey - SOIL - 8) * Math.pow(f, 1.3);
        const lk = B.look("evidence", { verdict: e.verdict, retracted: e.retracted });
        g.push(B.svgKnot("evidence", px, py, 5.5, e.counted || e.retracted ? lk : { ...lk, fill: null, stroke: "var(--root-faint)" }));
        if (e.verdict === "pass" && e.counted && !e.retracted && !cut) {
          const spore = S("circle", { r: 2.2, fill: "var(--pass)", opacity: 0.85, class: "spore" });
          spore.style.setProperty("offset-path", `path("${d}")`);
          spore.style.setProperty("animation-delay", `${-j * 1.1}s`);
          g.push(spore);
        }
        const note = e.retracted ? " (retracted)" : e.counted ? "" : " (not counted)";
        g.push(label(px + (ex < CX ? -12 : 12), py + 4, [{ text: `${nameOf(e.author, e.author_name)} ${e.verdict}: ${B.firstLine(e.method, 34)}${note}`, cls: "faint" }], { anchor: ex < CX ? "end" : "start" }));
      });
    });
  }

  function drawStem(g, r, status, entering) {
    // The stem grows with the claim's counted passes, toward the flower.
    const passes = r.claim.assessment ? r.claim.assessment.passes.length : 0;
    const top = SOIL - 150 - 90 * Math.min(1, passes / Math.max(1, minPasses));
    const sway = (y) => CX + Math.sin((SOIL - y) * 0.02) * 6;
    let d = `M${CX},${SOIL - 10}`;
    for (let y = SOIL - 10; y >= top; y -= 6) d += ` L${sway(y)},${y}`;
    const stemColour = { rejected: "var(--rejected)", accepted: "var(--accepted)", superseded: "var(--superseded)", retracted: "var(--retracted)" }[status] || "var(--root-dim)";
    g.push(S("path", { d, fill: "none", stroke: stemColour, "stroke-width": 2.6, "stroke-linecap": "round", "stroke-dasharray": status === "retracted" ? "4 4" : null }));
    g.push(flower(sway(top), top, status, passes, entering));
    if (status === "retracted") {
      // The author's pruning cut, across the stem.
      const y = (SOIL + top) / 2;
      g.push(B.svgKnot("retract", sway(y), y, 14, B.look("retract", { agent: agentIndex.get(r.claim.author) || 0 })));
      g.push(label(sway(y) + 22, y + 4, [{ text: "retracted by its author", cls: "sc dim" }], { air: true }));
    }

    // The claim's own evidence, as spores on the stem.
    r.evidence.forEach((e, j) => {
      const y = SOIL - 50 - j * 34;
      const lk = B.look("evidence", { verdict: e.verdict, retracted: e.retracted });
      g.push(B.svgKnot("evidence", sway(y), y, 5.5, e.counted || e.retracted ? lk : { ...lk, fill: null, stroke: "var(--root-faint)" }));
      const note = e.retracted ? " (retracted)" : e.counted ? "" : " (not counted)";
      g.push(label(sway(y) + 14, y + 4, [{ text: `${nameOf(e.author, e.author_name)} ${e.verdict}: ${B.firstLine(e.method, 42)}${note}`, cls: "dim" }], { air: true }));
    });

    // Intents: buds to the left of the stem. A live one wears an arc of the time it has left.
    r.intents.forEach((it, j) => {
      const y = SOIL - 70 - j * 46;
      const x = sway(y) - 46;
      const ended = it.ended_by !== null;
      const who = nameOf(it.author, it.author_name);
      g.push(S("path", { d: `M${sway(y)},${y} Q${x + 20},${y - 2} ${x + 8},${y}`, stroke: "var(--root-dim)", fill: "none", "stroke-width": 1.4 }));
      const lk = ended ? { fill: null, stroke: "var(--superseded)", dash: [2, 2] } : { fill: null, stroke: inkOf(it.author) };
      g.push(B.svgKnot("claim", x, y, 6, lk, { transform: `translate(${x} ${y}) rotate(-60)` }));
      if (!ended) {
        const left = Math.max(0, Math.min(1, (it.until_ms - r.now_ms) / Math.max(1, it.until_ms - it.from_ms)));
        const rr = 13;
        const a1 = -Math.PI / 2, a2 = a1 + left * Math.PI * 2 * 0.999;
        g.push(S("path", { d: `M${x + rr * Math.cos(a1)},${y + rr * Math.sin(a1)} A${rr},${rr} 0 ${left > 0.5 ? 1 : 0} 1 ${x + rr * Math.cos(a2)},${y + rr * Math.sin(a2)}`, fill: "none", stroke: "var(--focus)", "stroke-width": 2, "stroke-linecap": "round" }));
        const mins = Math.max(0, Math.round((it.until_ms - r.now_ms) / 60000));
        g.push(label(x - 22, y + 4, [{ text: `${who} intends · ${mins >= 60 ? Math.round(mins / 60) + " h" : mins + " min"} left`, cls: "sc" }], { anchor: "end", air: true }));
      } else {
        g.push(label(x - 16, y + 4, [{ text: `${who}'s intent ended: ${it.ended_by}`, cls: "faint" }], { anchor: "end", air: true }));
      }
    });

    // Refinements: leaves on the right of the stem.
    r.refinements.forEach((rf, j) => {
      const y = SOIL - 100 - j * 40;
      const x = sway(y);
      g.push(S("path", { d: `M${x},${y} q22,-16 44,-4 q-22,12 -44,4 z`, fill: "var(--accepted-glow)", stroke: "var(--root-dim)", "stroke-width": 1 }));
      g.push(label(x + 52, y - 2, [{ text: `${nameOf("", rf.author_name)} refines: ${B.firstLine(rf.statement, 36)}`, cls: "dim italic" }], { air: true }));
    });
  }

  function flower(x, y, status, passes, entering) {
    // Pending: a closed bud. Accepted: open petals, lit. Rejected: petals fallen, rust. Superseded or
    // retracted: a dry, pressed bud.
    const grp = S("g", { transform: `translate(${x} ${y}) scale(1.45)` });
    const inner = S("g", { class: status === "accepted" && entering && !B.reducedMotion() ? "bloom" : null });
    grp.append(inner);
    const dry = status === "superseded" || status === "retracted";
    for (let p = 0; p < 6; p++) {
      const a = (p / 6) * 360;
      let d, fill, stroke, transform;
      if (status === "accepted") {
        d = "M0,0 C-9,-10 -8,-26 0,-32 C8,-26 9,-10 0,0 Z";
        fill = "var(--accepted)";
        stroke = "var(--accepted)";
        transform = `rotate(${a})`;
      } else if (status === "rejected") {
        d = "M0,0 C-6,6 -5,18 0,22 C5,18 6,6 0,0 Z";
        fill = "none";
        stroke = "var(--rejected)";
        transform = `rotate(${a * 0.25 - 45})`;
      } else {
        d = "M0,0 C-5,-6 -4,-18 0,-22 C4,-18 5,-6 0,0 Z";
        fill = "none";
        stroke = dry ? "var(--superseded)" : "var(--root-dim)";
        transform = `rotate(${(p - 2.5) * 9})`;
      }
      inner.append(S("path", { d, fill, stroke, "stroke-width": 1.2, transform, opacity: status === "accepted" ? 0.9 : 1, "stroke-dasharray": dry ? "2 2" : null }));
    }
    if (status === "accepted") inner.append(S("circle", { r: 34, fill: "var(--accepted-glow)" }));
    // The receptacle's slots: one per pass the policy needs, filled as counted passes arrive. What's missing shows.
    const slots = Math.max(minPasses, passes);
    for (let k = 0; k < slots; k++) {
      const a = -Math.PI / 2 + (k - (slots - 1) / 2) * 0.5;
      inner.append(S("circle", { cx: 6 * Math.cos(a), cy: 6 * Math.sin(a), r: 3.2, fill: k < passes ? "var(--focus)" : "var(--ground-raised)", stroke: "var(--focus)", "stroke-width": 1 }));
    }
    return grp;
  }

  function drawRivals(g, r) {
    r.rivals.slice(0, 2).forEach((rv, j) => {
      const x = 960 + j * 110, y = SOIL;
      // The bramble: a thorned line from this claim's stem to the rival's.
      const base = SOIL - 16;
      let d = `M${CX + 16},${base}`;
      const steps = 16;
      for (let k = 1; k <= steps; k++) d += ` L${CX + 16 + ((x - CX - 26) * k) / steps},${base + (k % 2 ? -5 : 5)}`;
      g.push(S("path", { d, fill: "none", stroke: "var(--rejected)", "stroke-width": 1.3, opacity: 0.8 }));
      for (let k = 1; k < steps; k += 2) g.push(S("path", { d: `M${CX + 16 + ((x - CX - 26) * k) / steps},${base - 5} l3,-7`, stroke: "var(--rejected)", "stroke-width": 1.1 }));
      g.push(S("text", { x: (CX + x) / 2, y: base + 22 + j * 16, "text-anchor": "middle", class: "sc faint halo" }, rv.how === "contradicts" ? "contradicts" : "divergent"));
      g.push(S("path", { d: `M${x},${y - 8} q-4,-40 2,-70`, stroke: "var(--root-dim)", "stroke-width": 2, fill: "none" }));
      g.push(B.svgKnot("claim", x, y, 8, B.look("claim", { status: rv.status || "pending", agent: 3 })));
      g.push(label(x, y - 130 - j * 60, [
        { text: `${rv.how} · ${nameOf("", rv.author_name)} · ${rv.status || "pending"}`, cls: "sc dim" },
        ...wrap(rv.statement, 28, 3).map((text) => ({ text, cls: "italic" })),
      ], { anchor: "middle", air: true }));
    });
    if (r.rivals.length > 2) g.push(label(1015, SOIL + 40, [{ text: `and ${r.rivals.length - 2} more rivals, in the story`, cls: "faint" }], { anchor: "middle" }));
  }

  function drawLineage(g, r) {
    r.lineage.replaces.slice(0, 3).forEach((rp, j) => {
      const x = 150 + j * 90;
      g.push(B.svgKnot("claim", x, SOIL, 8, B.look("claim", { status: "superseded" })));
      g.push(label(x, SOIL + 30, [{ text: `replaces · ${nameOf("", rp.author_name)}`, cls: "sc faint" }, { text: B.firstLine(rp.statement, 30), cls: "italic faint" }], { anchor: "middle" }));
    });
    r.lineage.replaced_by.slice(0, 2).forEach((rb, j) => {
      const x = 960 + (Math.min(2, r.rivals.length) + j) * 120;
      g.push(S("path", { d: `M${CX + 14},${SOIL + 4} Q${(CX + x) / 2},${SOIL + 40} ${x - 10},${SOIL + 4}`, fill: "none", stroke: "var(--superseded)", "stroke-width": 1.2, "stroke-dasharray": "3 4" }));
      g.push(S("path", { d: `M${x},${SOIL - 8} q3,-40 -2,-96`, stroke: "var(--root-dim)", "stroke-width": 1.6, fill: "none" }));
      g.push(B.svgKnot("claim", x, SOIL, 7, B.look("claim", { status: rb.status || "pending", agent: 3 })));
      g.push(label(x, SOIL - 170, [
        { text: `${nameOf("", rb.author_name)} would replace it`, cls: "sc dim" },
        ...wrap(rb.statement, 28, 2).map((text) => ({ text, cls: "italic" })),
        { text: rb.counts ? "" : "doesn't count: it isn't by this claim's author", cls: "faint" },
      ], { anchor: "middle", air: true }));
    });
  }

  // ---- The title and the story ----
  function renderTitle(r, status) {
    const c = r.claim;
    B.text($("statement"), B.firstLine(c.body.statement || c.body.name, 240));
    const tags = c.body.tags && c.body.tags.length ? c.body.tags : [];
    $("byline").replaceChildren(
      ...[
        B.bdi(nameOf(c.author, c.author_name)),
        tags.map((t, k) => [k ? ", " : " · ", B.bdi(t)]),
        ` · written ${B.day(c.timestamp_ms)} ${B.clock(c.timestamp_ms)} UTC, their clock · ${c.id.slice(0, 12)}`,
      ].flat(Infinity)
    );
    const v = $("verdict");
    v.dataset.status = status;
    const a = c.assessment || { passes: [], fails: [], superseded_by: [] };
    const passes = a.passes.length;
    const plural = (n, word) => `${n} ${word}${n === 1 ? "" : "es"}`;
    const why = {
      accepted: `${plural(passes, "counted pass")}, the policy needs ${minPasses}`,
      rejected: `${a.fails.length} counted fail${a.fails.length === 1 ? "" : "s"}`,
      pending: `waiting: ${passes} of ${minPasses} counted pass${minPasses === 1 ? "" : "es"}`,
      superseded: `superseded by ${a.superseded_by.map((x) => x.slice(0, 12)).join(", ")}`,
      retracted: "retracted by its author",
    }[status] || "";
    v.replaceChildren(B.h("span", { class: "word" }, status), ` · ${why}`);
  }

  /** How a life entry looks; `later` (a step after the one shown) draws it without a status. */
  function lookOf(e, later) {
    if (e.kind === "evidence") return B.look("evidence", { verdict: e.verdict });
    if (e.kind === "artifact") return B.look("artifact", { status: later ? "pending" : e.status, agent: 2 });
    if (e.kind === "intent-end") return { fill: null, stroke: "var(--superseded)", dash: [2, 2] };
    return B.look("claim", { status: "pending", agent: 1 });
  }
  const shapeOf = (e) => (e.kind === "intent" || e.kind === "intent-end" ? "claim" : e.kind);

  function renderStory() {
    // Now: the whole life as it stands. Replayed: the life as the stop's own response tells it, then what
    // came after, faded, saying only what happened (how it stands now would be a later status).
    const line = (e, cls, later) =>
      B.h(
        "li",
        { class: cls },
        B.h("span", { class: "when num" }, B.clock(e.t)),
        B.knotIcon(shapeOf(e), lookOf(e, later), 16),
        B.h("span", { class: "what" }, B.h("span", { class: "sc" }, B.bdi(e.who), " "), later ? e.plain || e.what : e.what, e.text ? [" ", B.h("span", { class: "statement" }, B.firstLine(e.text, 70))] : null)
      );
    if (!then) return $("story").replaceChildren(...life.map((e) => line(e, null, false)));
    const here = stops[at].ids[0];
    const told = new Set(then.flatMap((e) => e.ids));
    const after = life.slice(life.indexOf(stops[at]) + 1).filter((e) => !e.ids.some((x) => told.has(x)));
    $("story").replaceChildren(...then.map((e) => line(e, e.ids[0] === here ? "step-now" : null, false)), ...after.map((e) => line(e, "later", true)));
  }

  // ---- Replay: the life's track, and the scrubber ----
  const track = $("track-canvas");
  function drawTrack() {
    const dpr = window.devicePixelRatio || 1;
    const rect = track.getBoundingClientRect();
    track.width = Math.round(rect.width * dpr);
    track.height = Math.round(rect.height * dpr);
    const ctx = track.getContext("2d");
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, rect.width, rect.height);
    if (!stops.length) return B.text($("asof-label"), "");
    // Stops sit evenly along the track, in the order the life is told: the authors' clocks are advisory.
    const x = (k) => 8 + (k / Math.max(1, stops.length)) * (rect.width - 16);
    ctx.strokeStyle = B.token("--stratum-line");
    ctx.beginPath();
    ctx.moveTo(8, rect.height / 2);
    ctx.lineTo(rect.width - 8, rect.height / 2);
    ctx.stroke();
    // Replayed, a stop up to the one shown looks as the shown response tells it; a later one has no status.
    const asThen = new Map((then || []).map((e) => [e.ids[0], e]));
    stops.forEach((e, k) => {
      const later = at >= 0 && k > at;
      const told = at < 0 ? e : asThen.get(e.ids[0]);
      B.drawKnot(ctx, shapeOf(e) === "intent-end" ? "claim" : shapeOf(e), x(k), rect.height / 2, 4, lookOf(told || e, later || !told), later ? 0.3 : 1);
    });
    B.text($("asof-label"), at < 0 ? "now" : `${B.clock(stops[at].t)} · step ${at + 1}/${stops.length}`);
  }

  $("asof").addEventListener("input", (e) => {
    stopReplay();
    show(Number(e.target.value));
  });

  let timer = 0;
  const replayBtn = $("replay");
  function stopReplay() {
    clearTimeout(timer);
    timer = 0;
    B.attr(replayBtn, "aria-pressed", "false");
    B.text(replayBtn, "Replay its life");
  }
  replayBtn.addEventListener("click", () => {
    if (replayBtn.getAttribute("aria-pressed") === "true") return stopReplay();
    B.attr(replayBtn, "aria-pressed", "true");
    B.text(replayBtn, "Pause");
    // Stop by stop: each shows the replica as it stood once that contribution had arrived.
    let k = 0;
    const step = async () => {
      $("asof").value = String(k);
      if (k + 1 < stops.length) responseAt(k + 1).catch(() => {}); // fetch ahead while this one shows
      await show(k);
      k++;
      if (replayBtn.getAttribute("aria-pressed") !== "true") return;
      if (k <= stops.length) timer = setTimeout(step, B.reducedMotion() ? 1400 : 1100);
      else stopReplay();
    };
    step();
  });

  $("to-loom").addEventListener("click", () => id && location.assign(`/#${id}`));

  // ---- Live: the claim's life follows the replica while it's shown as it is now ----
  let refetch = 0;
  function listen() {
    const events = new EventSource(`/api/events?since=${encodeURIComponent(ground ? ground.cursor : "")}`);
    events.addEventListener("contributions", (e) => {
      const d = JSON.parse(e.data);
      if (d.graph.author.some((a) => a >= agentIndex.size)) loadGround().catch(() => {});
      clearTimeout(refetch);
      refetch = setTimeout(async () => {
        if (!id) return;
        try {
          current = await api(`/api/root/${id}`);
        } catch (_) {
          return;
        }
        rebuild();
        if (at < 0) render(current, false);
        else renderStory();
        drawTrack();
      }, 300);
    });
  }

  document.addEventListener("bracken-theme", () => {
    if (current) show(at);
    drawTrack();
  });
  window.addEventListener("resize", drawTrack);
  window.addEventListener("hashchange", open);
  document.querySelector(".masthead nav").append(B.themeButton());
  (async () => {
    try {
      await loadGround();
    } catch (_) {
      return hint("Couldn't read the Ground.");
    }
    await open();
    listen();
  })();
})();
