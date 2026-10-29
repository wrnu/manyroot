// The Loom: agents as roots growing down a root window, each contribution a knot on its author's root,
// woven to its causal parents by hyphae. The core sample on the left is time, and its surface handle
// replays the Ground's growth.
//
// It reads the replica through the view's own API only (docs/design/visualizer.md section 1): the graph
// once, then the event stream's deltas, which name earlier contributions through `refs`. Text is fetched
// only when it's shown, and goes in as text.
"use strict";

(() => {
  const B = Bracken;
  const $ = (id) => document.getElementById(id);
  const TOP = 64; // the agents' band
  const PAD = 28;
  const DENSE = 5000; // above this many visible, knots give way to girth and bundles
  const BULK_TEXT = 3000; // a Ground this small has all its text fetched at once
  const LABELS = 90; // labels show while at most this many subjects are visible
  const FETCHES = 4; // text requests in flight at once
  const CELL = 16; // the spatial index's cell, in CSS pixels
  const HEX = /^[0-9a-f]{64}$/;

  // ---- The Ground, as the page holds it ----
  // One entry per contribution in each column. Each response is in causal order and names only what came
  // before it, so appending keeps parents ahead of their children.
  let ground = null; // /api/ground
  let statuses = {}; // /api/status's statuses
  const ids = [], index = new Map(), kind = [], author = [], parents = [], children = [], time = [], depth = [], tags = [];
  const failed = new Set();
  // Each retraction and its target, in index order: what's retracted is worked out from these (as
  // Graph::all_retracted does), so a retraction that is itself retracted later stops counting.
  const retracts = [];
  let N = 0, cursor = "", tMin = Infinity, tMax = -Infinity, dMax = 0, agentsCount = 0;
  let counts = []; // contributions per agent

  async function api(path) {
    const r = await fetch(path, { headers: { Accept: "application/json" } });
    if (!r.ok) throw new Error(`${path}: ${r.status}`);
    return r.json();
  }

  function reset() {
    for (const col of [ids, kind, author, parents, children, time, depth, tags]) col.length = 0;
    index.clear();
    retracts.length = 0;
    clearCuts();
    failed.clear();
    N = 0;
    tMin = Infinity;
    tMax = -Infinity;
    dMax = 0;
    counts = [];
    born.clear();
    geom.key = "";
  }

  /** Adds one /api/graph response. False when it names something the page doesn't have. */
  function apply(g, grow) {
    const n = g.ids.length;
    const pos = new Array(n + g.refs.length);
    for (let k = 0; k < g.refs.length; k++) {
      const i = index.get(g.refs[k]);
      if (i === undefined) return false;
      pos[n + k] = i;
    }
    const added = [];
    for (let k = 0; k < n; k++) {
      const known = index.get(g.ids[k]);
      if (known !== undefined) {
        pos[k] = known;
        continue;
      }
      const i = N++;
      pos[k] = i;
      index.set(g.ids[k], i);
      ids.push(g.ids[k]);
      added.push(k);
    }
    for (const k of added) {
      const i = pos[k];
      const ps = g.parents[k].map((p) => pos[p]);
      if (ps.some((p) => p === undefined)) return false;
      kind[i] = g.kind[k];
      author[i] = g.author[k];
      parents[i] = ps;
      children[i] = [];
      for (const p of ps) children[p].push(i);
      time[i] = g.time[k];
      depth[i] = g.depth[k];
      tags[i] = g.tags[k] || [];
      tMin = Math.min(tMin, time[i]);
      tMax = Math.max(tMax, time[i]);
      dMax = Math.max(dMax, depth[i]);
      counts[author[i]] = (counts[author[i]] || 0) + 1;
      if (grow && !B.reducedMotion() && !Number.isFinite(state.surface)) born.set(i, performance.now());
    }
    const addedSet = new Set(added.map((k) => pos[k]));
    for (const [p, t] of g.retracts || []) {
      if (pos[t] === undefined) return false;
      if (addedSet.has(pos[p])) retracts.push([pos[p], pos[t]]);
    }
    clearCuts();
    for (const p of g.failed || []) if (pos[p] !== undefined) failed.add(pos[p]);
    cursor = g.cursor;
    return true;
  }

  /** Reads the whole graph again, a page at a time. */
  async function loadAll() {
    reset();
    let since = null;
    for (;;) {
      const g = await api(since === null ? "/api/graph" : `/api/graph?since=${encodeURIComponent(since)}`);
      if (!apply(g, false)) throw new Error("the graph named a contribution it hadn't sent");
      if (!g.more) break;
      since = g.cursor;
    }
  }

  async function loadGround() {
    ground = await api("/api/ground");
    agentsCount = ground.agents.length;
    B.text($("ground-name"), ground.name);
    if (!flag.loaded) loadFlag();
  }

  let statusAsked = null;
  function loadStatus() {
    // One request at a time; a change while one is out asks once more when it ends.
    if (statusAsked) return (statusAsked.again = true);
    statusAsked = { again: false };
    api("/api/status")
      .then((s) => (statuses = s.statuses))
      .catch(() => {})
      .finally(() => {
        const again = statusAsked.again;
        statusAsked = null;
        if (again) loadStatus();
        refreshSide();
        requestDraw();
      });
  }

  // Deltas are applied one at a time, in the order they arrive.
  let work = Promise.resolve();
  function serially(fn) {
    work = work.then(fn).catch((e) => console.error(e));
  }

  function onContributions(event) {
    const d = JSON.parse(event.data);
    serially(async () => {
      const before = cursor;
      if (d.graph.author.some((a) => a >= agentsCount)) await loadGround();
      if (!apply(d.graph, true)) await loadAll();
      if (d.status_changed === null || Object.keys(d.status_changed).length) loadStatus();
      if (N <= BULK_TEXT) bulkTexts(before);
      buildAgents();
      layout();
      note();
    });
  }

  let stream = null;
  function listen() {
    stream = new EventSource(`/api/events?since=${encodeURIComponent(cursor)}`);
    stream.addEventListener("contributions", onContributions);
    stream.addEventListener("open", () => note());
    stream.addEventListener("error", () => {
      // Reconnect by hand, so the cursor sent is the latest, not the one the stream opened with.
      stream.close();
      note("reconnecting…");
      setTimeout(() => serially(listen), 3000);
    });
  }

  function note(extra) {
    const parts = [`${N.toLocaleString("en")} contributions`, `${agentsCount} agents`, extra || "live"];
    if (Number.isFinite(state.surface)) parts.push("scrubbed back: retractions as they stood, status only now");
    B.text($("data-note"), parts.join(" · "));
  }

  // ---- Text, fetched only when shown ----
  const texts = new Map(); // id -> one line about the contribution
  const asked = new Set();
  const queue = [];
  let fetching = 0;

  function summary(c) {
    const b = c.body;
    switch (c.kind) {
      case "claim":
        return b.statement;
      case "artifact":
        return b.name;
      case "relation":
        return `${b.kind.replace(/_/g, " ")} ${b.to.slice(0, 8)}`;
      case "evidence":
        return `${b.verdict}: ${b.method}`;
      case "retract":
        return `retracts ${b.target.slice(0, 8)}${b.reason ? ": " + b.reason : ""}`;
      default:
        return "";
    }
  }

  function textOf(i) {
    const t = texts.get(ids[i]);
    if (t === undefined) need(i);
    return t;
  }

  function need(i) {
    const id = ids[i];
    if (texts.has(id) || asked.has(id)) return;
    asked.add(id);
    queue.push(id);
    pump();
  }

  function pump() {
    while (fetching < FETCHES && queue.length) {
      const id = queue.pop(); // the latest asked is the likeliest still on screen
      fetching++;
      api(`/api/contribution/${id}`)
        .then((c) => texts.set(id, summary(c)))
        .catch(() => asked.delete(id))
        .finally(() => {
          fetching--;
          pump();
          textsArrived();
        });
    }
  }

  /** Every contribution's text since a cursor, a page at a time: for small Grounds, so labels show at once. */
  async function bulkTexts(since) {
    let at = since;
    for (;;) {
      const q = at ? `?since=${encodeURIComponent(at)}&limit=1000` : "?limit=1000";
      let page;
      try {
        page = await api(`/api/contributions${q}`);
      } catch (_) {
        return;
      }
      for (const c of page.contributions) texts.set(c.id, summary(c));
      if (!page.more) break;
      at = page.cursor;
    }
    textsArrived();
  }

  let textsPending = false;
  function textsArrived() {
    if (textsPending) return;
    textsPending = true;
    requestAnimationFrame(() => {
      textsPending = false;
      if (state.hover >= 0) showHover(state.hover);
      refreshSide();
      draw();
    });
  }

  // ---- "You left here": the cursor the reader last had, in this browser only ----
  const flag = { loaded: false, heads: [] };
  const flagKey = () => `bracken-left:${ground.ground}`;
  function loadFlag() {
    flag.loaded = true;
    let saved = null;
    try {
      saved = localStorage.getItem(flagKey());
    } catch (_) {}
    flag.saved = saved ? saved.split(",").filter((h) => HEX.test(h)) : [];
  }
  function flagHeads() {
    return (flag.saved || []).map((h) => index.get(h)).filter((i) => i !== undefined);
  }
  function flagValue(axis = state.axis) {
    const heads = flagHeads();
    return heads.length ? Math.max(...heads.map((i) => valueOf(i, axis))) : null;
  }
  function saveFlag() {
    if (!ground || !cursor) return;
    try {
      localStorage.setItem(flagKey(), cursor);
    } catch (_) {}
  }
  window.addEventListener("pagehide", saveFlag);
  document.addEventListener("visibilitychange", () => document.visibilityState === "hidden" && saveFlag());

  // ---- State and geometry ----
  const state = { axis: "time", surface: Infinity, hover: -1, selected: -1, view: null, playing: false, speed: 1, morph: 1 };
  const canvas = $("loom-canvas");
  const ctx = canvas.getContext("2d");
  const gauge = $("gauge-canvas");
  const gctx = gauge.getContext("2d");
  let W = 0, H = 0, GW = 0, GH = 0, dpr = 1;
  let colX = [];
  let fromAxis = null; // the previous axis while the vertical axis morphs

  const valueOf = (i, axis = state.axis) => (axis === "time" ? time[i] : depth[i]);
  function range(axis = state.axis) {
    if (!N) return [0, 1];
    return axis === "time" ? [tMin, tMax] : [0, dMax];
  }
  function view() {
    return state.view || range();
  }
  const ease = (t) => 1 - Math.pow(1 - t, 3);
  const yAt = (v, [a, b]) => TOP + PAD + ((v - a) / Math.max(1, b - a)) * (H - TOP - PAD * 2);
  function rootX(a, y) {
    const amp = Math.min(9, (W / Math.max(1, agentsCount)) * 0.07);
    return colX[a] + amp * Math.sin(y * 0.011 + a * 1.7) + amp * 0.45 * Math.sin(y * 0.031 + a * 0.6);
  }

  // Every knot's place, worked out when the geometry changes (size, axis, zoom, the Ground growing),
  // never on a hover or a replay frame, and the spatial index with it.
  const geom = { key: "", x: new Float32Array(0), y: new Float32Array(0), grid: new Map() };
  const X = (i) => geom.x[i];
  const Y = (i) => geom.y[i];
  function place() {
    const morphing = state.morph < 1 && fromAxis;
    const key = `${W}x${H}:${state.axis}:${view().join(",")}:${N}:${agentsCount}`;
    if (!morphing && geom.key === key) return;
    if (geom.x.length < N) {
      geom.x = new Float32Array(Math.max(N, geom.x.length * 2, 1024));
      geom.y = new Float32Array(geom.x.length);
    }
    const v = view(), from = morphing ? range(fromAxis) : null, t = morphing ? ease(state.morph) : 1;
    for (let i = 0; i < N; i++) {
      let y = yAt(valueOf(i), v);
      if (morphing) {
        const y0 = yAt(valueOf(i, fromAxis), from);
        y = y0 + (y - y0) * t;
      }
      geom.y[i] = y;
      geom.x[i] = rootX(author[i], y);
    }
    if (morphing) {
      geom.key = ""; // mid-morph places aren't kept, and nothing is hit-tested until it settles
      geom.grid = new Map();
      return;
    }
    geom.key = key;
    const grid = new Map();
    for (let i = 0; i < N; i++) {
      const y = geom.y[i];
      if (y < TOP - CELL || y > H + CELL) continue;
      const k = Math.floor(geom.x[i] / CELL) * 65536 + Math.floor(y / CELL);
      const cell = grid.get(k);
      if (cell) cell.push(i);
      else grid.set(k, [i]);
    }
    geom.grid = grid;
  }

  function hit(x, y) {
    let best = -1, bd = 14 * 14;
    const dense = visibleCount > DENSE;
    for (let gx = Math.floor(x / CELL) - 1; gx <= Math.floor(x / CELL) + 1; gx++)
      for (let gy = Math.floor(y / CELL) - 1; gy <= Math.floor(y / CELL) + 1; gy++)
        for (const i of geom.grid.get(gx * 65536 + gy) || []) {
          if (valueOf(i) > state.surface) continue;
          if (dense && kind[i] !== "claim" && kind[i] !== "artifact") continue;
          const d = (X(i) - x) ** 2 + (Y(i) - y) ** 2;
          if (d < bd) (bd = d), (best = i);
        }
    return best;
  }

  function layout() {
    dpr = window.devicePixelRatio || 1;
    const r = canvas.getBoundingClientRect();
    W = r.width;
    H = r.height;
    canvas.width = Math.round(W * dpr);
    canvas.height = Math.round(H * dpr);
    const gr = gauge.getBoundingClientRect();
    GW = gr.width;
    GH = gr.height;
    gauge.width = Math.round(GW * dpr);
    gauge.height = Math.round(GH * dpr);
    const left = 56, right = Math.min(200, W * 0.16);
    const span = (W - left - right) / Math.max(1, agentsCount - 1 || 1);
    colX = Array.from({ length: agentsCount }, (_, a) => (agentsCount === 1 ? W / 2 : left + a * span));
    // Each heading is as wide as its text, so long as it stays clear of its neighbours: two neighbours
    // split the space between their centres, and a short heading leaves the rest to the one beside it.
    const natural = agentEls.map(
      (el) => Math.max(el.querySelector(".heading").getBoundingClientRect().width, el.querySelector(".count").getBoundingClientRect().width) + 2
    );
    const gap = 8;
    for (const [a, el] of agentEls.entries()) {
      const room = (b) => {
        if (b < 0) return colX[a];
        if (b >= agentsCount) return W - colX[a];
        const between = Math.abs(colX[a] - colX[b]) - gap;
        return between - Math.min(natural[b] / 2, between / 2);
      };
      const width = Math.min(natural[a], 2 * Math.min(room(a - 1), room(a + 1)));
      el.style.left = `${colX[a]}px`;
      el.style.setProperty("--col", `${Math.max(32, width)}px`);
      B.attr(el, "title", B.agentName(ground, a));
    }
    geom.key = "";
    draw();
  }

  // ---- The agents' band ----
  let agentEls = [];
  function buildAgents() {
    const band = $("agents");
    band.replaceChildren();
    const heads = B.headings(ground.agents.map((_, i) => B.agentName(ground, i)));
    agentEls = ground.agents.map((a, i) => {
      const el = B.h("div", { class: "agent" }, B.h("span", { class: "heading" }, heads[i]), B.h("span", { class: "count num" }, (counts[i] || 0).toLocaleString("en")));
      el.style.setProperty("--agent-ink", B.ink(i));
      band.append(el);
      return el;
    });
  }

  // What's retracted at a surface: only retractions at or above it count. Walking the retractions from
  // the last decides each one before its target, since a retraction comes after what it retracts.
  // Kept for now and for the current surface; cleared when the Ground grows.
  const cuts = { now: null, at: null, axis: "", surface: 0 };
  function clearCuts() {
    cuts.now = cuts.at = null;
  }
  function cutAt(surface) {
    const marks = new Uint8Array(N);
    for (let k = retracts.length - 1; k >= 0; k--) {
      const [r, t] = retracts[k];
      if (!marks[r] && valueOf(r) <= surface) marks[t] = 1;
    }
    return marks;
  }
  function retractedAt(surface) {
    if (!Number.isFinite(surface)) return cuts.now || (cuts.now = cutAt(Infinity));
    if (!cuts.at || cuts.surface !== surface || cuts.axis !== state.axis) {
      cuts.at = cutAt(surface);
      cuts.surface = surface;
      cuts.axis = state.axis;
    }
    return cuts.at;
  }
  const isRetracted = (i, surface = state.surface) => retractedAt(surface)[i] === 1;
  const behind = () => Number.isFinite(state.surface);
  /** A subject's status now, whatever the surface. */
  const statusNow = (i) => {
    if (isRetracted(i, Infinity)) return "retracted";
    const s = statuses[ids[i]];
    return s ? s.status : "pending";
  };
  // Behind now, only retractions are shown as they stood: status needs the policy over the Ground at
  // that cursor, which the page doesn't have, so knots draw neutral and the label says "now".
  const statusOf = (i) => (isRetracted(i) ? "retracted" : behind() ? "pending" : statusNow(i));
  const verdictOf = (i) => (kind[i] !== "evidence" ? null : failed.has(i) ? "fail" : "pass");
  const lookOf = (i) => B.look(kind[i], { status: statusOf(i), verdict: verdictOf(i), retracted: isRetracted(i), agent: author[i] });

  // ---- Drawing ----
  let ancestry = null; // the hovered knot's ancestors, as marks in `mark` equal to `markGen`
  const mark = { gen: 0, of: new Int32Array(0) };
  const inAncestry = (i) => ancestry !== null && mark.of[i] === mark.gen;
  const born = new Map(); // knots that appeared while growing: index -> when they appeared
  let visibleCount = 0;

  function visible() {
    const [a, b] = view();
    const lo = a - (b - a) * 0.05, hi = b + (b - a) * 0.05;
    const out = [];
    for (let i = 0; i < N; i++) {
      const v = valueOf(i);
      if (v > state.surface || v < lo || v > hi) continue;
      out.push(i);
    }
    return out;
  }

  let lastDrawMs = 0;
  function draw() {
    if (!W || !ground) return;
    const t0 = performance.now();
    place();
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, W, H);
    const vis = visible();
    visibleCount = vis.length;
    const dense = vis.length > DENSE;
    const r = Math.max(1.6, Math.min(5.5, Math.sqrt((W * (H - TOP)) / Math.max(1, vis.length) / 40)));
    drawStrata();
    drawRoots(vis, dense);
    drawHyphae(vis, dense, false);
    if (!dense) drawKnots(vis, r);
    else drawBlooms(vis);
    if (ancestry) {
      ctx.fillStyle = B.token("--ground");
      ctx.globalAlpha = 0.72;
      ctx.fillRect(0, TOP, W, H - TOP);
      ctx.globalAlpha = 1;
      // A late knot's ancestry can be most of the Ground: past the same limit, it's drawn dense too.
      const anc = vis.filter(inAncestry);
      if (anc.length > DENSE) {
        drawRoots(anc, true);
        drawHyphae(anc, true, false);
        drawBlooms(anc);
      } else {
        drawAncestorRoots(anc);
        drawHyphae(anc, false, true);
        drawKnots(anc, Math.max(r, 3.2));
      }
    }
    drawSurface();
    drawLabels(vis, r, dense);
    drawGauge();
    lastDrawMs = performance.now() - t0;
    canvas.dataset.drawMs = lastDrawMs.toFixed(1);
  }

  function drawStrata() {
    // Hour (or day) bands, as faint soil strata behind everything.
    if (state.axis !== "time") return;
    const [a, b] = view();
    if (b <= a) return;
    const step = b - a > 3 * 86400000 ? 86400000 : b - a > 6 * 3600000 ? 3600000 * 2 : 3600000;
    ctx.fillStyle = B.token("--stratum");
    ctx.globalAlpha = 0.35;
    for (let t = Math.floor(a / step) * step; t < b; t += step) {
      if (Math.floor(t / step) % 2) continue;
      const y0 = yAt(t, [a, b]);
      const y1 = yAt(t + step, [a, b]);
      ctx.fillRect(0, Math.max(TOP, y0), W, Math.max(0, y1 - Math.max(TOP, y0)));
    }
    ctx.globalAlpha = 1;
  }

  function drawRoots(vis, dense) {
    // Each agent's root runs from its first visible knot to its last, thicker where it's busiest.
    const bins = Math.max(1, Math.floor((H - TOP) / 5));
    const hist = new Float32Array(agentsCount * bins);
    const y0 = new Float32Array(agentsCount).fill(Infinity), y1 = new Float32Array(agentsCount).fill(-Infinity);
    for (const i of vis) {
      const a = author[i], y = Y(i);
      if (y < y0[a]) y0[a] = y;
      if (y > y1[a]) y1[a] = y;
      hist[a * bins + Math.max(0, Math.min(bins - 1, Math.floor(((y - TOP) / (H - TOP)) * bins)))]++;
    }
    ctx.lineCap = "round";
    for (let a = 0; a < agentsCount; a++) {
      if (y0[a] === Infinity) continue;
      ctx.strokeStyle = B.ink(a);
      ctx.globalAlpha = 0.85;
      if (!dense) {
        ctx.lineWidth = 1.5;
        ctx.beginPath();
        ctx.moveTo(rootX(a, y0[a]), y0[a]);
        for (let y = y0[a] + 3; y < y1[a]; y += 3) ctx.lineTo(rootX(a, y), y);
        ctx.lineTo(rootX(a, y1[a]), y1[a]);
        ctx.stroke();
      } else {
        // The density reads as the root's girth.
        let max = 0;
        for (let b = 0; b < bins; b++) max = Math.max(max, hist[a * bins + b]);
        let prev = null;
        for (let b = 0; b < bins; b++) {
          const y = TOP + ((b + 0.5) / bins) * (H - TOP);
          if (y < y0[a] || y > y1[a]) continue;
          if (prev !== null) {
            ctx.lineWidth = 0.8 + 6 * Math.sqrt(hist[a * bins + b] / Math.max(1, max));
            ctx.beginPath();
            ctx.moveTo(rootX(a, prev), prev);
            ctx.lineTo(rootX(a, y), y);
            ctx.stroke();
          }
          prev = y;
        }
      }
      // The root cap at the growing tip.
      ctx.globalAlpha = 1;
      ctx.beginPath();
      ctx.arc(rootX(a, y1[a]), y1[a] + 2.5, 1.6, 0, Math.PI * 2);
      ctx.fillStyle = B.ink(a);
      ctx.fill();
    }
    ctx.globalAlpha = 1;
  }

  function drawAncestorRoots(anc) {
    // The hovered knot's lineage, bright along each root it passes through.
    ctx.lineWidth = 2.4;
    ctx.lineCap = "round";
    for (const i of anc)
      for (const p of parents[i]) {
        if (author[p] !== author[i] || !inAncestry(p)) continue;
        ctx.strokeStyle = B.ink(author[i]);
        ctx.beginPath();
        const ya = Y(p), yb = Y(i);
        ctx.moveTo(X(p), ya);
        for (let y = ya + 3; y < yb; y += 3) ctx.lineTo(rootX(author[i], y), y);
        ctx.lineTo(X(i), yb);
        ctx.stroke();
      }
  }

  function hyphaPath(x0, y0, x1, y1) {
    const dy = Math.max(26, (y1 - y0) * 0.55);
    ctx.moveTo(x0, y0);
    ctx.bezierCurveTo(x0, y0 + dy, x1, y1 - dy, x1, y1);
  }

  function drawHyphae(vis, dense, lit) {
    // Causal links between agents. One agent's links to its own earlier work run along its root.
    if (!dense) {
      ctx.lineWidth = lit ? 1.6 : 1;
      if (!lit) {
        ctx.strokeStyle = B.token("--hypha");
        ctx.beginPath();
      }
      for (const i of vis)
        for (const p of parents[i]) {
          if (author[p] === author[i] || valueOf(p) > state.surface) continue;
          if (lit) {
            if (!inAncestry(p)) continue;
            ctx.strokeStyle = B.ink(author[p]);
            ctx.beginPath();
            hyphaPath(X(p), Y(p), X(i), Y(i));
            ctx.stroke();
          } else hyphaPath(X(p), Y(p), X(i), Y(i));
        }
      if (!lit) ctx.stroke();
      return;
    }
    // At density, links bundle: one strand per pair of agents per band of the window, weighted by its count,
    // so the paths the work actually takes stand out and a uniform mesh doesn't.
    const band = Math.max(14, (H - TOP) / 70);
    const bundles = new Map();
    for (const i of vis)
      for (const p of parents[i]) {
        if (author[p] === author[i] || valueOf(p) > state.surface) continue;
        const key = (author[p] * 256 + author[i]) * 4096 + Math.floor(Y(i) / band);
        let e = bundles.get(key);
        if (!e) bundles.set(key, (e = { a: author[p], b: author[i], y0: 0, y1: 0, n: 0 }));
        e.y0 += Y(p);
        e.y1 += Y(i);
        e.n++;
      }
    const list = [...bundles.values()].sort((p, q) => p.n - q.n);
    const max = list.length ? list[list.length - 1].n : 1;
    ctx.strokeStyle = B.token("--hypha-strong");
    for (const e of list) {
      const w = e.n / max;
      ctx.globalAlpha = 0.03 + 0.55 * Math.pow(w, 1.4);
      ctx.lineWidth = 0.5 + 2.4 * Math.sqrt(w);
      const y0 = e.y0 / e.n, y1 = e.y1 / e.n;
      ctx.beginPath();
      hyphaPath(rootX(e.a, y0), y0, rootX(e.b, y1), y1);
      ctx.stroke();
    }
    ctx.globalAlpha = 1;
  }

  function drawKnots(vis, r) {
    const now = performance.now();
    for (const i of vis) {
      const k = kind[i];
      const small = k === "relation" || k === "evidence" || k === "retract";
      let rr = small ? r * 0.72 : r;
      const b = born.get(i);
      if (b !== undefined && !B.reducedMotion()) {
        const t = Math.min(1, (now - b) / 420);
        rr *= 0.25 + 0.75 * ease(t);
        if (t >= 1) born.delete(i);
      }
      B.drawKnot(ctx, k, X(i), Y(i), rr, lookOf(i), 1);
      if (i === state.hover || i === state.selected) {
        ctx.beginPath();
        ctx.arc(X(i), Y(i), rr * 2.6, 0, Math.PI * 2);
        ctx.strokeStyle = B.token("--focus");
        ctx.lineWidth = 1.5;
        ctx.stroke();
      }
    }
    if (born.size) requestDraw();
  }

  function drawBlooms(vis) {
    // At density, only what the policy settled shows as a point: accepted green, rejected rust.
    const accepted = B.token("--accepted"), rejected = B.token("--rejected");
    for (const i of vis) {
      if (kind[i] !== "claim" && kind[i] !== "artifact") continue;
      const st = statusOf(i);
      if (st !== "accepted" && st !== "rejected") continue;
      ctx.fillStyle = st === "accepted" ? accepted : rejected;
      ctx.fillRect(X(i) - 1, Y(i) - 1, 2, 2);
    }
    for (const i of [state.hover, state.selected]) {
      if (i < 0 || i >= N || valueOf(i) > state.surface) continue;
      ctx.beginPath();
      ctx.arc(X(i), Y(i), 6, 0, Math.PI * 2);
      ctx.strokeStyle = B.token("--focus");
      ctx.lineWidth = 1.5;
      ctx.stroke();
    }
  }

  function drawSurface() {
    // Below the replay surface, the soil is unturned: nothing has grown there yet.
    if (!Number.isFinite(state.surface)) return;
    const v = view();
    if (state.surface >= v[1]) return;
    const y = yAt(state.surface, v);
    ctx.strokeStyle = B.token("--root-faint");
    ctx.setLineDash([2, 4]);
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(0, y);
    ctx.lineTo(W, y);
    ctx.stroke();
    ctx.setLineDash([]);
  }

  // Labels for claims and artifacts, tied to their knots, only while there are few enough to read.
  function drawLabels(vis, r, dense) {
    const box = $("labels");
    const subjects = dense ? [] : vis.filter((i) => kind[i] === "claim" || kind[i] === "artifact");
    if (subjects.length > LABELS || !subjects.length) {
      if (box.firstChild) box.replaceChildren();
      return;
    }
    const lastY = new Map();
    const colW = agentsCount > 1 ? colX[1] - colX[0] : W;
    const els = [];
    for (const i of subjects) {
      const y = Y(i) - TOP;
      const a = author[i];
      if ((lastY.get(a) ?? -Infinity) > y - 15) continue;
      const isIntent = tags[i].includes("intent");
      const t = isIntent ? "intends to work on it" : textOf(i);
      if (t === undefined) continue;
      lastY.set(a, y);
      const el = B.h("div", { class: ancestry && !inAncestry(i) ? "knot-label dim" : "knot-label" }, B.firstLine(t, 70));
      el.style.left = `${X(i) + r + 6}px`;
      el.style.top = `${y - 8}px`;
      el.style.maxWidth = `${Math.max(80, Math.min(260, colW * 1.45, W - X(i) - r - 14))}px`;
      els.push(el);
    }
    box.replaceChildren(...els);
  }

  // ---- The core sample ----
  const fibres = { key: "", hist: new Float32Array(0), max: 0 };
  function drawGauge() {
    gctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    gctx.clearRect(0, 0, GW, GH);
    const [a, b] = range();
    const y = (v) => TOP + PAD + ((v - a) / Math.max(1, b - a)) * (GH - TOP - PAD * 2);
    // Strata and their times.
    gctx.font = `500 11px "Alegreya Sans SC", sans-serif`;
    gctx.textBaseline = "middle";
    if (state.axis === "time" && b > a) {
      const step = b - a > 3 * 86400000 ? 86400000 * Math.ceil((b - a) / 86400000 / 12) : b - a > 6 * 3600000 ? 7200000 : 3600000;
      for (let t = Math.ceil(a / step) * step; t <= b; t += step) {
        gctx.fillStyle = B.token("--stratum-line");
        gctx.fillRect(GW - 14, y(t), 14, 1);
        gctx.fillStyle = B.token("--root-faint");
        gctx.fillText(step >= 86400000 ? B.day(t) : B.clock(t), 8, y(t));
      }
    } else if (state.axis !== "time") {
      gctx.fillStyle = B.token("--root-faint");
      for (let d = 0, step = Math.max(1, Math.round(b / 10)); d <= b; d += step) gctx.fillText(`step ${d}`, 8, y(d));
    }
    // The density of the whole history, as fibres in the core: counted again only when the Ground or the
    // axis changes.
    const bins = Math.max(1, Math.floor((GH - TOP - PAD * 2) / 3));
    const key = `${bins}:${state.axis}:${N}`;
    if (fibres.key !== key) {
      fibres.key = key;
      fibres.hist = new Float32Array(bins);
      for (let i = 0; i < N; i++) fibres.hist[Math.min(bins - 1, Math.floor(((valueOf(i) - a) / Math.max(1, b - a)) * bins))]++;
      fibres.max = fibres.hist.reduce((m, c) => Math.max(m, c), 0);
    }
    gctx.fillStyle = B.token("--root-dim");
    gctx.globalAlpha = 0.55;
    for (let k = 0; k < bins; k++) {
      const w = 22 * Math.sqrt(fibres.hist[k] / Math.max(1, fibres.max));
      gctx.fillRect(GW - 16 - w, TOP + PAD + (k / bins) * (GH - TOP - PAD * 2), w, 2);
    }
    gctx.globalAlpha = 1;
    // The window's extent, when zoomed.
    if (state.view) {
      const [va, vb] = state.view;
      gctx.strokeStyle = B.token("--root-dim");
      gctx.strokeRect(2.5, y(va), GW - 5, y(vb) - y(va));
    }
    // The handle and the flag.
    const surface = Number.isFinite(state.surface) ? Math.min(state.surface, b) : b;
    $("surface").style.top = `${y(surface)}px`;
    B.attr($("surface"), "aria-valuenow", String(Math.round(((surface - a) / Math.max(1, b - a)) * 1000)));
    B.attr($("surface"), "aria-valuetext", state.axis === "time" ? `${B.day(surface)} ${B.clock(surface)}` : `causal step ${surface}`);
    B.text($("surface-time"), state.axis === "time" ? B.clock(surface) : `step ${surface}`);
    const fv = flagValue();
    $("flag").hidden = fv === null;
    $("to-flag").disabled = fv === null;
    if (fv !== null) $("flag").style.top = `${y(fv)}px`;
  }

  // ---- Interaction ----
  let pending = false;
  function requestDraw() {
    if (pending) return;
    pending = true;
    requestAnimationFrame(() => {
      pending = false;
      draw();
    });
  }

  /** Marks `i` and everything it was woven onto. */
  function markAncestors(i) {
    if (mark.of.length < N) mark.of = new Int32Array(Math.max(N, mark.of.length * 2, 1024));
    const gen = ++mark.gen;
    mark.of[i] = gen;
    const stack = [i];
    while (stack.length)
      for (const p of parents[stack.pop()])
        if (mark.of[p] !== gen) {
          mark.of[p] = gen;
          stack.push(p);
        }
  }

  const win = $("window");
  win.addEventListener("pointermove", (e) => {
    if (!geom.key) return; // mid-morph
    const r = canvas.getBoundingClientRect();
    const i = hit(e.clientX - r.left, e.clientY - r.top);
    if (i === state.hover) return;
    const t0 = performance.now();
    state.hover = i;
    if (i >= 0) markAncestors(i);
    canvas.dataset.hoverMs = (performance.now() - t0).toFixed(1);
    ancestry = i >= 0 ? i : null;
    showHover(i);
    canvas.style.cursor = i >= 0 ? "pointer" : "default";
    requestDraw();
  });
  win.addEventListener("pointerleave", () => {
    state.hover = -1;
    ancestry = null;
    showHover(-1);
    requestDraw();
  });
  win.addEventListener("click", () => select(state.hover));
  win.addEventListener(
    "wheel",
    (e) => {
      e.preventDefault();
      const [a, b] = view();
      const [ra, rb] = range();
      const r = canvas.getBoundingClientRect();
      if (e.ctrlKey || e.metaKey) {
        const at = a + ((e.clientY - r.top - TOP - PAD) / (H - TOP - PAD * 2)) * (b - a);
        const f = Math.exp(e.deltaY * 0.0025);
        const na = at - (at - a) * f, nb = at + (b - at) * f;
        if (nb - na >= rb - ra) state.view = null;
        else state.view = [Math.max(ra, na), Math.min(rb, nb)];
      } else if (state.view) {
        const d = (e.deltaY / (H - TOP)) * (b - a);
        const na = Math.max(ra, Math.min(rb - (b - a), a + d));
        state.view = [na, na + (b - a)];
      }
      requestDraw();
    },
    { passive: false }
  );

  function showHover(i) {
    const el = $("hover-label");
    const tether = $("tether");
    tether.replaceChildren();
    if (i < 0 || !geom.key) {
      el.hidden = true;
      return;
    }
    const who = [`${B.KIND_WORD[kind[i]]} · `, B.bdi(B.agentName(ground, author[i])), ` · ${state.axis === "time" ? B.clock(time[i]) : "step " + depth[i]}`];
    const t = textOf(i);
    el.replaceChildren(B.h("div", { class: "who sc" }, who), B.h("div", { class: "what" }, t === undefined ? "…" : B.firstLine(t, 140)));
    el.hidden = false;
    const kx = X(i), ky = Y(i);
    const left = kx > W - 380 ? kx - 360 : kx + 26;
    const top = Math.max(TOP + 6, Math.min(H - 90, ky - 46));
    el.style.left = `${left}px`;
    el.style.top = `${top}px`;
    // The tether: a hairline stitch from the label to its knot.
    tether.append(B.s("path", { d: `M${kx},${ky} L${left < kx ? left + 340 : left},${top + 18}`, stroke: B.token("--root-faint"), "stroke-width": 1, fill: "none" }));
  }

  // ---- The side: the specimen label, from /api/contribution/<id> ----
  let shown = null; // the selected contribution's /api/contribution, once it arrives
  function select(i) {
    state.selected = i;
    shown = null;
    refreshSide();
    requestDraw();
    if (i < 0) return;
    const id = ids[i];
    api(`/api/contribution/${id}`)
      .then((c) => {
        texts.set(id, summary(c));
        if (ids[state.selected] === id) (shown = c), refreshSide();
      })
      .catch(() => {});
  }

  function body(c) {
    // The whole text of a contribution's body, for the label.
    const b = c.body;
    switch (c.kind) {
      case "claim":
        return [b.statement];
      case "artifact":
        return [b.name];
      case "relation":
        return [`${b.from.slice(0, 12)} ${b.kind.replace(/_/g, " ")} ${b.to.slice(0, 12)}`, b.note];
      case "evidence":
        return [[`${b.verdict} via `, B.bdi(b.method)], b.details];
      case "retract":
        return [`retracts ${b.target.slice(0, 12)}`, b.reason];
      default:
        return [];
    }
  }

  function refreshSide() {
    const side = $("side");
    const i = state.selected;
    if (i < 0 || i >= N) {
      side.replaceChildren(B.h("p", { class: "side-hint" }, "Hover a knot to light its ancestry. Click it for its label."));
      return;
    }
    if (valueOf(i) > state.surface) {
      // Scrubbed back past the selected knot: it isn't written yet here. It comes back further on.
      side.replaceChildren(B.h("p", { class: "side-hint" }, "The selected knot isn't written yet at this point. Scrub forward to see its label."));
      return;
    }
    const id = ids[i];
    const c = shown && shown.id === id ? shown : null;
    const subject = kind[i] === "claim" || kind[i] === "artifact";
    const cut = isRetracted(i);
    const st = !behind() ? statusOf(i) : `${cut ? "retracted here · " : ""}${statusNow(i)} now`;
    const evidence = behind() ? "evidence now" : "evidence";
    const s = statuses[id];
    const a = c && c.assessment;
    const lines = c ? body(c).filter(Boolean) : [textOf(i) ?? "…"];
    const name = c && c.author_name ? c.author_name : B.agentName(ground, author[i]);
    const card = B.h(
      "article",
      { class: "label-card" },
      B.h("div", { class: "kind sc" }, B.knotIcon(kind[i], lookOf(i)), `${B.KIND_WORD[kind[i]]} · ${subject ? st : cut ? "retracted" : verdictOf(i) || "woven"}`),
      B.h("p", { class: "statement" }, lines[0]),
      lines.slice(1).map((l) => B.h("p", { class: "detail" }, l)),
      B.h(
        "dl",
        {},
        B.h("dt", {}, "author"), B.h("dd", {}, name),
        B.h("dt", {}, "written"), B.h("dd", { class: "num" }, `${B.day(time[i])} ${B.clock(time[i])} UTC (their clock)`),
        B.h("dt", {}, "causal step"), B.h("dd", { class: "num" }, String(depth[i])),
        B.h("dt", {}, "id"), B.h("dd", { class: "id" }, id),
        c && c.kind === "artifact" && c.body.media_type ? [B.h("dt", {}, "type"), B.h("dd", {}, c.body.media_type)] : null,
        a ? [B.h("dt", {}, evidence), B.h("dd", {}, `${a.passes.length} passing, ${a.fails.length} failing`)]
          : s ? [B.h("dt", {}, evidence), B.h("dd", {}, `${s.passes} passing, ${s.fails} failing`)] : null,
        a && a.superseded_by && a.superseded_by.length ? [B.h("dt", {}, "replaced by"), B.h("dd", { class: "id" }, a.superseded_by.map((x) => x.slice(0, 12)).join(", "))] : null,
        tags[i].length ? [B.h("dt", {}, "tags"), B.h("dd", {}, tags[i].map((t, k) => [k ? ", " : "", B.bdi(t)]))] : null,
        c && c.intents && c.intents.length ? [B.h("dt", {}, "intents"), B.h("dd", {}, `${c.intents.length} live`)] : null
      )
    );
    const lineage = (title, list) =>
      list.length
        ? [
            B.h("h2", { class: "sc" }, title),
            B.h(
              "ul",
              { class: "lineage" },
              list.slice(0, 6).map((j) =>
                B.h(
                  "li",
                  {},
                  B.knotIcon(kind[j], lookOf(j), 16),
                  B.h("span", {}, B.h("span", { class: "sc" }, B.bdi(B.agentName(ground, author[j])), " "), B.h("span", { class: "statement" }, B.firstLine(textOf(j) ?? "…", 90)))
                )
              ),
              list.length > 6 ? B.h("li", {}, "", B.h("span", { class: "synthetic" }, `and ${list.length - 6} more`)) : null
            ),
          ]
        : [];
    // Behind now, only what was there at the surface: a knot's parents always were, its children may not be.
    const there = (list) => list.filter((j) => valueOf(j) <= state.surface);
    // A claim's whole life, in the Root view. Ids are hex, so the address holds nothing else.
    const toRoot =
      kind[i] === "claim"
        ? B.h("button", { type: "button", class: "open-root", on: { click: () => location.assign(`/root.html#${id}`) } }, "Follow this claim's life")
        : null;
    side.replaceChildren(...[card, toRoot].filter(Boolean), ...lineage("Woven onto", there(parents[i])), ...lineage("Built on by", there(children[i])));
  }

  // ---- The scrubber: drag the surface handle, or use the keys, to replay. It settles on a contribution. ----
  const surfaceEl = $("surface");
  const gaugeEl = gauge.parentElement;
  function surfaceFromY(clientY) {
    const r = gauge.getBoundingClientRect();
    const [a, b] = range();
    const v = a + ((clientY - r.top - TOP - PAD) / (GH - TOP - PAD * 2)) * (b - a);
    return Math.max(a, Math.min(b, v));
  }
  function snap(v) {
    // To the nearest contribution's value at or before v.
    let best = range()[0];
    for (let i = 0; i < N; i++) {
      const x = valueOf(i);
      if (x <= v && x > best) best = x;
    }
    return best;
  }
  const stops = { key: "", values: [] };
  function values() {
    const key = `${state.axis}:${N}`;
    if (stops.key !== key) {
      stops.key = key;
      stops.values = Array.from(new Set(Array.from({ length: N }, (_, i) => valueOf(i)))).sort((x, y) => x - y);
    }
    return stops.values;
  }
  let dragging = false;
  gaugeEl.addEventListener("pointerdown", (e) => {
    if (!N) return;
    dragging = true;
    gaugeEl.setPointerCapture(e.pointerId);
    stop();
    setSurface(surfaceFromY(e.clientY), false);
  });
  gaugeEl.addEventListener("pointermove", (e) => dragging && setSurface(surfaceFromY(e.clientY), false));
  gaugeEl.addEventListener("pointerup", (e) => {
    if (!dragging) return;
    dragging = false;
    setSurface(snap(surfaceFromY(e.clientY)), false);
  });
  surfaceEl.addEventListener("keydown", (e) => {
    const vs = values();
    if (!vs.length) return;
    const cur = Number.isFinite(state.surface) ? state.surface : vs[vs.length - 1];
    let k = vs.findIndex((v) => v >= cur);
    if (k < 0) k = vs.length - 1;
    const jump = Math.max(1, Math.round(vs.length / 10));
    const to = { ArrowDown: k + 1, ArrowUp: k - 1, PageDown: k + jump, PageUp: k - jump, Home: 0, End: vs.length - 1 }[e.key];
    if (to === undefined) return;
    e.preventDefault();
    stop();
    setSurface(vs[Math.max(0, Math.min(vs.length - 1, to))], true);
  });
  $("to-flag").addEventListener("click", () => {
    const fv = flagValue();
    if (fv === null) return;
    stop();
    setSurface(fv, false);
  });

  function setSurface(v, grow) {
    const before = state.surface;
    const [, b] = range();
    state.surface = v >= b ? Infinity : v;
    if (grow && !B.reducedMotion()) {
      const now = performance.now();
      for (let i = 0; i < N; i++) {
        const x = valueOf(i);
        if (x > before && x <= v) born.set(i, now);
      }
    }
    if (state.hover >= 0 && valueOf(state.hover) > state.surface) {
      state.hover = -1;
      ancestry = null;
      showHover(-1);
    }
    note();
    refreshSide();
    requestDraw();
  }

  // ---- Grow: replay the Ground from its first knot ----
  let raf = 0, last = 0;
  const playBtn = $("play");
  function play() {
    if (!N) return;
    const [a, b] = range();
    if (!Number.isFinite(state.surface) || state.surface >= b) state.surface = a;
    state.playing = true;
    B.attr(playBtn, "aria-pressed", "true");
    B.text(playBtn, "Pause");
    last = performance.now();
    const step = (now) => {
      if (!state.playing) return;
      const [a, b] = range();
      if (B.reducedMotion()) {
        // Reduced motion: whole steps, a tenth of the history at a time, no easing and no swelling knots.
        if (now - last > 900 / state.speed) {
          last = now;
          const next = Math.min(b, state.surface + (b - a) / 10);
          setSurface(next, false);
          if (next >= b) return stop();
        }
      } else {
        const dt = now - last;
        last = now;
        const next = state.surface + ((b - a) / 20000) * dt * state.speed;
        setSurface(Math.min(b, next), true);
        if (next >= b) return stop();
      }
      raf = requestAnimationFrame(step);
    };
    raf = requestAnimationFrame(step);
  }
  function stop() {
    state.playing = false;
    cancelAnimationFrame(raf);
    B.attr(playBtn, "aria-pressed", "false");
    B.text(playBtn, "Grow");
  }
  playBtn.addEventListener("click", () => (state.playing ? stop() : play()));
  $("speed").addEventListener("change", (e) => (state.speed = Number(e.target.value)));

  // ---- The vertical axis: authors' clocks, or causal steps ----
  function setAxis(axis) {
    if (axis === state.axis) return;
    stop();
    fromAxis = state.axis;
    state.axis = axis;
    state.view = null;
    state.surface = Infinity;
    note();
    refreshSide();
    state.hover = -1;
    ancestry = null;
    showHover(-1);
    B.attr($("axis-time"), "aria-pressed", String(axis === "time"));
    B.attr($("axis-causal"), "aria-pressed", String(axis === "causal"));
    if (B.reducedMotion()) {
      state.morph = 1;
      return requestDraw();
    }
    const t0 = performance.now();
    const tick = (now) => {
      state.morph = Math.min(1, (now - t0) / 650);
      draw();
      if (state.morph < 1) requestAnimationFrame(tick);
    };
    state.morph = 0;
    requestAnimationFrame(tick);
  }
  $("axis-time").addEventListener("click", () => setAxis("time"));
  $("axis-causal").addEventListener("click", () => setAxis("causal"));

  // ---- The key ----
  function buildKey() {
    const key = $("key");
    const item = (kindName, lk, label) => B.h("span", {}, B.knotIcon(kindName, lk, 16), label);
    key.replaceChildren(
      item("claim", B.look("claim", { status: "pending", agent: 0 }), "claim"),
      item("artifact", B.look("artifact", { status: "pending", agent: 2 }), "artifact"),
      item("relation", B.look("relation", { agent: 4 }), "relation"),
      item("evidence", B.look("evidence", { verdict: "pass" }), "pass"),
      item("evidence", B.look("evidence", { verdict: "fail" }), "fail"),
      item("retract", B.look("retract", { agent: 1 }), "retraction"),
      item("claim", B.look("claim", { status: "accepted" }), "accepted"),
      item("claim", B.look("claim", { status: "rejected" }), "rejected")
    );
  }

  document.addEventListener("bracken-theme", () => {
    buildKey();
    if (!ground) return;
    buildAgents();
    layout();
    refreshSide();
  });
  window.addEventListener("resize", () => ground && layout());
  // Headings are sized to their text, so lay out again once the fonts have arrived.
  document.fonts.ready.then(() => ground && layout());
  document.querySelector(".masthead nav").append(B.themeButton());
  buildKey();

  serially(async () => {
    try {
      await loadGround();
      await loadAll();
      statuses = (await api("/api/status")).statuses;
    } catch (e) {
      note("couldn't read the Ground");
      throw e;
    }
    buildAgents();
    note();
    layout();
    if (N <= BULK_TEXT) bulkTexts(null);
    // A link to /#<id> selects that knot.
    const want = location.hash.slice(1).toLowerCase();
    if (HEX.test(want) && index.has(want)) select(index.get(want));
    listen();
  });
})();
