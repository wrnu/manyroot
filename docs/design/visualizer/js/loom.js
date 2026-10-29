// The Loom: agents as roots growing down a root window, each contribution a knot on its author's root,
// woven to its causal parents by hyphae. The core sample on the left is time, and its surface handle
// replays the Ground's growth.
"use strict";

(() => {
  const B = Bracken;
  const $ = (id) => document.getElementById(id);
  const TOP = 64; // the agents' band
  const PAD = 28;

  // ---- Data ----
  let data; // { ground, graph, status, contributions?, statements?, roots? }
  let N, kind, author, parents, time, depth, tags, statusById, retractedSet, children, agentsCount;
  let ownParent; // per node: is any parent by the same author (then the root carries the link)

  function load(size) {
    data = size === "sample" ? B.sample() : Synthetic.make(Number(size));
    const g = data.graph;
    N = g.ids.length;
    ({ kind, author, parents, time, depth, tags } = g);
    statusById = data.status.statuses;
    retractedSet = new Set(g.retracted);
    agentsCount = data.ground.agents.length;
    children = Array.from({ length: N }, () => []);
    ownParent = new Uint8Array(N);
    for (let i = 0; i < N; i++)
      for (const p of parents[i]) {
        children[p].push(i);
        if (author[p] === author[i]) ownParent[i] = 1;
      }
    B.text($("ground-name"), data.ground.name);
    B.text($("data-note"), size === "sample"
      ? "Synthetic sample from a throwaway Ground; times respaced for display."
      : "Synthetic Ground, generated in the page to show scale.");
    state.hover = -1;
    state.selected = -1;
    state.view = null;
    state.surface = Infinity;
    // The cursor the reader last saw: the sample says it's about two thirds of the way down.
    state.flag = valueOf(Math.floor(N * 0.68));
    buildAgents();
    showSide(-1);
    layout();
  }

  const statusOf = (i) => {
    if (retractedSet.has(i)) return "retracted";
    const s = statusById[data.graph.ids[i]];
    return s ? s.status : "pending";
  };
  const verdictOf = (i) => {
    if (kind[i] !== "evidence") return null;
    const c = data.contributions && data.contributions[data.graph.ids[i]];
    if (c) return c.body.verdict;
    return (i * 2654435761) % 100 < 16 ? "fail" : "pass";
  };
  const textOf = (i) => {
    const c = data.contributions && data.contributions[data.graph.ids[i]];
    if (c) {
      const b = c.body;
      if (c.kind === "claim") return b.statement;
      if (c.kind === "artifact") return b.name;
      if (c.kind === "relation") return `${b.kind.replace("_", " ")} ${b.to.slice(0, 8)}`;
      if (c.kind === "evidence") return `${b.verdict}: ${b.method}`;
      if (c.kind === "retract") return `retracts ${b.target.slice(0, 8)}${b.reason ? ": " + b.reason : ""}`;
    }
    return data.statements ? data.statements[i] : "";
  };

  // ---- State and geometry ----
  const state = { axis: "time", surface: Infinity, hover: -1, selected: -1, view: null, playing: false, speed: 1, flag: 0, morph: 1 };
  const canvas = $("loom-canvas");
  const ctx = canvas.getContext("2d");
  const gauge = $("gauge-canvas");
  const gctx = gauge.getContext("2d");
  let W = 0, H = 0, GW = 0, GH = 0, dpr = 1;
  let colX = [];
  let fromAxis = null; // the previous axis while the vertical axis morphs

  const valueOf = (i, axis = state.axis) => (axis === "time" ? time[i] : depth[i]);
  function range(axis = state.axis) {
    return axis === "time" ? [time[0], time[N - 1]] : [0, depth.reduce((a, b) => Math.max(a, b), 0)];
  }
  function view() {
    return state.view || range();
  }
  function yOf(i) {
    const at = (axis, [a, b]) => TOP + PAD + ((valueOf(i, axis) - a) / Math.max(1, b - a)) * (H - TOP - PAD * 2);
    const y = at(state.axis, view());
    if (state.morph >= 1 || !fromAxis) return y;
    const y0 = at(fromAxis, range(fromAxis));
    return y0 + (y - y0) * ease(state.morph);
  }
  const ease = (t) => 1 - Math.pow(1 - t, 3);
  function rootX(a, y) {
    const amp = Math.min(9, (W / Math.max(1, agentsCount)) * 0.07);
    return colX[a] + amp * Math.sin(y * 0.011 + a * 1.7) + amp * 0.45 * Math.sin(y * 0.031 + a * 0.6);
  }
  const xOf = (i) => rootX(author[i], yOf(i));

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
    colX = data.ground.agents.map((_, a) => (agentsCount === 1 ? W / 2 : left + a * span));
    for (const [a, el] of agentEls.entries()) {
      el.style.left = `${colX[a]}px`;
      el.style.setProperty("--col", `${Math.max(48, span - 8)}px`);
      el.title = B.agentName(data.ground, a);
    }
    draw();
  }

  // ---- The agents' band ----
  let agentEls = [];
  function buildAgents() {
    const band = $("agents");
    band.replaceChildren();
    agentEls = data.ground.agents.map((a, i) => {
      const el = B.h("div", { class: "agent" }, B.agentName(data.ground, i), B.h("span", { class: "count num" }, a.contributions.toLocaleString("en")));
      el.style.setProperty("--agent-ink", B.ink(i));
      band.append(el);
      return el;
    });
  }

  // ---- Drawing ----
  let grid = new Map(); // hit-testing cells
  let ancestry = null; // Set of the hovered knot's ancestors, or null
  let born = new Map(); // knots that appeared while growing: index -> time they appeared

  function visible() {
    const [a, b] = view();
    const out = [];
    for (let i = 0; i < N; i++) {
      const v = valueOf(i);
      if (v > state.surface) continue;
      if (v < a - (b - a) * 0.05 || v > b + (b - a) * 0.05) continue;
      out.push(i);
    }
    return out;
  }

  function draw() {
    if (!W) return;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, W, H);
    const vis = visible();
    const dense = vis.length > 5000;
    const r = Math.max(1.6, Math.min(5.5, Math.sqrt(((W * (H - TOP)) / Math.max(1, vis.length)) / 40)));
    drawStrata();
    drawRoots(vis, dense);
    drawHyphae(vis, dense, null);
    if (!dense) drawKnots(vis, r, null);
    else drawBlooms(vis);
    if (ancestry) {
      ctx.fillStyle = B.token("--ground");
      ctx.globalAlpha = 0.72;
      ctx.fillRect(0, TOP, W, H - TOP);
      ctx.globalAlpha = 1;
      const anc = vis.filter((i) => ancestry.has(i));
      drawAncestorRoots(anc);
      drawHyphae(anc, false, ancestry);
      drawKnots(anc, Math.max(r, 3.2), ancestry);
    }
    drawSurface();
    drawLabels(vis, r, dense);
    buildGrid(dense ? vis.filter((i) => kind[i] === "claim" || kind[i] === "artifact") : vis);
    drawGauge();
  }

  function drawStrata() {
    // Hour (or day) bands, as faint soil strata behind everything.
    if (state.axis !== "time") return;
    const [a, b] = view();
    const step = b - a > 3 * 86400000 ? 86400000 : b - a > 6 * 3600000 ? 3600000 * 2 : 3600000;
    ctx.fillStyle = B.token("--stratum");
    ctx.globalAlpha = 0.35;
    for (let t = Math.floor(a / step) * step, k = 0; t < b; t += step, k++) {
      if (Math.floor(t / step) % 2) continue;
      const y0 = TOP + PAD + ((t - a) / (b - a)) * (H - TOP - PAD * 2);
      const y1 = TOP + PAD + ((t + step - a) / (b - a)) * (H - TOP - PAD * 2);
      ctx.fillRect(0, Math.max(TOP, y0), W, Math.max(0, y1 - Math.max(TOP, y0)));
    }
    ctx.globalAlpha = 1;
  }

  function drawRoots(vis, dense) {
    // Each agent's root runs from its first visible knot to its last, thicker where it's busiest.
    const bins = Math.max(1, Math.floor((H - TOP) / 5));
    for (let a = 0; a < agentsCount; a++) {
      let y0 = Infinity, y1 = -Infinity;
      const hist = new Float32Array(bins);
      for (const i of vis) {
        if (author[i] !== a) continue;
        const y = yOf(i);
        y0 = Math.min(y0, y);
        y1 = Math.max(y1, y);
        hist[Math.max(0, Math.min(bins - 1, Math.floor(((y - TOP) / (H - TOP)) * bins)))]++;
      }
      if (y0 === Infinity) continue;
      let max = 0;
      for (const c of hist) max = Math.max(max, c);
      ctx.strokeStyle = B.ink(a);
      ctx.lineCap = "round";
      ctx.globalAlpha = 0.85;
      if (!dense) {
        ctx.lineWidth = 1.5;
        ctx.beginPath();
        for (let y = y0; y <= y1; y += 3) (y === y0 ? ctx.moveTo : ctx.lineTo).call(ctx, rootX(a, y), y);
        ctx.lineTo(rootX(a, y1), y1);
        ctx.stroke();
      } else {
        // The density reads as the root's girth.
        let prev = null;
        for (let b = 0; b < bins; b++) {
          const y = TOP + ((b + 0.5) / bins) * (H - TOP);
          if (y < y0 || y > y1) continue;
          const w = 0.8 + 6 * Math.sqrt(hist[b] / Math.max(1, max));
          if (prev) {
            ctx.lineWidth = w;
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
      ctx.arc(rootX(a, y1), y1 + 2.5, 1.6, 0, Math.PI * 2);
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
        if (author[p] !== author[i] || !ancestry.has(p)) continue;
        ctx.strokeStyle = B.ink(author[i]);
        ctx.beginPath();
        const y0 = yOf(p), y1 = yOf(i);
        for (let y = y0; y <= y1; y += 3) (y === y0 ? ctx.moveTo : ctx.lineTo).call(ctx, rootX(author[i], y), y);
        ctx.lineTo(xOf(i), y1);
        ctx.stroke();
      }
  }

  function hyphaPath(x0, y0, x1, y1) {
    const dy = Math.max(26, (y1 - y0) * 0.55);
    ctx.moveTo(x0, y0);
    ctx.bezierCurveTo(x0, y0 + dy, x1, y1 - dy, x1, y1);
  }

  function drawHyphae(vis, dense, only) {
    // Causal links between agents. One agent's links to its own earlier work run along its root.
    if (!dense) {
      ctx.lineWidth = only ? 1.6 : 1;
      for (const i of vis)
        for (const p of parents[i]) {
          if (author[p] === author[i] || valueOf(p) > state.surface) continue;
          if (only && !only.has(p)) continue;
          ctx.strokeStyle = only ? B.ink(author[p]) : B.token("--hypha");
          ctx.beginPath();
          hyphaPath(xOf(p), yOf(p), xOf(i), yOf(i));
          ctx.stroke();
        }
      return;
    }
    // At density, links bundle: one strand per pair of agents per band of the window, weighted by its count,
    // so the paths the work actually takes stand out and a uniform mesh doesn't.
    const band = Math.max(14, (H - TOP) / 70);
    const bundles = new Map();
    for (const i of vis)
      for (const p of parents[i]) {
        if (author[p] === author[i]) continue;
        const key = `${author[p]}:${author[i]}:${Math.floor(yOf(i) / band)}`;
        const e = bundles.get(key) || { a: author[p], b: author[i], y0: 0, y1: 0, n: 0 };
        e.y0 += yOf(p);
        e.y1 += yOf(i);
        e.n++;
        bundles.set(key, e);
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

  function drawKnots(vis, r, only) {
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
      const lk = B.look(k, { status: statusOf(i), verdict: verdictOf(i), retracted: retractedSet.has(i), agent: author[i] });
      B.drawKnot(ctx, k, xOf(i), yOf(i), rr, lk, 1);
      if (i === state.hover || i === state.selected) {
        ctx.beginPath();
        ctx.arc(xOf(i), yOf(i), rr * 2.6, 0, Math.PI * 2);
        ctx.strokeStyle = B.token("--focus");
        ctx.lineWidth = 1.5;
        ctx.stroke();
      }
    }
    if (born.size) requestDraw();
  }

  function drawBlooms(vis) {
    // At density, only what the policy settled shows as a point: accepted green, rejected rust.
    for (const i of vis) {
      if (kind[i] !== "claim" && kind[i] !== "artifact") continue;
      const st = statusOf(i);
      if (st !== "accepted" && st !== "rejected") continue;
      ctx.fillStyle = B.token(st === "accepted" ? "--accepted" : "--rejected");
      ctx.fillRect(xOf(i) - 1, yOf(i) - 1, 2, 2);
    }
  }

  function drawSurface() {
    // Below the replay surface, the soil is unturned: nothing has grown there yet.
    if (!Number.isFinite(state.surface)) return;
    const [a, b] = view();
    if (state.surface >= b) return;
    const y = TOP + PAD + ((state.surface - a) / Math.max(1, b - a)) * (H - TOP - PAD * 2);
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
    box.replaceChildren();
    const subjects = vis.filter((i) => kind[i] === "claim" || kind[i] === "artifact");
    if (dense || subjects.length > 90) return;
    const lastY = new Map();
    const colW = agentsCount > 1 ? colX[1] - colX[0] : W;
    for (const i of subjects) {
      const y = yOf(i) - TOP;
      const a = author[i];
      if ((lastY.get(a) ?? -Infinity) > y - 15) continue;
      lastY.set(a, y);
      const isIntent = tags[i] && tags[i].includes("intent");
      const t = isIntent ? "intends to work on it" : B.firstLine(textOf(i), 70);
      const el = B.h("div", { class: ancestry && !ancestry.has(i) ? "knot-label dim" : "knot-label" }, t);
      el.style.left = `${xOf(i) + r + 6}px`;
      el.style.top = `${y - 8}px`;
      el.style.maxWidth = `${Math.max(80, Math.min(260, colW * 1.45))}px`;
      box.append(el);
    }
  }

  function buildGrid(points) {
    grid = new Map();
    for (const i of points) {
      const key = `${Math.floor(xOf(i) / 16)},${Math.floor(yOf(i) / 16)}`;
      (grid.get(key) || grid.set(key, []).get(key)).push(i);
    }
  }
  function hit(x, y) {
    let best = -1, bd = 14 * 14;
    for (let gx = Math.floor(x / 16) - 1; gx <= Math.floor(x / 16) + 1; gx++)
      for (let gy = Math.floor(y / 16) - 1; gy <= Math.floor(y / 16) + 1; gy++)
        for (const i of grid.get(`${gx},${gy}`) || []) {
          const d = (xOf(i) - x) ** 2 + (yOf(i) - y) ** 2;
          if (d < bd) (bd = d), (best = i);
        }
    return best;
  }

  // ---- The core sample ----
  function drawGauge() {
    gctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    gctx.clearRect(0, 0, GW, GH);
    const [a, b] = range();
    const y = (v) => TOP + PAD + ((v - a) / Math.max(1, b - a)) * (GH - TOP - PAD * 2);
    // Strata and their times.
    gctx.font = `500 11px "Alegreya Sans SC", sans-serif`;
    gctx.textBaseline = "middle";
    if (state.axis === "time") {
      const step = b - a > 3 * 86400000 ? 86400000 * Math.ceil((b - a) / 86400000 / 12) : b - a > 6 * 3600000 ? 7200000 : 3600000;
      for (let t = Math.ceil(a / step) * step, k = 0; t <= b; t += step, k++) {
        gctx.fillStyle = B.token("--stratum-line");
        gctx.fillRect(GW - 14, y(t), 14, 1);
        gctx.fillStyle = B.token("--root-faint");
        gctx.fillText(step >= 86400000 ? B.day(t) : B.clock(t), 8, y(t));
      }
    } else {
      gctx.fillStyle = B.token("--root-faint");
      for (let d = 0, step = Math.max(1, Math.round(b / 10)); d <= b; d += step) gctx.fillText(`step ${d}`, 8, y(d));
    }
    // The density of the whole history, as fibres in the core.
    const bins = Math.max(1, Math.floor((GH - TOP - PAD * 2) / 3));
    const hist = new Float32Array(bins);
    for (let i = 0; i < N; i++) hist[Math.min(bins - 1, Math.floor(((valueOf(i) - a) / Math.max(1, b - a)) * bins))]++;
    let max = 0;
    for (const c of hist) max = Math.max(max, c);
    gctx.fillStyle = B.token("--root-dim");
    for (let k = 0; k < bins; k++) {
      const w = 22 * Math.sqrt(hist[k] / Math.max(1, max));
      gctx.globalAlpha = 0.55;
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
    $("surface").setAttribute("aria-valuenow", String(Math.round(((surface - a) / Math.max(1, b - a)) * 1000)));
    $("surface").setAttribute("aria-valuetext", state.axis === "time" ? B.clock(surface) : `causal step ${surface}`);
    B.text($("surface-time"), state.axis === "time" ? B.clock(surface) : `step ${surface}`);
    const flagV = state.axis === "time" ? state.flag : depth[Math.floor(N * 0.68)];
    $("flag").style.top = `${y(flagV)}px`;
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

  function ancestorsOf(i) {
    const seen = new Set([i]);
    const stack = [i];
    while (stack.length) for (const p of parents[stack.pop()]) if (!seen.has(p)) seen.add(p), stack.push(p);
    return seen;
  }

  const win = $("window");
  win.addEventListener("pointermove", (e) => {
    const r = canvas.getBoundingClientRect();
    const i = hit(e.clientX - r.left, e.clientY - r.top);
    if (i === state.hover) return;
    state.hover = i;
    ancestry = i >= 0 ? ancestorsOf(i) : null;
    showHover(i, e.clientX - r.left, e.clientY - r.top);
    canvas.style.cursor = i >= 0 ? "pointer" : "default";
    requestDraw();
  });
  win.addEventListener("pointerleave", () => {
    state.hover = -1;
    ancestry = null;
    showHover(-1);
    requestDraw();
  });
  win.addEventListener("click", () => {
    state.selected = state.hover;
    showSide(state.selected);
    requestDraw();
  });
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
        let na = at - (at - a) * f, nb = at + (b - at) * f;
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

  function showHover(i, x, y) {
    const el = $("hover-label");
    const tether = $("tether");
    tether.replaceChildren();
    if (i < 0) {
      el.hidden = true;
      return;
    }
    const who = `${B.KIND_WORD[kind[i]]} · ${B.agentName(data.ground, author[i])} · ${state.axis === "time" ? B.clock(time[i]) : "step " + depth[i]}`;
    el.replaceChildren(B.h("div", { class: "who sc" }, who), B.h("div", { class: "what" }, B.firstLine(textOf(i), 140)));
    el.hidden = false;
    const kx = xOf(i), ky = yOf(i);
    const left = kx > W - 380 ? kx - 360 : kx + 26;
    const top = Math.max(TOP + 6, Math.min(H - 90, ky - 46));
    el.style.left = `${left}px`;
    el.style.top = `${top}px`;
    // The tether: a hairline stitch from the label to its knot.
    tether.append(B.s("path", { d: `M${kx},${ky} L${left < kx ? left + 340 : left},${top + 18}`, stroke: B.token("--root-faint"), "stroke-width": 1, fill: "none" }));
  }

  function showSide(i) {
    const side = $("side");
    if (i < 0) {
      side.replaceChildren(B.h("p", { class: "side-hint" }, "Hover a knot to light its ancestry. Click it for its label."));
      return;
    }
    const id = data.graph.ids[i];
    const st = statusOf(i);
    const lk = B.look(kind[i], { status: st, verdict: verdictOf(i), retracted: retractedSet.has(i), agent: author[i] });
    const s = statusById[id];
    const card = B.h(
      "article",
      { class: "label-card" },
      B.h("div", { class: "kind sc" }, B.knotIcon(kind[i], lk), `${B.KIND_WORD[kind[i]]} · ${kind[i] === "claim" || kind[i] === "artifact" ? st : verdictOf(i) || "woven"}`),
      B.h("p", { class: "statement" }, textOf(i)),
      B.h(
        "dl",
        {},
        B.h("dt", {}, "author"), B.h("dd", {}, B.agentName(data.ground, author[i])),
        B.h("dt", {}, "written"), B.h("dd", { class: "num" }, `${B.day(time[i])} ${B.clock(time[i])} (their clock)`),
        B.h("dt", {}, "causal step"), B.h("dd", { class: "num" }, String(depth[i])),
        B.h("dt", {}, "id"), B.h("dd", { class: "id" }, id.slice(0, 16) + "…"),
        s ? [B.h("dt", {}, "evidence"), B.h("dd", {}, `${s.passes} passing, ${s.fails} failing`)] : null,
        tags[i] && tags[i].length ? [B.h("dt", {}, "tags"), B.h("dd", {}, tags[i].join(", "))] : null
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
                  B.knotIcon(kind[j], B.look(kind[j], { status: statusOf(j), verdict: verdictOf(j), retracted: retractedSet.has(j), agent: author[j] }), 16),
                  B.h("span", {}, B.h("span", { class: "sc" }, B.agentName(data.ground, author[j]) + " "), B.h("span", { class: "statement" }, B.firstLine(textOf(j), 90)))
                )
              ),
              list.length > 6 ? B.h("li", {}, "", B.h("span", { class: "synthetic" }, `and ${list.length - 6} more`)) : null
            ),
          ]
        : [];
    const hasRoot = data.roots && data.roots[id];
    side.replaceChildren(
      ...[
        card,
        hasRoot ? B.h("a", { class: "open-root", href: `root.html#${id}` }, "Follow this claim's life in the Root view →") : null,
        ...lineage("Woven onto", parents[i]),
        ...lineage("Built on by", children[i]),
      ].filter(Boolean)
    );
  }

  // The surface handle: drag it, or use the keys, to replay. It settles on a contribution, never between.
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
    let best = valueOf(0);
    for (let i = 0; i < N; i++) {
      const x = valueOf(i);
      if (x <= v && x > best) best = x;
    }
    return best;
  }
  let dragging = false;
  gaugeEl.addEventListener("pointerdown", (e) => {
    dragging = true;
    gaugeEl.setPointerCapture(e.pointerId);
    stop();
    setSurface(surfaceFromY(e.clientY), false);
  });
  gaugeEl.addEventListener("pointermove", (e) => dragging && setSurface(surfaceFromY(e.clientY), false));
  gaugeEl.addEventListener("pointerup", (e) => {
    dragging = false;
    setSurface(snap(surfaceFromY(e.clientY)), false);
  });
  surfaceEl.addEventListener("keydown", (e) => {
    const values = Array.from(new Set(Array.from({ length: N }, (_, i) => valueOf(i)))).sort((x, y) => x - y);
    const cur = Number.isFinite(state.surface) ? state.surface : values[values.length - 1];
    let k = values.findIndex((v) => v >= cur);
    if (k < 0) k = values.length - 1;
    const jump = Math.max(1, Math.round(values.length / 10));
    const to = { ArrowDown: k + 1, ArrowUp: k - 1, PageDown: k + jump, PageUp: k - jump, Home: 0, End: values.length - 1 }[e.key];
    if (to === undefined) return;
    e.preventDefault();
    stop();
    setSurface(values[Math.max(0, Math.min(values.length - 1, to))], true);
  });

  function setSurface(v, grow) {
    const before = state.surface;
    const [, b] = range();
    state.surface = v >= b ? Infinity : v;
    if (grow && !B.reducedMotion())
      for (let i = 0; i < N; i++) {
        const x = valueOf(i);
        if (x > before && x <= v) born.set(i, performance.now());
      }
    requestDraw();
  }

  // ---- Grow: replay the Ground from its first knot ----
  let raf = 0, last = 0;
  const playBtn = $("play");
  function play() {
    const [a, b] = range();
    if (!Number.isFinite(state.surface) || state.surface >= b) state.surface = a;
    state.playing = true;
    playBtn.setAttribute("aria-pressed", "true");
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
    playBtn.setAttribute("aria-pressed", "false");
    B.text(playBtn, "Grow");
  }
  playBtn.addEventListener("click", () => (state.playing ? stop() : play()));
  $("speed").addEventListener("change", (e) => (state.speed = Number(e.target.value)));

  // ---- The vertical axis: authors' clocks, or causal steps ----
  function setAxis(axis) {
    if (axis === state.axis) return;
    fromAxis = state.axis;
    state.axis = axis;
    state.view = null;
    state.surface = Infinity;
    $("axis-time").setAttribute("aria-pressed", String(axis === "time"));
    $("axis-causal").setAttribute("aria-pressed", String(axis === "causal"));
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

  for (const b of document.querySelectorAll("[data-size]"))
    b.addEventListener("click", () => {
      stop();
      for (const o of document.querySelectorAll("[data-size]")) o.setAttribute("aria-pressed", String(o === b));
      load(b.dataset.size);
    });

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
    buildAgents();
    buildKey();
    layout();
  });
  window.addEventListener("resize", layout);
  document.querySelector(".masthead nav").append(B.themeButton());
  buildKey();
  // A link from the Root view (loom.html#<id>) selects that knot.
  load("sample");
  const want = location.hash.slice(1);
  if (want) {
    const i = data.graph.ids.indexOf(want);
    if (i >= 0) (state.selected = i), showSide(i), draw();
  }
})();
