// Root: one claim's life. The claim is a seed at the soil line. Below it, each artifact that implements it
// is a root, and evidence climbs the root toward the seed. Above it, the stem carries the claim's own
// evidence, its live intents (buds whose arc shrinks as the time they claimed runs out) and refinements,
// and the flower opens when the policy accepts it. Rivals stand beside it, joined by a bramble.
"use strict";

(() => {
  const B = Bracken;
  const $ = (id) => document.getElementById(id);
  const S = B.s;
  const SOIL = 430; // the soil line, in the stage's 1200 x 760 units
  const CX = 560; // the seed

  const sample = B.sample();
  const ids = Object.keys(sample.roots);
  let root; // the /api/root/<id> response
  let asOf = Infinity; // replay: only what existed by then
  let life = []; // [{ t, label, kind, ... }] in time order
  const minPasses = sample.ground.policy.min_passes;

  // ---- The claim picker ----
  const pick = $("claim-pick");
  for (const id of ids) pick.append(B.h("option", { value: id }, B.firstLine(sample.roots[id].claim.body.statement, 70)));
  pick.addEventListener("change", () => open(pick.value));
  B.text($("ground-name"), sample.ground.name);
  B.text($("data-note"), "Synthetic sample; times respaced for display.");
  document.querySelector(".masthead nav").append(B.themeButton());

  // ---- Status as of a moment ----
  // The server will answer this (/api/root/<id>?as_of=<cursor>); the mockup works it out from the evidence's times.
  function statusAt(evidence, t, supersededAt = Infinity) {
    const counted = evidence.filter((e) => e.counted && !e.retracted && e.timestamp_ms <= t);
    if (t >= supersededAt) return "superseded";
    if (counted.some((e) => e.verdict === "fail")) return "rejected";
    if (counted.filter((e) => e.verdict === "pass").length >= minPasses) return "accepted";
    return "pending";
  }

  function open(id) {
    root = sample.roots[id];
    pick.value = id;
    if (location.hash.slice(1) !== id) history.replaceState(null, "", `#${id}`);
    life = buildLife();
    asOf = Infinity;
    $("asof").value = 1000;
    render(true);
    drawTrack();
  }

  function buildLife() {
    const c = root.claim;
    const ev = [{ t: c.timestamp_ms, what: "written", who: c.author_name, kind: "claim", text: c.body.statement }];
    for (const i of root.intents) {
      ev.push({ t: i.from_ms, what: "intends to work on it", who: i.author_name, kind: "intent" });
      if (i.ended_ms) ev.push({ t: i.ended_ms, what: `intent ended: ${i.ended_by}`, who: i.author_name, kind: "intent-end" });
    }
    for (const im of root.implementations) {
      ev.push({ t: im.artifact.timestamp_ms, what: "implements it with", who: im.artifact.author_name, kind: "artifact", text: im.artifact.body.name, status: im.assessment.status });
      for (const e of im.evidence) ev.push({ t: e.timestamp_ms, what: `${e.verdict} on ${im.artifact.body.name}`, who: e.author_name, kind: "evidence", verdict: e.verdict, text: e.method });
    }
    for (const e of root.evidence) ev.push({ t: e.timestamp_ms, what: `${e.verdict} on the claim`, who: e.author_name, kind: "evidence", verdict: e.verdict, text: e.method });
    ev.sort((a, b) => a.t - b.t);
    return ev;
  }

  // ---- Drawing ----
  const stage = $("stage");
  function render(entering) {
    const t = asOf;
    const claim = root.claim;
    const status = Number.isFinite(t) ? statusAt(root.evidence, t) : claim.assessment.status;
    const g = [];

    // Air and soil, past the edges so a wide or tall window has no seam.
    g.push(S("rect", { x: -3000, y: -3000, width: 7200, height: 3000 + SOIL, fill: "var(--ground-raised)" }));
    g.push(S("rect", { x: -3000, y: SOIL, width: 7200, height: 3000, fill: "var(--ground)" }));
    for (let k = 0; k < 6; k++)
      g.push(S("rect", { x: -3000, y: SOIL + 48 + k * 52, width: 7200, height: 22, fill: "var(--stratum)", opacity: 0.28 - k * 0.03 }));
    g.push(S("path", { d: soilEdge(), fill: "none", stroke: "var(--stratum-line)", "stroke-width": 1.5 }));

    // Lineage: what this claim replaces (a husk to the left), and what would replace it (a seedling).
    drawLineage(g, t);
    // Rivals: neighbouring plants, joined by a bramble.
    drawRivals(g, t);
    // Roots: what implements it.
    drawRoots(g, t, entering);
    // The stem, its evidence, buds and leaves, and the flower.
    drawStem(g, t, status, entering);

    // The seed itself, at the soil line.
    const lk = B.look("claim", { status, agent: 1 });
    g.push(B.svgKnot("claim", CX, SOIL, 11, lk));

    stage.replaceChildren(...g);
    renderTitle(status);
    renderStory();
  }

  function soilEdge() {
    let d = `M-40,${SOIL}`;
    for (let x = -40; x <= 1240; x += 20) d += ` L${x},${SOIL + Math.sin(x * 0.05) * 1.2 + Math.sin(x * 0.013) * 1.8}`;
    return d;
  }

  /** Up to `max` lines of at most `width` characters, broken between words; the last one ends in "…" if cut. */
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
      if (!ln) return;
      grp.append(S("text", { x, y: y + k * 17, "text-anchor": anchor, class: `halo ${ln.cls || ""}` }, ln.text));
    });
    return grp;
  }

  function drawRoots(g, t, entering) {
    const impls = root.implementations;
    if (!impls.length) {
      g.push(label(CX, SOIL + 80, [{ text: "nothing implements it yet", cls: "faint italic" }], { anchor: "middle" }));
      return;
    }
    const n = impls.length;
    impls.forEach((im, k) => {
      const art = im.artifact;
      if (art.timestamp_ms > t) return;
      // Spread the roots under the seed; older roots have grown deeper.
      const spread = Math.min(520, 180 + n * 120);
      const ex = n === 1 ? CX + 40 : CX - spread / 2 + (k * spread) / (n - 1);
      const age = (Math.min(t, root.now_ms) - art.timestamp_ms) / Math.max(1, root.now_ms - root.claim.timestamp_ms);
      const ey = SOIL + 120 + 170 * Math.min(1, Math.max(0.15, age));
      const superseded = im.assessment.status === "superseded" || im.assessment.superseded_by.length > 0;
      const supersededAt = superseded ? Math.min(...impls.filter((o) => im.assessment.superseded_by.includes(o.artifact.id)).map((o) => o.artifact.timestamp_ms), Infinity) : Infinity;
      const st = Number.isFinite(t) ? statusAt(im.evidence, t, supersededAt) : im.assessment.status;
      const id = `root-${k}`;
      const d = `M${CX},${SOIL + 8} C${CX + (ex - CX) * 0.1},${SOIL + 70} ${ex - (ex - CX) * 0.15},${ey - 90} ${ex},${ey}`;
      const ink = B.ink(sample.ground.agents.findIndex((a) => a.agent === art.author));
      const cut = im.relation_retracted;
      g.push(S("path", { id, d, fill: "none", stroke: st === "superseded" ? "var(--superseded)" : ink, "stroke-width": st === "accepted" ? 3.2 : 2, "stroke-linecap": "round", "stroke-dasharray": st === "superseded" || cut ? "5 5" : null, class: entering ? "appear" : null }));
      // Root hairs: a few short side strokes, for the eye, not for data.
      for (let h = 1; h < 5; h++) {
        const f = h / 5;
        const hx = CX + (ex - CX) * f, hy = SOIL + 8 + (ey - SOIL - 8) * Math.pow(f, 1.3);
        g.push(S("path", { d: `M${hx},${hy} l${(h % 2 ? -1 : 1) * 9},${7}`, stroke: ink, "stroke-width": 0.8, opacity: 0.5 }));
      }
      // The artifact at the root's tip.
      g.push(B.svgKnot("artifact", ex, ey, 9, B.look("artifact", { status: st, agent: 2 })));
      g.push(label(ex + (ex < CX ? -18 : 18), ey - 4, [
        { text: `${art.author_name || "~" + art.author.slice(0, 8)} · ${st}`, cls: "sc dim" },
        { text: B.firstLine(art.body.name, 40), cls: "italic" },
      ], { anchor: ex < CX ? "end" : "start" }));
      // Its evidence climbs the root: passes flow up to the seed, fails stay where they landed.
      im.evidence.forEach((e, j) => {
        if (e.timestamp_ms > t) return;
        const f = 0.78 - j * 0.16;
        const px = CX + (ex - CX) * f, py = SOIL + 8 + (ey - SOIL - 8) * Math.pow(f, 1.3);
        const lk = B.look("evidence", { verdict: e.verdict });
        const knot = B.svgKnot("evidence", px, py, 5.5, e.counted ? lk : { ...lk, fill: null, stroke: "var(--root-faint)" });
        g.push(knot);
        if (e.verdict === "pass" && e.counted && !B.reducedMotion()) {
          // A spore of the pass, rising along the root to the seed, again and again.
          const spore = S("circle", { r: 2.2, fill: "var(--pass)", opacity: 0.85 });
          spore.append(S("animateMotion", { dur: `${3.4 + j}s`, repeatCount: "indefinite", keyPoints: "1;0", keyTimes: "0;1", calcMode: "linear" }, S("mpath", { href: `#${id}` })));
          g.push(spore);
        }
        g.push(label(px + (ex < CX ? -12 : 12), py + 4, [{ text: `${e.author_name} ${e.verdict}: ${B.firstLine(e.method, 34)}`, cls: "faint" }], { anchor: ex < CX ? "end" : "start" }));
      });
    });
  }

  function drawStem(g, t, status, entering) {
    // The stem grows with the claim's counted passes, toward the flower.
    const passes = root.evidence.filter((e) => e.counted && e.verdict === "pass" && e.timestamp_ms <= t).length;
    const top = SOIL - 150 - 90 * Math.min(1, passes / Math.max(1, minPasses));
    const sway = (y) => CX + Math.sin((SOIL - y) * 0.02) * 6;
    let d = `M${CX},${SOIL - 10}`;
    for (let y = SOIL - 10; y >= top; y -= 6) d += ` L${sway(y)},${y}`;
    const stemColour = status === "rejected" ? "var(--rejected)" : status === "accepted" ? "var(--accepted)" : "var(--root-dim)";
    g.push(S("path", { d, fill: "none", stroke: stemColour, "stroke-width": 2.6, "stroke-linecap": "round" }));
    g.push(flower(sway(top), top, status, passes, entering));

    // The claim's own evidence, as spores on the stem.
    root.evidence.forEach((e, j) => {
      if (e.timestamp_ms > t) return;
      const y = SOIL - 50 - j * 34;
      g.push(B.svgKnot("evidence", sway(y), y, 5.5, B.look("evidence", { verdict: e.verdict })));
      g.push(label(sway(y) + 14, y + 4, [{ text: `${e.author_name} ${e.verdict}: ${B.firstLine(e.method, 42)}`, cls: "dim" }], { air: true }));
    });

    // Intents: buds to the left of the stem. A live one wears an arc of the time it has left.
    root.intents.forEach((it, j) => {
      if (it.from_ms > t) return;
      const y = SOIL - 70 - j * 46;
      const x = sway(y) - 46;
      const ended = it.ended_ms !== null && it.ended_ms <= t;
      g.push(S("path", { d: `M${sway(y)},${y} Q${x + 20},${y - 2} ${x + 8},${y}`, stroke: "var(--root-dim)", fill: "none", "stroke-width": 1.4 }));
      const lk = ended ? { fill: null, stroke: "var(--superseded)", dash: [2, 2] } : { fill: null, stroke: B.ink(sample.ground.agents.findIndex((a) => a.agent === it.author)) };
      g.push(B.svgKnot("claim", x, y, 6, lk, { transform: `translate(${x} ${y}) rotate(-60)` }));
      const now = Math.min(t, root.now_ms);
      if (!ended) {
        const left = Math.max(0, Math.min(1, (it.until_ms - now) / (it.until_ms - it.from_ms)));
        const r = 13;
        const a1 = -Math.PI / 2, a2 = a1 + left * Math.PI * 2 * 0.999;
        g.push(S("path", { d: `M${x + r * Math.cos(a1)},${y + r * Math.sin(a1)} A${r},${r} 0 ${left > 0.5 ? 1 : 0} 1 ${x + r * Math.cos(a2)},${y + r * Math.sin(a2)}`, fill: "none", stroke: "var(--focus)", "stroke-width": 2, "stroke-linecap": "round" }));
        const mins = Math.round((it.until_ms - now) / 60000);
        g.push(label(x - 22, y + 4, [{ text: `${it.author_name} intends · ${mins >= 60 ? Math.round(mins / 60) + " h" : mins + " min"} left`, cls: "sc" }], { anchor: "end", air: true }));
      } else {
        g.push(label(x - 16, y + 4, [{ text: `${it.author_name}'s intent ended: ${it.ended_by}`, cls: "faint" }], { anchor: "end", air: true }));
      }
    });

    // Refinements: leaves on the right of the stem.
    root.refinements.forEach((rf, j) => {
      const y = SOIL - 100 - j * 40;
      const x = sway(y);
      g.push(S("path", { d: `M${x},${y} q22,-16 44,-4 q-22,12 -44,4 z`, fill: "var(--accepted-glow)", stroke: "var(--root-dim)", "stroke-width": 1 }));
      g.push(label(x + 52, y - 2, [{ text: `${rf.author_name} refines: ${B.firstLine(rf.statement, 36)}`, cls: "dim italic" }], { air: true }));
    });
  }

  function flower(x, y, status, passes, entering) {
    // Pending: a closed bud. Accepted: open petals, lit. Rejected: petals fallen, rust.
    const grp = S("g", { transform: `translate(${x} ${y}) scale(1.45)` });
    const inner = S("g", { class: status === "accepted" && entering && !B.reducedMotion() ? "bloom" : null });
    grp.append(inner);
    const petals = 6;
    for (let p = 0; p < petals; p++) {
      const a = (p / petals) * 360;
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
        stroke = "var(--root-dim)";
        transform = `rotate(${(p - 2.5) * 9})`;
      }
      inner.append(S("path", { d, fill, stroke, "stroke-width": 1.2, transform, opacity: status === "accepted" ? 0.9 : 1 }));
    }
    if (status === "accepted") inner.append(S("circle", { r: 34, fill: "var(--accepted-glow)" }));
    // The receptacle's slots: one per pass the policy needs, filled as counted passes arrive. What's missing shows.
    const slots = Math.max(minPasses, passes);
    for (let k = 0; k < slots; k++) {
      const a = -Math.PI / 2 + (k - (slots - 1) / 2) * 0.5;
      const filled = k < passes;
      inner.append(S("circle", { cx: 6 * Math.cos(a), cy: 6 * Math.sin(a), r: 3.2, fill: filled ? "var(--focus)" : "var(--ground-raised)", stroke: "var(--focus)", "stroke-width": 1 }));
    }
    return grp;
  }

  // When a short entry's contribution was written, for replay.
  const arrived = (entry) => entry.timestamp_ms;

  function drawRivals(g, t) {
    root.rivals.filter((rv) => arrived(rv) <= t).forEach((rv, j) => {
      const x = 960 + j * 110, y = SOIL;
      // The bramble: a thorned line from this claim's stem to the rival's.
      const base = SOIL - 16;
      let d = `M${CX + 16},${base}`;
      const steps = 16;
      for (let k = 1; k <= steps; k++) {
        const px = CX + 16 + ((x - CX - 26) * k) / steps;
        const py = base + (k % 2 ? -5 : 5);
        d += ` L${px},${py}`;
      }
      g.push(S("path", { d, fill: "none", stroke: "var(--rejected)", "stroke-width": 1.3, opacity: 0.8 }));
      for (let k = 1; k < steps; k += 2) {
        const px = CX + 16 + ((x - CX - 26) * k) / steps;
        g.push(S("path", { d: `M${px},${base - 5} l3,-7`, stroke: "var(--rejected)", "stroke-width": 1.1 }));
      }
      g.push(S("text", { x: (CX + x) / 2, y: base + 22, "text-anchor": "middle", class: "sc faint halo" }, "contradicts"));
      g.push(S("path", { d: `M${x},${y - 8} q-4,-40 2,-70`, stroke: "var(--root-dim)", "stroke-width": 2, fill: "none" }));
      g.push(B.svgKnot("claim", x, y, 8, B.look("claim", { status: rv.status, agent: 3 })));
      g.push(label(x, y - 130, [
        { text: `${rv.how} · ${rv.author_name} · ${rv.status}`, cls: "sc dim" },
        ...wrap(rv.statement, 28, 3).map((text) => ({ text, cls: "italic" })),
      ], { anchor: "middle", air: true }));
    });
  }

  function drawLineage(g, t) {
    root.lineage.replaces.forEach((rp, j) => {
      const x = 150 + j * 90;
      g.push(B.svgKnot("claim", x, SOIL, 8, B.look("claim", { status: "superseded" })));
      g.push(label(x, SOIL + 30, [{ text: `replaces · ${rp.author_name}`, cls: "sc faint" }, { text: B.firstLine(rp.statement, 30), cls: "italic faint" }], { anchor: "middle" }));
    });
    root.lineage.replaced_by.filter((rb) => arrived(rb) <= t).forEach((rb, j) => {
      const x = 960 + (root.rivals.length + j) * 120;
      g.push(S("path", { d: `M${CX + 14},${SOIL + 4} Q${(CX + x) / 2},${SOIL + 40} ${x - 10},${SOIL + 4}`, fill: "none", stroke: "var(--superseded)", "stroke-width": 1.2, "stroke-dasharray": "3 4" }));
      g.push(S("path", { d: `M${x},${SOIL - 8} q3,-20 -2,-36`, stroke: "var(--root-dim)", "stroke-width": 1.6, fill: "none" }));
      g.push(B.svgKnot("claim", x, SOIL, 7, B.look("claim", { status: rb.status, agent: 3 })));
      g.push(label(x, SOIL - 90, [
        { text: `${rb.author_name} would replace it`, cls: "sc dim" },
        ...wrap(rb.statement, 28, 2).map((text) => ({ text, cls: "italic" })),
        { text: rb.counts === false ? `doesn't count: ${rb.author_name} didn't write it` : "", cls: "faint" },
      ], { anchor: "middle", air: true }));
    });
  }

  // ---- The title and the story ----
  function renderTitle(status) {
    const c = root.claim;
    B.text($("statement"), c.body.statement);
    const tags = c.body.tags && c.body.tags.length ? ` · ${c.body.tags.join(", ")}` : "";
    B.text($("byline"), `${c.author_name || "~" + c.author.slice(0, 8)}${tags} · written ${B.day(c.timestamp_ms)} ${B.clock(c.timestamp_ms)}, their clock`);
    const v = $("verdict");
    v.dataset.status = status;
    const passes = root.evidence.filter((e) => e.counted && e.verdict === "pass" && e.timestamp_ms <= asOf).length;
    const why = {
      accepted: `${passes} counted pass${passes === 1 ? "" : "es"}, the policy needs ${minPasses}`,
      rejected: "a counted validator reported a failure",
      pending: `waiting: ${passes} of ${minPasses} counted pass${minPasses === 1 ? "" : "es"}`,
      superseded: "an accepted replacement supersedes it",
    }[status];
    v.replaceChildren(B.h("span", { class: "word" }, status), ` · ${why}`);
  }

  function renderStory() {
    const list = $("story");
    list.replaceChildren(
      ...life.map((e) => {
        const lk =
          e.kind === "evidence" ? B.look("evidence", { verdict: e.verdict })
          : e.kind === "artifact" ? B.look("artifact", { status: e.status, agent: 2 })
          : e.kind === "intent-end" ? { fill: null, stroke: "var(--superseded)", dash: [2, 2] }
          : B.look("claim", { status: "pending", agent: 1 });
        const shape = e.kind === "intent" || e.kind === "intent-end" ? "claim" : e.kind;
        return B.h(
          "li",
          { class: e.t > asOf ? "later" : null },
          B.h("span", { class: "when num" }, B.clock(e.t)),
          B.knotIcon(shape, lk, 16),
          B.h("span", { class: "what" }, B.h("span", { class: "sc" }, `${e.who || ""} `), e.what, e.text ? [" ", B.h("span", { class: "statement" }, B.firstLine(e.text, 70))] : null)
        );
      })
    );
  }

  // ---- Replay: the life's track, and the scrubber ----
  const track = $("track-canvas");
  function span() {
    const t0 = root.claim.timestamp_ms;
    return [t0, Math.max(root.now_ms, life[life.length - 1].t)];
  }
  function drawTrack() {
    const dpr = window.devicePixelRatio || 1;
    const r = track.getBoundingClientRect();
    track.width = Math.round(r.width * dpr);
    track.height = Math.round(r.height * dpr);
    const ctx = track.getContext("2d");
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, r.width, r.height);
    const [a, b] = span();
    const x = (t) => 8 + ((t - a) / (b - a)) * (r.width - 16);
    ctx.strokeStyle = B.token("--stratum-line");
    ctx.beginPath();
    ctx.moveTo(8, r.height / 2);
    ctx.lineTo(r.width - 8, r.height / 2);
    ctx.stroke();
    for (const e of life) {
      const lk = e.kind === "evidence" ? B.look("evidence", { verdict: e.verdict }) : e.kind === "artifact" ? B.look("artifact", { status: e.status, agent: 2 }) : B.look("claim", { status: "pending", agent: 1 });
      B.drawKnot(ctx, e.kind === "evidence" || e.kind === "artifact" ? e.kind : "claim", x(e.t), r.height / 2, 4, lk);
    }
    B.text($("asof-label"), Number.isFinite(asOf) ? B.clock(asOf) : "now");
  }

  $("asof").addEventListener("input", (e) => {
    stopReplay();
    const [a, b] = span();
    const v = Number(e.target.value);
    asOf = v >= 1000 ? Infinity : a + (v / 1000) * (b - a);
    render(false);
    drawTrack();
  });

  let timer = 0;
  const replayBtn = $("replay");
  function stopReplay() {
    clearTimeout(timer);
    replayBtn.setAttribute("aria-pressed", "false");
    B.text(replayBtn, "Replay its life");
  }
  replayBtn.addEventListener("click", () => {
    if (replayBtn.getAttribute("aria-pressed") === "true") return stopReplay();
    replayBtn.setAttribute("aria-pressed", "true");
    B.text(replayBtn, "Pause");
    // Event by event: each step shows the next thing that happened, with its own entrance.
    let k = 0;
    const [a, b] = span();
    const step = () => {
      asOf = k < life.length ? life[k].t : Infinity;
      $("asof").value = Number.isFinite(asOf) ? Math.round(((asOf - a) / (b - a)) * 1000) : 1000;
      render(true);
      drawTrack();
      k++;
      if (k <= life.length) timer = setTimeout(step, B.reducedMotion() ? 1400 : 1100);
      else stopReplay();
    };
    step();
  });

  document.addEventListener("bracken-theme", () => {
    render(false);
    drawTrack();
  });
  window.addEventListener("resize", drawTrack);
  open(ids.includes(location.hash.slice(1)) ? location.hash.slice(1) : ids[0]);
})();
