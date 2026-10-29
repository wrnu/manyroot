// The Tension map (docs/design/visualizer.md section 3.5): contradictions as brambles in a hedgerow.
// Each claim sits in its author's row, in causal order from left to right. A contradiction is a cane
// between the two claims it holds apart: taut and thorned while it's open, hanging slack once it's
// cleared (an end replaced or retracted), faint and cut once its author withdraws it. Green shoots run
// from a claim to what replaced it, and an open divergence is an amber bramble between the competing
// replacements.
//
// Everything drawn comes from one /api/tension response: the replica now, or as of a cursor when the
// map is replayed. Which contradictions are open is `conflicts --json`'s answer, and every status is
// `show --json`'s; the page works none of it out.
"use strict";

(() => {
  const B = Bracken;
  const $ = (id) => document.getElementById(id);
  const S = B.s;
  const HEX = /^[0-9a-f]{64}$/;
  const MAX_CURSOR = 200; // ids in one as_of cursor, so a request head stays small

  let ground = null;
  const agentIndex = new Map(); // agent key -> its index in /api/ground, for its ink and row
  let current = null; // /api/tension now
  let shown = null; // the response drawn: `current`, or the replica as of the stop shown
  let layout = null; // where each claim sits, from `current`, so replaying doesn't move anything
  let stops = []; // the moments the map replays through, in causal order
  let at = -1; // the stop shown, or -1 for now
  let onlyOpen = false;
  let chosen = null; // the key of the chosen spring or fork
  const past = new Map(); // as_of cursor -> response (a promise)

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

  async function loadGround() {
    ground = await api("/api/ground");
    agentIndex.clear();
    ground.agents.forEach((a, i) => agentIndex.set(a.agent, i));
    B.text($("ground-name"), ground.name);
  }

  async function load() {
    await loadGround();
    current = await api("/api/tension");
    rebuild();
    if (at < 0) shown = current;
    draw();
    drawTrack();
    const scroll = $("scroll");
    scroll.scrollLeft = scroll.scrollWidth; // the latest at the right
  }

  const name = (key, fallback) => (agentIndex.has(key) ? B.agentName(ground, agentIndex.get(key)) : fallback || `~${String(key).slice(0, 8)}`);
  const statusOf = (e) => (e.assessment ? e.assessment.status : e.status) || "pending";
  /** A claim's word in this world: its finding or pr tag, else its kind. */
  const wordOf = (e) => (e.tags || []).find((t) => t === "finding" || t === "pr" || t === "task" || t === "decision") || e.kind || "claim";
  /** Every end-like entry in a response, keyed by id, with what replaced each. */
  function endsOf(r) {
    const ends = new Map();
    const add = (e) => e && e.id && !ends.has(e.id) && ends.set(e.id, e);
    for (const sp of r.springs) for (const e of [sp.from, sp.to]) add(e);
    for (const d of r.divergences) [d.target, ...d.successors].forEach(add);
    for (const e of [...ends.values()]) for (const n of e.replaced_by || []) add(n);
    return ends;
  }

  // ---- Layout and stops, from the replica now ----
  function rebuild() {
    const ends = endsOf(current);
    const placed = [...ends.values()].filter((e) => e.seq !== undefined).sort((a, b) => a.seq - b.seq);
    const lanes = [...new Set(placed.map((e) => agentIndex.get(e.author)).filter((i) => i !== undefined))].sort((a, b) => a - b);
    const height = Math.max(240, $("scroll").clientHeight);
    const top = 56, bottom = 40;
    const gap = lanes.length > 1 ? (height - top - bottom) / (lanes.length - 1) : 0;
    const laneY = new Map(lanes.map((a, k) => [a, lanes.length > 1 ? top + k * gap : height / 2]));
    const view = $("scroll").clientWidth || 900;
    const step = Math.max(18, Math.min(70, (view - 80) / Math.max(1, placed.length)));
    const x = new Map(placed.map((e, k) => [e.id, 40 + k * step]));
    layout = { x, laneY, lanes, height, width: Math.max(view, 80 + placed.length * step), seqs: placed.map((e) => [e.seq, e.id]) };

    // The stops: each contradiction (with both its ends), each piece of evidence on an end, and each
    // replacement (with its relation). Each brings in what it names, so its cursor is a causal cut.
    const byKey = new Map();
    const stop = (seq, ids, kind, extra) => {
      if (seq === undefined || seq === null) return;
      const s = byKey.get(seq) || { seq, ids: [], kind, ...extra };
      for (const id of ids) if (id && !s.ids.includes(id)) s.ids.push(id);
      byKey.set(seq, s);
    };
    for (const sp of current.springs) stop(sp.seq, [sp.from.id, sp.to.id, sp.relation], "relation");
    for (const e of ends.values()) {
      for (const ev of e.evidence || []) stop(ev.seq, [e.id, ev.id], "evidence", { verdict: ev.verdict });
      for (const n of e.replaced_by || []) stop(n.seq, [e.id, n.id, n.relation], "claim");
    }
    stops = [...byKey.values()].sort((a, b) => a.seq - b.seq);
    const slider = $("asof");
    slider.max = String(stops.length);
    slider.value = String(at < 0 ? stops.length : Math.min(at, stops.length));
    slider.disabled = !stops.length;
    $("now").disabled = at < 0;
  }

  /** Where a claim sits: its place now, or the left edge for one the map no longer names now (a
   * replacement whose supersession was later retracted, say). */
  function xOf(e) {
    return layout.x.has(e.id) ? layout.x.get(e.id) : 40;
  }
  function yOf(e) {
    const i = agentIndex.get(e.author);
    return layout.laneY.has(i) ? layout.laneY.get(i) : layout.height - 20;
  }

  // ---- Drawing ----
  function draw() {
    const field = $("field");
    const r = shown;
    B.attr(field, "width", layout.width);
    B.attr(field, "height", layout.height);
    B.attr(field, "viewBox", `0 0 ${layout.width} ${layout.height}`);
    drawLanes();

    const springs = r.springs.filter((sp) => !onlyOpen || sp.open);
    const forks = r.divergences;
    const ends = endsOf(r);
    // Which keys light each knot.
    const keysOf = new Map();
    const mark = (id, key) => keysOf.set(id, (keysOf.get(id) || []).concat(key));
    const g = [];
    for (const y of layout.laneY.values()) g.push(S("path", { d: `M0,${y} L${layout.width},${y}`, class: "row" }));

    // Shoots: from a claim to what replaced it.
    const drawn = new Set();
    const shoot = (from, to, key) => {
      const d = arch(xOf(from), yOf(from), xOf(to), yOf(to), -0.1);
      const el = S("path", { d, class: "shoot" });
      el.dataset.key = key;
      g.push(el);
    };
    for (const sp of springs) {
      const key = sp.relation;
      for (const e of [sp.from, sp.to]) {
        mark(e.id, key);
        for (const n of e.replaced_by || []) {
          mark(n.id, key);
          if (!drawn.has(`${e.id}>${n.id}`)) drawn.add(`${e.id}>${n.id}`), shoot(e, ends.get(n.id) || n, key);
        }
      }
    }
    // Forks: the competing replacements of one claim, a bramble between each neighbouring pair.
    for (const d of forks) {
      const key = `fork:${d.target.id}`;
      mark(d.target.id, key);
      for (const s of d.successors) {
        mark(s.id, key);
        if (!drawn.has(`${d.target.id}>${s.id}`)) drawn.add(`${d.target.id}>${s.id}`), shoot(d.target, s, key);
      }
      for (let k = 1; k < d.successors.length; k++) g.push(...cane(d.successors[k - 1], d.successors[k], key, "fork", true));
    }
    // The springs themselves: cleared and withdrawn first, so the open ones lie on top.
    const state = (sp) => (sp.open ? "open" : sp.retracted ? "withdrawn" : "cleared");
    for (const sp of springs.slice().sort((a, b) => (a.open === b.open ? a.seq - b.seq : a.open ? 1 : -1)))
      g.push(...cane(sp.from, sp.to, sp.relation, state(sp), sp.open));
    // Knots, with their evidence as spores above them.
    for (const [id, e] of ends) {
      if (!keysOf.has(id)) continue;
      const x = xOf(e), y = yOf(e);
      const kind = e.kind === "artifact" ? "artifact" : "claim";
      const knot = B.svgKnot(kind, x, y, 6, B.look(kind, { status: statusOf(e), retracted: e.retracted, agent: agentIndex.get(e.author) || 0 }), { class: "knot", tabindex: 0 });
      knot.dataset.keys = keysOf.get(id).join(" ");
      knot.dataset.id = id;
      knot.addEventListener("click", () => choose(keysOf.get(id)[keysOf.get(id).length - 1]));
      knot.addEventListener("keydown", (ev) => (ev.key === "Enter" || ev.key === " ") && (ev.preventDefault(), choose(keysOf.get(id)[keysOf.get(id).length - 1])));
      knot.addEventListener("pointerenter", (ev) => tip(ev, endLine(e)));
      knot.addEventListener("pointerleave", () => tip(null));
      g.push(knot);
      (e.evidence || [])
        .filter((ev) => !ev.retracted)
        .slice(0, 6)
        .forEach((ev, k) => {
          const pass = ev.verdict === "pass";
          g.push(S("circle", { cx: x - 12 + k * 5, cy: y - 13, r: 1.9, fill: pass ? "var(--pass)" : "none", stroke: pass ? "none" : "var(--fail)", "stroke-width": 1.1, opacity: ev.counted ? 1 : 0.4, class: "knot" }));
        });
    }
    // The chosen spring's ends, named.
    const sel = selected(r);
    if (sel)
      for (const e of sel.ends) {
        // Anchored inward near either edge, so a label isn't cut off.
        const x = xOf(e);
        const anchor = x > layout.width - 220 ? "end" : x < 220 ? "start" : "middle";
        g.push(S("text", { x, y: yOf(e) - 22, "text-anchor": anchor, class: "label" }, `${name(e.author, e.author_name)}: ${B.firstLine(e.statement, 40)}`));
      }
    if (!springs.length && !forks.length)
      g.push(S("text", { x: 40, y: layout.height / 2, class: "label" }, onlyOpen ? "Nothing is open: every contradiction here has been cleared." : "No contradictions here."));
    field.replaceChildren(...g);
    field.classList.toggle("lit", !!sel);
    for (const el of field.querySelectorAll("[data-key]")) el.classList.toggle("on", !!sel && el.dataset.key === chosen);
    for (const el of field.querySelectorAll(".knot[data-keys]")) el.classList.toggle("on", !!sel && el.dataset.keys.split(" ").includes(chosen));
    renderSide(r, springs, forks, sel);
    note(r);
  }

  function drawLanes() {
    $("lanes").replaceChildren(
      ...layout.lanes.map((a) => {
        const el = B.h("div", { class: "lane", title: B.agentName(ground, a) }, B.agentName(ground, a));
        el.style.top = `${layout.laneY.get(a)}px`;
        el.style.color = B.ink(a);
        return el;
      })
    );
  }

  /** A quadratic arch from (x0, y0) to (x1, y1), lifted by `lift` of its length (negative: it sags). */
  function arch(x0, y0, x1, y1, lift) {
    const len = Math.hypot(x1 - x0, y1 - y0) || 1;
    const rise = Math.min(140, 18 + Math.abs(lift) * len) * Math.sign(lift || 1);
    const mx = (x0 + x1) / 2, my = (Math.min(y0, y1) + Math.max(y0, y1)) / 2 - rise;
    return `M${x0.toFixed(1)},${y0.toFixed(1)} Q${mx.toFixed(1)},${my.toFixed(1)} ${x1.toFixed(1)},${y1.toFixed(1)}`;
  }

  /** One bramble between two claims: taut and thorned when open, slack otherwise; with a wide hit area. */
  function cane(a, b, key, cls, taut) {
    const x0 = xOf(a), y0 = yOf(a), x1 = xOf(b), y1 = yOf(b);
    const lift = taut ? 0.16 : -0.22;
    const d = arch(x0, y0, x1, y1, lift);
    const out = [];
    const width = taut ? 2 : 1.2;
    const el = S("path", { d, class: `cane ${cls}`, "stroke-width": width, "stroke-dasharray": cls === "withdrawn" ? "2 4" : cls === "cleared" ? "5 4" : null, opacity: taut ? 0.95 : 0.55 });
    el.dataset.key = key;
    out.push(el);
    if (taut) {
      // Thorns along the cane, alternating sides.
      const [mx, my] = d.split("Q")[1].trim().split(" ")[0].split(",").map(Number);
      const len = Math.hypot(x1 - x0, y1 - y0);
      const n = Math.max(2, Math.min(14, Math.floor(len / 24)));
      let path = "";
      for (let k = 1; k <= n; k++) {
        const t = k / (n + 1);
        const px = (1 - t) * (1 - t) * x0 + 2 * (1 - t) * t * mx + t * t * x1;
        const py = (1 - t) * (1 - t) * y0 + 2 * (1 - t) * t * my + t * t * y1;
        let tx = 2 * (1 - t) * (mx - x0) + 2 * t * (x1 - mx), ty = 2 * (1 - t) * (my - y0) + 2 * t * (y1 - my);
        const tl = Math.hypot(tx, ty) || 1;
        tx /= tl;
        ty /= tl;
        const side = k % 2 ? 1 : -1;
        const nx = -ty * side, ny = tx * side;
        path += `M${(px - tx * 2.2).toFixed(1)},${(py - ty * 2.2).toFixed(1)} L${(px + nx * 5 + tx * 1.5).toFixed(1)},${(py + ny * 5 + ty * 1.5).toFixed(1)} L${(px + tx * 2.2).toFixed(1)},${(py + ty * 2.2).toFixed(1)} Z `;
      }
      const thorns = S("path", { d: path, class: "thorn", fill: cls === "fork" ? "var(--focus)" : null });
      thorns.dataset.key = key;
      out.push(thorns);
    }
    const hit = S("path", { d, class: "hit", tabindex: 0, role: "button", "aria-label": "A contradiction; choose it to see both ends" });
    hit.addEventListener("pointerenter", (ev) => tip(ev, springLine(key)));
    hit.addEventListener("pointermove", (ev) => tip(ev, springLine(key)));
    hit.addEventListener("pointerleave", () => tip(null));
    hit.addEventListener("click", () => choose(key));
    hit.addEventListener("keydown", (ev) => (ev.key === "Enter" || ev.key === " ") && (ev.preventDefault(), choose(key)));
    out.push(hit);
    return out;
  }

  // ---- Words ----
  function endLine(e) {
    return [B.bdi(name(e.author, e.author_name)), `'s ${wordOf(e)}: `, B.h("span", { class: "statement" }, B.firstLine(e.statement, 80)), ` · ${statusOf(e)}`];
  }
  function stateOf(sp) {
    if (sp.open) return "open";
    if (sp.retracted) return `withdrawn: ${name(sp.author, sp.author_name)} retracted it`;
    const gone = [sp.from, sp.to].find((e) => e.retracted);
    if (gone) return `cleared: ${name(gone.author, gone.author_name)}'s ${wordOf(gone)} was retracted`;
    const replaced = [sp.from, sp.to].find((e) => (e.replaced_by || []).length);
    if (replaced) return `cleared: ${name(replaced.author, replaced.author_name)}'s ${wordOf(replaced)} was replaced`;
    return "not open";
  }
  function springLine(key) {
    const sp = shown.springs.find((s) => s.relation === key);
    if (sp) return [B.bdi(name(sp.from.author, sp.from.author_name)), `'s ${wordOf(sp.from)} contradicts `, B.bdi(name(sp.to.author, sp.to.author_name)), `'s ${wordOf(sp.to)} · ${stateOf(sp)}`];
    const d = shown.divergences.find((x) => `fork:${x.target.id}` === key);
    if (d) return [`${d.successors.length} competing replacements for `, B.bdi(name(d.target.author, d.target.author_name)), `'s ${wordOf(d.target)}`];
    return [""];
  }

  function tip(ev, parts) {
    const el = $("tip");
    if (!ev) {
      el.hidden = true;
      return;
    }
    el.replaceChildren(...parts);
    el.hidden = false;
    const box = el.parentElement.getBoundingClientRect();
    el.style.left = `${Math.max(8, Math.min(box.width - 380, ev.clientX - box.left + 14))}px`;
    el.style.top = `${ev.clientY - box.top + 14}px`;
  }

  // ---- The side: every contradiction as text, or the chosen one's two ends ----
  function selected(r) {
    if (!chosen) return null;
    const sp = r.springs.find((s) => s.relation === chosen);
    if (sp) return { spring: sp, ends: [sp.from, sp.to] };
    const d = r.divergences.find((x) => `fork:${x.target.id}` === chosen);
    if (d) return { fork: d, ends: [d.target, ...d.successors] };
    return null;
  }

  function choose(key) {
    chosen = chosen === key ? null : key;
    draw();
  }

  function renderSide(r, springs, forks, sel) {
    const side = $("side");
    if (sel) return side.replaceChildren(detail(sel));
    const open = springs.filter((s) => s.open);
    const rest = springs.filter((s) => !s.open);
    const row = (key, cls, ends, why) => {
      const li = B.h("li", { class: `spring ${cls}`, tabindex: 0 }, B.h("span", { class: "ends" }, ...ends), B.h("span", { class: "why" }, why));
      li.addEventListener("click", () => choose(key));
      li.addEventListener("keydown", (ev) => (ev.key === "Enter" || ev.key === " ") && (ev.preventDefault(), choose(key)));
      return li;
    };
    const springRow = (sp) =>
      row(sp.relation, sp.open ? "open" : "", [B.bdi(name(sp.from.author, sp.from.author_name)), `'s ${wordOf(sp.from)} ✕ `, B.bdi(name(sp.to.author, sp.to.author_name)), `'s ${wordOf(sp.to)}: `, B.h("span", { class: "statement" }, B.firstLine(sp.to.statement, 60))], stateOf(sp));
    const forkRow = (d) => row(`fork:${d.target.id}`, "fork", [`${d.successors.length} replacements for `, B.bdi(name(d.target.author, d.target.author_name)), `'s ${wordOf(d.target)}: `, B.h("span", { class: "statement" }, B.firstLine(d.target.statement, 60))], "open: a divergence");
    const list = (items, empty) => B.h("ul", {}, items.length ? items : [B.h("li", { class: "empty" }, empty)]);
    side.replaceChildren(
      B.h("h2", { class: "sc" }, `Open (${open.length + forks.length})`),
      list([...open.slice().reverse().map(springRow), ...forks.map(forkRow)], "Nothing is open."),
      onlyOpen ? null : B.h("h2", { class: "sc" }, `Cleared or withdrawn (${rest.length})`),
      onlyOpen ? null : list(rest.slice().reverse().map(springRow), "None yet.")
    );
  }

  function detail(sel) {
    const card = (e) => {
      const a = e.assessment;
      const counts = a ? ` · ${a.passes.length} counted pass${a.passes.length === 1 ? "" : "es"}, ${a.fails.length} counted fail${a.fails.length === 1 ? "" : "s"}` : "";
      const evidence = (e.evidence || []).map((ev) =>
        B.h("li", { class: ev.retracted ? "gone" : ev.counted ? null : "uncounted" }, B.bdi(name(ev.author, ev.author_name)), ` ${ev.verdict} · `, B.bdi(B.firstLine(ev.method, 50)), ev.retracted ? " · retracted" : ev.counted ? " · counted" : " · not counted")
      );
      const replaced = (e.replaced_by || []).map((n) => B.h("p", { class: "why" }, "replaced by ", B.bdi(name(n.author, n.author_name)), "'s ", B.h("span", { class: "statement" }, B.firstLine(n.statement, 60)), ` (${n.id.slice(0, 12)})`));
      const go = (label, path) => {
        const b = B.h("button", { type: "button" }, label);
        b.addEventListener("click", () => HEX.test(e.id) && location.assign(path));
        return b;
      };
      return B.h(
        "div",
        { class: "end" },
        B.h("div", { class: "kind sc" }, `${wordOf(e)} · `, B.bdi(name(e.author, e.author_name)), ` · ${e.id.slice(0, 12)}`),
        B.h("div", { class: "statement" }, B.firstLine(e.statement, 160)),
        B.h("div", {}, B.h("span", { class: "word" }, statusOf(e)), e.retracted ? " · retracted by its author" : counts),
        replaced,
        evidence.length ? B.h("ul", { class: "evidence" }, evidence) : null,
        B.h("div", { class: "actions" }, go("Follow its life", `/root.html#${e.id}`), go("Show in the Loom", `/#${e.id}`))
      );
    };
    const back = B.h("button", { type: "button" }, "Every contradiction");
    back.addEventListener("click", () => choose(chosen));
    if (sel.spring) {
      const sp = sel.spring;
      return B.h(
        "div",
        { class: "detail" },
        back,
        B.h("h2", { class: "sc" }, B.bdi(name(sp.author, sp.author_name)), " related these as contradicting"),
        B.h("p", { class: "why" }, stateOf(sp), sp.note ? [" · ", B.bdi(sp.note)] : null),
        card(sp.from),
        B.h("p", { class: "cross" }, "✕"),
        card(sp.to)
      );
    }
    const d = sel.fork;
    return B.h("div", { class: "detail" }, back, B.h("h2", { class: "sc" }, "Competing replacements"), B.h("p", { class: "why" }, "Open: more than one current claim replaces this one, and none replaces the others."), card(d.target), ...d.successors.map(card));
  }

  function note(r) {
    const open = r.springs.filter((s) => s.open).length + r.divergences.length;
    const all = r.springs.length;
    B.text($("data-note"), `${open} open · ${all - r.springs.filter((s) => s.open).length} cleared or withdrawn · ${at < 0 ? "live" : `as of step ${at + 1} of ${stops.length}: the replica as it stood then`}`);
  }

  // ---- Replay ----
  /** The cursor for stop k: everything the stops brought in up to it, the latest when that's too many. */
  function cursorFor(k) {
    const ids = new Set();
    for (let j = 0; j <= k; j++) for (const x of stops[j].ids) ids.add(x);
    return [...ids].slice(-MAX_CURSOR).join(",");
  }
  function responseAt(k) {
    if (k < 0 || k >= stops.length) return Promise.resolve(current);
    const cursor = cursorFor(k);
    if (!past.has(cursor)) past.set(cursor, api(`/api/tension?as_of=${encodeURIComponent(cursor)}`));
    return past.get(cursor);
  }

  let asked = 0;
  async function show(k) {
    const ticket = ++asked;
    at = k >= stops.length ? -1 : k;
    $("now").disabled = at < 0;
    $("asof").value = String(at < 0 ? stops.length : at);
    drawTrack();
    $("scroll").classList.toggle("waiting", true);
    if (at >= 0) B.text($("data-note"), "reading the replica as of that step…");
    let r;
    try {
      r = await responseAt(at);
    } catch (e) {
      if (ticket === asked) B.text($("data-note"), `couldn't read that step: ${e.message}`);
      return;
    } finally {
      if (ticket === asked) $("scroll").classList.toggle("waiting", false);
    }
    if (ticket !== asked) return;
    shown = r;
    draw();
    if (at >= 0 && at + 1 < stops.length) responseAt(at + 1).catch(() => {});
  }

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
    const x = (k) => 8 + (k / Math.max(1, stops.length)) * (rect.width - 16);
    ctx.strokeStyle = B.token("--stratum-line");
    ctx.beginPath();
    ctx.moveTo(8, rect.height / 2);
    ctx.lineTo(rect.width - 8, rect.height / 2);
    ctx.stroke();
    // A stop's mark says only what kind of moment it was; how anything stood is the map's to say.
    stops.forEach((s, k) => {
      const look = s.kind === "evidence" ? B.look("evidence", { verdict: s.verdict }) : s.kind === "relation" ? { fill: null, stroke: B.token("--rejected") } : { fill: null, stroke: B.token("--accepted") };
      B.drawKnot(ctx, s.kind, x(k), rect.height / 2, 3.5, look, at >= 0 && k > at ? 0.3 : 1);
    });
    B.text($("asof-label"), at < 0 ? "now" : `step ${at + 1}/${stops.length}`);
  }

  $("asof").addEventListener("input", (e) => show(Number(e.target.value)));
  $("now").addEventListener("click", () => show(stops.length));

  // ---- The toggle ----
  function setOnlyOpen(on) {
    onlyOpen = on;
    B.attr($("all"), "aria-pressed", String(!on));
    B.attr($("open"), "aria-pressed", String(on));
    draw();
  }
  $("all").addEventListener("click", () => setOnlyOpen(false));
  $("open").addEventListener("click", () => setOnlyOpen(true));

  // ---- Live: the map follows the replica while it's shown as it is now ----
  let again = 0;
  function listen() {
    const events = new EventSource(`/api/events?since=${encodeURIComponent(ground.cursor)}`);
    events.addEventListener("contributions", () => {
      clearTimeout(again);
      again = setTimeout(async () => {
        try {
          await loadGround();
          current = await api("/api/tension");
        } catch (_) {
          return;
        }
        rebuild();
        if (at < 0) shown = current;
        draw();
        drawTrack();
      }, 400);
    });
  }

  document.addEventListener("bracken-theme", () => {
    if (shown) draw();
    drawTrack();
  });
  window.addEventListener("resize", () => {
    if (!current) return;
    rebuild();
    draw();
    drawTrack();
  });
  document.querySelector(".masthead nav").append(B.themeButton());
  load().then(listen, (e) => B.text($("data-note"), `couldn't read the Ground: ${e.message}`));
})();
