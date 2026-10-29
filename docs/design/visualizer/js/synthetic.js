// Synthetic Grounds of any size, in the API's shapes (docs/design/visualizer.md section 1), to show how
// the views hold up at 1,000 and 50,000 contributions. Deterministic: the same size gives the same Ground.
// Labelled synthetic wherever it's shown.
"use strict";

const Synthetic = (() => {
  function prng(seed) {
    let a = seed >>> 0;
    return () => {
      a = (a + 0x6d2b79f5) >>> 0;
      let t = a;
      t = Math.imul(t ^ (t >>> 15), t | 1);
      t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
      return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
    };
  }

  const NAMES = ["owner", "manager", "builder-sync", "builder-demo", "builder-cli", "reviewer", "security",
    "builder-driver", "validator", "builder-docs", "builder-proto", "builder-ui"];
  const SUBJECTS = ["sync", "the merge gate", "intents", "the store", "bundles", "the log", "evidence", "the server",
    "cursors", "retractions", "the CLI", "the demo"];
  const VERBS = ["must refuse", "should batch", "keeps", "never drops", "verifies", "orders", "caches", "streams"];
  const OBJECTS = ["unknown ids", "stale heads", "large bundles", "concurrent writes", "every signature",
    "expired intents", "a closed pipe", "the causal parents"];

  function hex(r) {
    let out = "";
    for (let i = 0; i < 8; i++) out += Math.floor(r() * 0xffffffff).toString(16).padStart(8, "0");
    return out;
  }

  /** A Ground of `n` contributions by `agentCount` agents over about `days` days. */
  function make(n, agentCount = 12, days = n > 5000 ? 40 : 3) {
    const r = prng(n * 7919 + agentCount);
    const start = 1761552000000; // an invented day
    const span = days * 86400000;
    const agents = [];
    for (let i = 0; i < agentCount; i++)
      agents.push({ agent: hex(r), name: NAMES[i % NAMES.length], contributions: 0, first_ms: 0, last_ms: 0 });
    // Each agent's activity: a weight, and the hours it works.
    const weight = agents.map((_, i) => (i === 1 ? 2.2 : i === 0 ? 0.25 : 0.6 + r() * 1.4));
    const total = weight.reduce((a, b) => a + b, 0);
    // Who builds on whom: everyone reads the manager; each builder has a validator and a neighbour or two.
    const partners = agents.map((_, i) => {
      if (i === 1) return [2, 3, 4, 7, 9, 10, 11].filter((x) => x < agentCount);
      const list = [1, (i + 1) % agentCount, i % 2 ? 5 : 8, i % 3 ? 6 : 5].filter((x) => x !== i && x < agentCount);
      return list;
    });

    const ids = [], kind = [], author = [], parents = [], time = [], depth = [], tags = [], retracted = [];
    const last = new Array(agentCount).fill(-1);
    const subjects = []; // indexes of claims and artifacts, for evidence and relations to point at
    const statements = [];
    const statuses = {};
    let t = start;
    for (let i = 0; i < n; i++) {
      // Bursts and lulls: most gaps are short, a few are long.
      t += (span / n) * (r() < 0.08 ? 6 + r() * 10 : 0.2 + r() * 0.6);
      let pick = r() * total, a = 0;
      while (pick > weight[a] && a < agentCount - 1) pick -= weight[a++];
      const roll = r();
      const k = i < 3 ? "claim" : roll < 0.3 ? "claim" : roll < 0.42 ? "artifact" : roll < 0.7 ? "relation" : roll < 0.97 ? "evidence" : "retract";
      // Parents: the agent's own last contribution, plus what it last synced from one or two others.
      const ps = new Set();
      if (last[a] >= 0) ps.add(last[a]);
      const merges = r() < 0.55 ? 1 : r() < 0.2 ? 2 : 0;
      for (let m = 0; m < merges; m++) {
        const list = partners[a];
        const o = r() < 0.9 ? list[Math.floor(r() * list.length)] : Math.floor(r() * agentCount);
        if (last[o] >= 0) ps.add(last[o]);
      }
      if (ps.size === 0 && i > 0) ps.add(i - 1);
      const p = [...ps].sort((x, y) => x - y);
      ids.push(hex(r));
      kind.push(k);
      author.push(a);
      parents.push(p);
      time.push(Math.round(t));
      depth.push(p.length ? 1 + Math.max(...p.map((q) => depth[q])) : 0);
      const tg = k === "claim" ? (r() < 0.12 ? ["task"] : r() < 0.1 ? ["intent"] : r() < 0.05 ? ["decision"] : []) : [];
      tags.push(tg);
      statements.push(
        k === "artifact"
          ? `src/${SUBJECTS[Math.floor(r() * SUBJECTS.length)].replace(/^the /, "").replace(/ /g, "_")}.rs`
          : `${SUBJECTS[Math.floor(r() * SUBJECTS.length)]} ${VERBS[Math.floor(r() * VERBS.length)]} ${OBJECTS[Math.floor(r() * OBJECTS.length)]}`
      );
      if (k === "claim" || k === "artifact") subjects.push(i);
      if (k === "retract" && subjects.length) retracted.push(subjects[Math.floor(r() * subjects.length)]);
      if (k === "evidence" && subjects.length) {
        const target = subjects[Math.max(0, subjects.length - 1 - Math.floor(r() * 30))];
        const st = (statuses[ids[target]] ||= { status: "pending", passes: 0, fails: 0, superseded_by: [] });
        if (r() < 0.82) st.passes++;
        else st.fails++;
        st.status = st.fails ? "rejected" : "accepted";
      }
      last[a] = i;
      const e = agents[a];
      e.contributions++;
      if (!e.first_ms) e.first_ms = time[i];
      e.last_ms = time[i];
    }
    for (const i of retracted) if (statuses[ids[i]]) statuses[ids[i]].status = "retracted";
    const heads = [...new Set(last.filter((x) => x >= 0))].map((x) => ids[x]).sort();
    return {
      note: `Synthetic Ground of ${n.toLocaleString("en")} contributions, generated in the page.`,
      ground: { ground: "0".repeat(64), name: `synthetic, ${n.toLocaleString("en")}`, now_ms: Math.round(t + 600000),
        contributions: n, heads, cursor: heads.join(","), policy: { min_passes: 1, validators: [] }, agents },
      graph: { cursor: heads.join(","), ids, kind, author, parents, time, depth, tags, refs: [], retracted: [...new Set(retracted)] },
      status: { policy: { min_passes: 1, validators: [] }, statuses, conflicts: [] },
      statements,
    };
  }

  return { make };
})();
