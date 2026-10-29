// The Trust constellation (docs/design/visualizer.md section 3.4): who validates whom, as a meadow.
// Each agent is a flower; pollen flies from a validator to the author whose work it checked. A pair's
// passes and fails are two strands, as thick as their counts, and pollen drifts along counted passes.
// The numbers are /api/trust's, which counts as `show --json` does.
"use strict";

(() => {
  const B = Bracken;
  const $ = (id) => document.getElementById(id);
  const S = B.s;
  const CX = 500, CY = 400, RING = 300; // the field's 1000 x 800 units

  let ground = null;
  let trust = null;
  let counted = false; // show only what the policy counts
  let lit = null; // { agent } or { edge: "from:to" } while something is hovered or chosen

  async function api(path) {
    const r = await fetch(path, { headers: { Accept: "application/json" } });
    if (!r.ok) throw new Error(`${path}: ${r.status}`);
    return r.json();
  }

  async function load() {
    [ground, trust] = await Promise.all([api("/api/ground"), api("/api/trust")]);
    B.text($("ground-name"), ground.name);
    const p = ground.policy;
    const validators = p.validators.length ? p.validators.map((v) => v.slice(0, 8)).join(", ") : "anyone but the author";
    B.text($("policy"), `policy: ${p.min_passes} counted pass${p.min_passes === 1 ? "" : "es"} to accept; counted validators: ${validators}`);
    draw();
  }

  /** Each pair's numbers under the toggle: passes and fails that stand, or only those counted. */
  function pairs() {
    return trust.edges
      .map((e) => ({ from: e.from, to: e.to, passes: counted ? e.counted_passes : e.passes, fails: counted ? e.counted_fails : e.fails, counted: e }))
      .filter((e) => e.passes + e.fails > 0);
  }

  const name = (i) => B.agentName(ground, i);
  const place = (i, n) => {
    const a = -Math.PI / 2 + (i / Math.max(1, n)) * Math.PI * 2;
    return [CX + RING * Math.cos(a), CY + RING * Math.sin(a), a];
  };

  // ---- Drawing ----
  function draw() {
    const field = $("field");
    const n = ground.agents.length;
    const list = pairs();
    const others = list.filter((e) => e.from !== e.to);
    // Passes and fails each on their own scale: fails are rarer, and must still show.
    const max = { pass: Math.max(1, ...others.map((e) => e.passes)), fail: Math.max(1, ...others.map((e) => e.fails)) };
    // How much each agent checked and was checked, for the flowers' size.
    const given = new Array(n).fill(0), got = new Array(n).fill(0), own = new Array(n).fill(0);
    for (const e of list) {
      if (e.from === e.to) own[e.from] += e.passes + e.fails;
      else {
        given[e.from] += e.passes + e.fails;
        got[e.to] += e.passes + e.fails;
      }
    }
    const most = Math.max(1, ...given.map((g, i) => g + got[i]));
    const radius = (i) => 9 + 16 * Math.sqrt((given[i] + got[i]) / most);

    const g = [];
    // Strands under the flowers: fails bend wider than passes, so both of a pair show.
    for (const e of others) {
      for (const [verdict, count, bend] of [["pass", e.passes, 0.2], ["fail", e.fails, 0.32]]) {
        if (!count) continue;
        const d = arc(e.from, e.to, n, radius, bend);
        // Steep on purpose: when every pair has checked every other (a big Ground), the strongest paths
        // stand out and the rest recede, as the Loom's bundles do.
        const w = count / max[verdict];
        const widest = verdict === "pass" ? 6.5 : 3.5;
        const strand = S("path", { d, class: `strand ${verdict}`, "stroke-width": 0.6 + widest * Math.pow(w, 1.2), opacity: 0.06 + 0.84 * w * w });
        strand.dataset.edge = `${e.from}:${e.to}`;
        strand.addEventListener("pointerenter", (ev) => tip(ev, e));
        strand.addEventListener("pointermove", (ev) => tip(ev, e));
        strand.addEventListener("pointerleave", () => tip(null));
        strand.addEventListener("click", () => light({ edge: `${e.from}:${e.to}` }, true));
        g.push(strand);
        // Pollen drifts along counted passes, from the validator to the author.
        if (verdict === "pass" && e.counted.counted_passes > 0 && w >= 0.45) {
          for (let k = 0; k < Math.min(3, 1 + Math.floor(Math.sqrt(e.counted.counted_passes) / 2)); k++) {
            const grain = S("circle", { r: 2.4, fill: "var(--focus)", class: "pollen" });
            grain.style.setProperty("offset-path", `path("${d}")`);
            grain.style.setProperty("animation-delay", `${-k * 1.4 - (e.from * 0.37 + e.to * 0.19)}s`);
            grain.dataset.edge = `${e.from}:${e.to}`;
            g.push(grain);
          }
        }
      }
    }
    // The flowers.
    for (let i = 0; i < n; i++) g.push(flower(i, n, radius(i), given[i], got[i], own[i]));
    field.replaceChildren(...g);
    if (!list.length) field.append(S("text", { x: CX, y: CY, "text-anchor": "middle" }, "No evidence yet: nobody has checked anyone's work."));
    listPairs(list);
    applyLight();
    note();
  }

  /** A quadratic arc from flower a to flower b, always bending to the same side of its direction. */
  function arc(a, b, n, radius, bend) {
    const [x0, y0] = place(a, n), [x1, y1] = place(b, n);
    const dx = x1 - x0, dy = y1 - y0, len = Math.hypot(dx, dy) || 1;
    const nx = dy / len, ny = -dx / len; // to the left of the direction of flight
    const mx = (x0 + x1) / 2 + nx * len * bend, my = (y0 + y1) / 2 + ny * len * bend;
    // Start and end at the flowers' rims.
    const trim = (x, y, cx, cy, r) => {
      const l = Math.hypot(cx - x, cy - y) || 1;
      return [x + ((cx - x) / l) * r, y + ((cy - y) / l) * r];
    };
    const [sx, sy] = trim(x0, y0, mx, my, radius(a) + 4);
    const [ex, ey] = trim(x1, y1, mx, my, radius(b) + 6);
    return `M${sx.toFixed(1)},${sy.toFixed(1)} Q${mx.toFixed(1)},${my.toFixed(1)} ${ex.toFixed(1)},${ey.toFixed(1)}`;
  }

  function flower(i, n, r, given, got, own) {
    const [x, y, a] = place(i, n);
    const grp = S("g", { class: "flower", tabindex: 0, transform: `translate(${x.toFixed(1)} ${y.toFixed(1)})` });
    grp.dataset.agent = String(i);
    const ink = B.ink(i);
    // Petals: six, the agent's ink.
    for (let p = 0; p < 6; p++)
      grp.append(S("path", { d: `M0,0 C${-r * 0.42},${-r * 0.5} ${-r * 0.36},${-r * 1.2} 0,${-r * 1.42} C${r * 0.36},${-r * 1.2} ${r * 0.42},${-r * 0.5} 0,0 Z`, transform: `rotate(${p * 60})`, fill: ink, opacity: 0.32, stroke: ink, "stroke-width": 1 }));
    grp.append(S("circle", { class: "centre", r: r * 0.42, fill: ink, stroke: "var(--ground)", "stroke-width": 1.5 }));
    if (own) grp.append(S("circle", { r: r * 1.62, fill: "none", stroke: "var(--root-faint)", "stroke-width": 1, "stroke-dasharray": "2 3" }));
    // The name outside the ring, and what it gave and got.
    const out = r * 1.42 + 16;
    const lx = Math.cos(a) * out, ly = Math.sin(a) * out;
    const anchor = Math.abs(Math.cos(a)) < 0.3 ? "middle" : Math.cos(a) > 0 ? "start" : "end";
    const below = Math.sin(a) > 0.3 ? 12 : Math.sin(a) < -0.3 ? -14 : 4;
    grp.append(S("text", { x: lx.toFixed(1), y: (ly + below).toFixed(1), "text-anchor": anchor }, name(i)));
    grp.append(S("text", { x: lx.toFixed(1), y: (ly + below + 15).toFixed(1), "text-anchor": anchor, class: "count" }, `checked ${given} · checked by ${got}${own ? ` · own ${own}` : ""}`));
    grp.addEventListener("pointerenter", () => light({ agent: i }, false));
    grp.addEventListener("pointerleave", () => light(null, false));
    grp.addEventListener("focus", () => light({ agent: i }, true));
    grp.addEventListener("blur", () => light(null, true));
    return grp;
  }

  // ---- Lighting one agent's or one pair's strands ----
  let pinned = null;
  function light(what, pin) {
    if (pin) pinned = what && pinned && JSON.stringify(pinned) === JSON.stringify(what) ? null : what;
    lit = what || pinned;
    applyLight();
  }
  function applyLight() {
    const field = $("field");
    field.classList.toggle("lit", !!lit);
    const on = (edge) => !!lit && (lit.edge ? lit.edge === edge : edge.split(":").includes(String(lit.agent)));
    for (const el of field.querySelectorAll(".strand, .pollen")) el.classList.toggle("on", on(el.dataset.edge));
    for (const el of field.querySelectorAll(".flower")) {
      const i = el.dataset.agent;
      el.classList.toggle("on", !!lit && (lit.edge ? lit.edge.split(":").includes(i) : String(lit.agent) === i));
    }
    for (const el of $("pairs").querySelectorAll("li[data-edge]")) el.classList.toggle("on", on(el.dataset.edge));
  }

  function tip(ev, e) {
    const el = $("tip");
    if (!ev) {
      el.hidden = true;
      return;
    }
    el.replaceChildren(...describe(e));
    el.hidden = false;
    const box = el.parentElement.getBoundingClientRect();
    el.style.left = `${Math.min(box.width - 300, ev.clientX - box.left + 14)}px`;
    el.style.top = `${ev.clientY - box.top + 14}px`;
  }

  function describe(e) {
    const extra = counted ? "" : ` (${e.counted.counted_passes} and ${e.counted.counted_fails} counted)`;
    return [B.bdi(name(e.from)), " checked ", B.bdi(name(e.to)), `: ${plural(e.passes, "pass", "passes")}, ${plural(e.fails, "fail", "fails")}${extra}`];
  }
  const plural = (n, one, many) => `${n} ${n === 1 ? one : many}`;

  function listPairs(list) {
    const ul = $("pairs");
    if (!list.length) {
      ul.replaceChildren(B.h("li", { class: "empty" }, "No evidence yet."));
      return;
    }
    const sorted = list.slice().sort((p, q) => q.passes + q.fails - (p.passes + p.fails) || p.from - q.from || p.to - q.to);
    ul.replaceChildren(
      ...sorted.map((e) => {
        const self = e.from === e.to;
        const li = B.h(
          "li",
          { tabindex: 0 },
          B.h("span", { class: "who" }, B.bdi(name(e.from)), self ? " checked its own work" : [" → ", B.bdi(name(e.to))]),
          B.h("span", { class: "n num" }, `${e.passes} ✓ ${e.fails} ✗${counted || self ? "" : ` · ${e.counted.counted_passes + e.counted.counted_fails} counted`}`)
        );
        li.dataset.edge = `${e.from}:${e.to}`;
        li.addEventListener("click", () => light({ edge: li.dataset.edge }, true));
        li.addEventListener("keydown", (ev) => (ev.key === "Enter" || ev.key === " ") && (ev.preventDefault(), light({ edge: li.dataset.edge }, true)));
        return li;
      })
    );
  }

  function note(extra) {
    const n = trust.edges.reduce((s, e) => s + (counted ? e.counted_passes + e.counted_fails : e.passes + e.fails), 0);
    B.text($("data-note"), `${n.toLocaleString("en")} ${counted ? "counted " : ""}evidence · ${ground.agents.length} agents · ${extra || "live"}`);
  }

  // ---- The toggle ----
  function setCounted(on) {
    counted = on;
    B.attr($("all"), "aria-pressed", String(!on));
    B.attr($("counted"), "aria-pressed", String(on));
    draw();
  }
  $("all").addEventListener("click", () => setCounted(false));
  $("counted").addEventListener("click", () => setCounted(true));

  // ---- Live ----
  let again = 0;
  function listen() {
    const events = new EventSource(`/api/events?since=${encodeURIComponent(ground.cursor)}`);
    events.addEventListener("contributions", () => {
      clearTimeout(again);
      again = setTimeout(() => load().catch(() => note("couldn't read the Ground")), 400);
    });
    events.addEventListener("error", () => note("reconnecting…"));
    events.addEventListener("open", () => trust && note());
  }

  document.addEventListener("bracken-theme", () => trust && draw());
  document.querySelector(".masthead nav").append(B.themeButton());
  load().then(listen, () => B.text($("data-note"), "couldn't read the Ground"));
})();
