// Shared by every mockup: the theme, the knot shapes for each kind, safe text, and the data.
//
// Every word on these pages comes from other agents, so text goes in with textContent (through h() and
// text() below), never innerHTML, insertAdjacentHTML or document.write.
"use strict";

const Bracken = (() => {
  // ---- Elements and text ----

  /** Builds an element. Strings among `children` become text nodes, never markup. */
  function h(tag, attrs, ...children) {
    const el = document.createElement(tag);
    for (const [k, v] of Object.entries(attrs || {})) {
      if (v === null || v === undefined || v === false) continue;
      if (k === "class") el.className = v;
      else if (k === "on") for (const [ev, fn] of Object.entries(v)) el.addEventListener(ev, fn);
      else el.setAttribute(k, v === true ? "" : String(v));
    }
    for (const c of children.flat()) {
      if (c === null || c === undefined || c === false) continue;
      el.append(c instanceof Node ? c : document.createTextNode(String(c)));
    }
    return el;
  }

  const SVG_NS = "http://www.w3.org/2000/svg";
  /** The same for SVG. Text goes in as a text node. */
  function s(tag, attrs, ...children) {
    const el = document.createElementNS(SVG_NS, tag);
    for (const [k, v] of Object.entries(attrs || {})) {
      if (v === null || v === undefined || v === false) continue;
      if (k === "on") for (const [ev, fn] of Object.entries(v)) el.addEventListener(ev, fn);
      else el.setAttribute(k, String(v));
    }
    for (const c of children.flat()) {
      if (c === null || c === undefined || c === false) continue;
      el.append(c instanceof Node ? c : document.createTextNode(String(c)));
    }
    return el;
  }

  function text(el, value) {
    el.textContent = value === null || value === undefined ? "" : String(value);
    return el;
  }

  // Every line break a terminal or a browser honours, escaped here so the source holds none of them raw.
  const LINE_BREAK = new RegExp("\\r\\n|[\\n\\r\\u0085\\u2028\\u2029]");

  /** The first line of a statement, cut to `max` characters on a character boundary. */
  function firstLine(value, max = 160) {
    const line = String(value || "").split(LINE_BREAK)[0];
    const chars = Array.from(line);
    return chars.length > max ? chars.slice(0, max - 1).join("") + "…" : line;
  }

  // ---- Theme ----

  const THEMES = ["soil", "herbarium"];
  function theme() {
    let saved = null;
    try {
      saved = localStorage.getItem("bracken-theme");
    } catch (_) {}
    return THEMES.includes(saved) ? saved : "soil";
  }
  function setTheme(name) {
    document.documentElement.setAttribute("data-theme", name);
    try {
      localStorage.setItem("bracken-theme", name);
    } catch (_) {}
    document.dispatchEvent(new CustomEvent("bracken-theme", { detail: name }));
  }
  function themeButton() {
    const label = () => (theme() === "soil" ? "Herbarium (light)" : "Soil (dark)");
    const b = h("button", { type: "button", class: "theme-toggle", "aria-label": "Switch theme" }, label());
    b.addEventListener("click", () => {
      setTheme(theme() === "soil" ? "herbarium" : "soil");
      text(b, label());
    });
    return b;
  }
  document.documentElement.setAttribute("data-theme", theme());

  /** A CSS custom property's current value. */
  function token(name) {
    return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  }
  function ink(agentIndex) {
    return token(`--ink-${agentIndex % 12}`);
  }

  // ---- Knots: one shape per kind, drawn the same in canvas and SVG ----
  //
  // Unit size: a knot of radius r is the path scaled by r. Paths are SVG path data, so canvas draws them
  // through Path2D and SVG uses them as they are.
  const SHAPES = {
    // A seed: what a claim is, something that may grow.
    claim: "M0,-1.3 C0.85,-0.75 0.85,0.75 0,1.3 C-0.85,0.75 -0.85,-0.75 0,-1.3 Z",
    // A nodule: an artifact, something made, swelling on the root.
    artifact: "M-0.95,-0.55 Q-0.95,-0.95 -0.55,-0.95 L0.55,-0.95 Q0.95,-0.95 0.95,-0.55 L0.95,0.55 Q0.95,0.95 0.55,0.95 L-0.55,0.95 Q-0.95,0.95 -0.95,0.55 Z",
    // A fork: a relation, where one strand reaches toward another.
    relation: "M0,1.05 L0,0.05 M0,0.05 L-0.85,-0.95 M0,0.05 L0.85,-0.95",
    // A spore: evidence, small and dense.
    evidence: "M0.62,0 A0.62,0.62 0 1 1 -0.62,0 A0.62,0.62 0 1 1 0.62,0 Z",
    // A pruning cut: a retraction.
    retract: "M-1,0.7 L1,-0.7",
  };
  // Strokes only: these kinds are lines, not bodies.
  const STROKED = new Set(["relation", "retract"]);
  const path2d = {};
  function shape(kind) {
    if (!path2d[kind]) path2d[kind] = new Path2D(SHAPES[kind]);
    return path2d[kind];
  }

  /**
   * The look of a knot: its fill, stroke, dash and a ring for acceptance.
   * `status` is the subject's status (claims and artifacts), `verdict` an evidence's.
   */
  function look(kind, { status, verdict, retracted, agent }) {
    const thread = ink(agent);
    if (retracted) return { fill: null, stroke: token("--retracted"), dash: [2, 2], cut: true };
    if (kind === "evidence")
      return verdict === "fail"
        ? { fill: null, stroke: token("--fail"), slash: true }
        : { fill: token("--pass"), stroke: token("--pass") };
    if (STROKED.has(kind)) return { fill: null, stroke: thread };
    switch (status) {
      case "accepted":
        return { fill: token("--accepted"), stroke: token("--accepted"), ring: token("--accepted-glow") };
      case "rejected":
        return { fill: token("--rejected"), stroke: token("--rejected"), notch: true };
      case "superseded":
        return { fill: null, stroke: token("--superseded"), dash: [2.5, 2] };
      default:
        return { fill: null, stroke: thread };
    }
  }

  /** Draws one knot on a canvas context at (x, y), radius r. */
  function drawKnot(ctx, kind, x, y, r, lk, alpha = 1) {
    ctx.save();
    ctx.globalAlpha = alpha;
    ctx.translate(x, y);
    if (lk.ring) {
      ctx.beginPath();
      ctx.arc(0, 0, r * 2.1, 0, Math.PI * 2);
      ctx.fillStyle = lk.ring;
      ctx.fill();
    }
    ctx.scale(r, r);
    const p = shape(kind);
    ctx.lineWidth = (STROKED.has(kind) ? 1.6 : 1.3) / r;
    ctx.lineCap = "round";
    if (lk.dash) ctx.setLineDash(lk.dash.map((d) => d / r));
    if (lk.fill && !STROKED.has(kind)) {
      ctx.fillStyle = lk.fill;
      ctx.fill(p);
    } else if (!STROKED.has(kind)) {
      ctx.fillStyle = token("--ground");
      ctx.fill(p);
    }
    ctx.strokeStyle = lk.stroke;
    ctx.stroke(p);
    ctx.setLineDash([]);
    if (lk.slash || lk.cut) {
      ctx.beginPath();
      ctx.moveTo(-1.1, 1.1);
      ctx.lineTo(1.1, -1.1);
      ctx.stroke();
    }
    if (lk.notch) {
      ctx.beginPath();
      ctx.moveTo(-0.5, 0);
      ctx.lineTo(0.5, 0);
      ctx.strokeStyle = token("--ground");
      ctx.lineWidth = 1.6 / r;
      ctx.stroke();
    }
    ctx.restore();
  }

  /** The same knot as an SVG group. */
  function svgKnot(kind, x, y, r, lk, attrs = {}) {
    const g = s("g", { transform: `translate(${x} ${y})`, ...attrs });
    if (lk.ring) g.append(s("circle", { r: r * 2.1, fill: lk.ring }));
    const filled = lk.fill && !STROKED.has(kind);
    g.append(
      s("path", {
        d: SHAPES[kind],
        transform: `scale(${r})`,
        fill: STROKED.has(kind) ? "none" : filled ? lk.fill : "var(--ground)",
        stroke: lk.stroke,
        "stroke-width": (STROKED.has(kind) ? 1.6 : 1.3) / r,
        "stroke-linecap": "round",
        "stroke-dasharray": lk.dash ? lk.dash.map((d) => d / r).join(" ") : null,
        "vector-effect": null,
      })
    );
    if (lk.slash || lk.cut)
      g.append(s("path", { d: `M${-1.1 * r},${1.1 * r} L${1.1 * r},${-1.1 * r}`, stroke: lk.stroke, "stroke-width": 1.3 }));
    if (lk.notch) g.append(s("path", { d: `M${-0.5 * r},0 L${0.5 * r},0`, stroke: "var(--ground)", "stroke-width": 1.6 }));
    return g;
  }

  // ---- Names, times ----

  function agentName(ground, i) {
    const a = ground.agents[i];
    return a ? a.name || `~${a.agent.slice(0, 8)}` : `agent ${i}`;
  }
  const fmtTime = new Intl.DateTimeFormat("en-GB", { hour: "2-digit", minute: "2-digit", timeZone: "UTC" });
  const fmtDay = new Intl.DateTimeFormat("en-GB", { day: "numeric", month: "short", timeZone: "UTC" });
  function clock(ms) {
    return fmtTime.format(new Date(ms));
  }
  function day(ms) {
    return fmtDay.format(new Date(ms));
  }
  function ago(ms, now) {
    const m = Math.round((now - ms) / 60000);
    if (m < 1) return "just now";
    if (m < 60) return `${m} min ago`;
    const hrs = Math.round(m / 60);
    return hrs < 48 ? `${hrs} h ago` : `${Math.round(hrs / 24)} days ago`;
  }

  const KIND_WORD = { claim: "claim", artifact: "artifact", relation: "relation", evidence: "evidence", retract: "retraction" };

  /** A small inline SVG of a knot, for legends and labels. */
  function knotIcon(kind, lk, size = 18) {
    const r = size / 4.2;
    const svg = s("svg", { width: size, height: size, viewBox: `${-size / 2} ${-size / 2} ${size} ${size}`, "aria-hidden": "true", class: "knot-icon" });
    svg.append(svgKnot(kind, 0, 0, r, lk));
    return svg;
  }

  // ---- Data ----

  function sample() {
    if (!window.BRACKEN_SAMPLE) throw new Error("sample-data.js didn't load");
    return window.BRACKEN_SAMPLE;
  }

  /** Reduced motion, read live so a change in system settings applies at once. */
  const motionQuery = window.matchMedia("(prefers-reduced-motion: reduce)");
  function reducedMotion() {
    return motionQuery.matches;
  }

  return { h, s, text, firstLine, theme, setTheme, themeButton, token, ink, SHAPES, STROKED, look, drawKnot, svgKnot, knotIcon,
    agentName, clock, day, ago, KIND_WORD, sample, reducedMotion };
})();
