// Weave (docs/design/weave-view.md): the Weave records of this replica, read from /api/weave. The home page
// lists the lines; a line shows its policy, heads, green and decisions; a decision shows its payload, its
// verdict and its counted check results.
//
// Every word here is another agent's, so it goes in only through Bracken.h(), as text. Before that, each
// invisible character (bidi controls, zero-width and other format characters) becomes a visible marker,
// and each control character a \u{..} escape, so no text reads differently from its bytes; common.js would
// otherwise strip the bidi controls silently. Moving between pages is a button that sets location.hash to
// a route this script builds from ids it has checked: no element here has an href.
"use strict";

(() => {
  const B = Bracken;
  const h = B.h;
  const $ = (id) => document.getElementById(id);
  const ID = /^[0-9a-f]{64}$/;
  const PAGE = 50;

  // ---- Text ----

  const INVISIBLE = new RegExp(
    "[\\u00ad\\u061c\\u180e\\u200b-\\u200f\\u202a-\\u202e\\u2060-\\u2069\\ufeff]|[\\u{e0000}-\\u{e007f}]|[\\u0000-\\u0008\\u000b-\\u001f\\u007f-\\u009f]",
    "gu",
  );

  /** Text as children for h(): its invisible and control characters shown, never hidden or obeyed. */
  function shown(value) {
    const text = String(value === null || value === undefined ? "" : value);
    const out = [];
    let last = 0;
    for (const m of text.matchAll(INVISIBLE)) {
      if (m.index > last) out.push(text.slice(last, m.index));
      const cp = m[0].codePointAt(0);
      const hex = cp.toString(16).toUpperCase().padStart(4, "0");
      const control = cp < 0x20 || (cp >= 0x7f && cp <= 0x9f);
      out.push(h("span", { class: "invisible", title: control ? "a control character" : "an invisible character" }, control ? `\\u{${cp.toString(16)}}` : `U+${hex}`));
      last = m.index + m[0].length;
    }
    if (last < text.length) out.push(text.slice(last));
    return out;
  }

  const short = (id) => String(id).slice(0, 12);
  const who = (key, name) => name || `~${String(key).slice(0, 8)}`;

  // ---- Routes ----

  /** A button that opens a page of this view: the route is built here, from a checked id. */
  function go(route, id, label) {
    if (!ID.test(String(id))) return h("span", { class: "id" }, shown(label === undefined ? id : label));
    return h(
      "button",
      { type: "button", class: label === undefined ? "go id" : "go", on: { click: () => { location.hash = `${route}/${id}`; } } },
      shown(label === undefined ? short(id) : label),
    );
  }

  function route() {
    const parts = location.hash.slice(1).split("/");
    const [name, id] = [parts[0], (parts[1] || "").toLowerCase()];
    if ((name === "line" || name === "decision") && ID.test(id)) return { name, id };
    return { name: "home" };
  }

  // ---- Reading ----

  /** Counts the routes shown: a read for an earlier route stops waiting when a later one starts. */
  let shownRoute = 0;

  // While the view reads the Weave records (at start, and after the replica changes) its Weave pages
  // answer 503 with a Retry-After: wait that long and ask again, for up to ten minutes, unless the reader
  // has moved to another page meanwhile.
  const WAIT_AT_MOST_MS = 10 * 60 * 1000;
  const sleep = (ms) => new Promise((done) => setTimeout(done, ms));

  async function api(path) {
    const mine = shownRoute;
    const until = Date.now() + WAIT_AT_MOST_MS;
    let r = await fetch(path, { headers: { Accept: "application/json" } });
    while (r.status === 503 && r.headers.get("Retry-After") && Date.now() < until && mine === shownRoute) {
      const seconds = Math.min(Math.max(Number(r.headers.get("Retry-After")) || 2, 1), 30);
      await sleep(seconds * 1000);
      if (mine !== shownRoute) break;
      r = await fetch(path, { headers: { Accept: "application/json" } });
    }
    if (!r.ok) {
      let message = `${r.status}`;
      try {
        message = (await r.json()).error || message;
      } catch (_) {}
      throw new Error(message);
    }
    return r.json();
  }

  function query(params) {
    return Object.entries(params)
      .filter(([, v]) => v !== undefined && v !== null)
      .map(([k, v]) => `${encodeURIComponent(k)}=${encodeURIComponent(v)}`)
      .join("&");
  }

  // ---- Building blocks ----

  function facts(pairs) {
    const dl = h("dl", { class: "facts" });
    for (const [k, v] of pairs) {
      if (v === undefined) continue;
      dl.append(h("dt", {}, k), h("dd", {}, v === null ? h("span", { class: "muted" }, "none") : v));
    }
    return dl;
  }

  /**
   * A paged list: the items an answer carried, then a button that fetches the next page from `more(after)`
   * until there are none. `key` gives an item's key for `after`; `row` builds its <li>.
   */
  function pagedList(first, key, row, more) {
    const ul = h("ul", { class: "rows" });
    const wrap = h("div", {}, ul);
    let items = first.items;
    let hasMore = first.more;
    const append = (list) => {
      for (const item of list) ul.append(row(item));
    };
    append(items);
    if (!items.length) ul.append(h("li", { class: "muted" }, "none"));
    const button = h("button", { type: "button", class: "more" }, `More (${first.total} in all)`);
    button.addEventListener("click", async () => {
      button.disabled = true;
      try {
        const next = await more(key(items[items.length - 1]));
        items = next.items;
        hasMore = next.more;
        append(items);
      } catch (e) {
        button.replaceWith(h("p", { class: "problem" }, shown(`Couldn't read more: ${e.message}`)));
        return;
      }
      button.disabled = false;
      if (!hasMore) button.remove();
    });
    if (hasMore) wrap.append(button);
    return wrap;
  }

  function section(title, ...body) {
    return [h("h3", {}, title), ...body];
  }

  function stateWord(green) {
    if (!green) return "unknown";
    return { one: "green", none: "no green decision yet", ambiguous: "green is ambiguous", undefined: "undefined: the policy has diverged" }[green.state] || green.state;
  }

  function checkRows(checks) {
    if (!checks) return h("p", { class: "muted" }, "No current policy, so nothing is counted.");
    if (!checks.length) return h("p", { class: "muted" }, "The policy names no checks.");
    const ul = h("ul", { class: "rows" });
    for (const c of checks) {
      const tally = [
        c.passes.length ? h("span", { class: "pass" }, `${c.passes.length} pass`) : null,
        c.fails.length ? h("span", { class: "fail" }, `${c.fails.length} fail`) : null,
        c.replaced.length ? h("span", { class: "muted" }, `${c.replaced.length} replaced by a later attempt`) : null,
        !c.passes.length && !c.fails.length ? h("span", { class: "muted" }, "no counted result") : null,
      ];
      ul.append(h("li", { class: c.green ? "green" : c.fails.length ? "failing" : "" }, h("span", { class: "sc" }, shown(c.check)), c.green ? "green" : "not green", ...tally));
    }
    return ul;
  }

  // ---- Pages ----

  async function home() {
    const w = await api("/api/weave");
    if (!w.weave) return [h("h2", {}, "No Weave records here"), h("p", { class: "lede" }, shown(w.why))];
    $("data-note").textContent = "";
    const out = [h("h2", {}, "Lines")];
    out.push(facts([
      ["marker", h("span", { class: "id" }, shown(short(w.marker)))],
      ["root", shown(who(w.root, w.root_name))],
    ]));
    if (w.default) {
      const d = w.default;
      out.push(...section("The default line",
        facts([
          ["line", go("line", d.line, d.name)],
          ["heads", d.heads.length ? h("span", {}, d.heads.map((x) => go("decision", x.decision))) : null],
          ["forked", d.forked ? "yes: a decision listing every head settles it" : "no"],
          ["green", stateWord(d.green)],
          ["open objections", d.open_objections === null ? null : String(d.open_objections)],
        ])));
    }
    const lineRow = (l) =>
      h("li", { class: l.default ? "green" : "" }, go("line", l.line, l.name), h("span", { class: "muted" }, shown(`by ${who(l.author, l.author_name)}`)),
        h("span", { class: "num" }, `${l.decisions} decisions`),
        l.reach !== "name" ? h("span", { class: "muted" }, l.reach === "id_only" ? "reached by id only" : "its name is shared") : null);
    out.push(...section("Every line", pagedList(w.lines, (l) => l.line, lineRow, (after) => api(`/api/weave?${query({ list: "lines", after, limit: PAGE })}`))));
    if (w.problems.total || w.unreadable.total) {
      const problemRow = (p) => h("li", { class: "problem" }, shown(p.text));
      out.push(...section("What weave check would list", pagedList(w.problems, (p) => String(p.at), problemRow, (after) => api(`/api/weave?${query({ list: "problems", after, limit: PAGE })}`))));
    }
    return out;
  }

  function policyFacts(v) {
    if (!v) return h("p", { class: "muted" }, "No policy version.");
    const agents = (list) => (list.length ? h("span", {}, shown(list.map((a) => who(a.agent, a.name)).join(", "))) : null);
    const checks = h("ul", { class: "rows" });
    for (const c of v.checks) checks.append(h("li", {}, h("span", { class: "sc" }, shown(c.check)), h("span", { class: "argv" }, shown(c.argv.join(" "))), c.executor ? h("span", { class: "muted" }, shown(`runs as ${c.executor.kind}`)) : null));
    if (!v.checks.length) checks.append(h("li", { class: "muted" }, "none"));
    return h("div", {},
      facts([
        ["version", h("span", { class: "id" }, short(v.version))],
        ["by", shown(who(v.author, v.author_name))],
        ["integrators", agents(v.integrators)],
        ["validators", agents(v.validators)],
        ["goal setters", agents(v.goal_setters)],
        ["guardrails", shown(`at most ${v.guardrails.max_known} known breakage, ${v.guardrails.max_age} moves old` + (v.guardrails.protected.length ? `; protected: ${v.guardrails.protected.join(", ")}` : ""))],
        ["owner decision", v.owner_decision === null ? null : h("span", { class: "id" }, shown(v.owner_decision))],
      ]),
      h("h3", {}, "Checks"), checks);
  }

  function decisionRow(d) {
    return h("li", { class: d.breakage.length ? "failing" : "" },
      go("decision", d.decision),
      h("span", { class: "muted" }, shown(`by ${who(d.author, d.author_name)}`)),
      d.source ? h("span", { class: "id" }, shown(`git ${String(d.source.git).slice(0, 12)}`)) : null,
      d.parents.length > 1 ? h("span", { class: "muted" }, `merges ${d.parents.length}`) : null,
      d.note ? h("span", { class: "note" }, shown(B.firstLine(d.note, 200))) : null);
  }

  async function line(id) {
    const l = await api(`/api/weave/line/${id}`);
    $("data-note").textContent = "";
    const policy = l.current.items[0];
    const out = [h("h2", {}, shown(l.name)),
      h("p", { class: "lede" }, shown(`A line by ${who(l.author, l.author_name)}${l.default ? ", the default line" : ""}. ${l.decisions} decisions.`))];
    out.push(facts([
      ["line", h("span", { class: "id" }, short(l.line))],
      ["heads", l.heads.length ? h("span", {}, l.heads.map((x) => go("decision", x.decision))) : null],
      ["forked", l.forked ? "yes: a decision listing every head settles it" : "no"],
      ["green", stateWord(l.green)],
      ["open objections", l.open_objections === null ? null : String(l.open_objections)],
    ]));
    if (l.green && l.green.heads.length) out.push(...section("Checks on the head", checkRows(l.green.heads[0].checks)));
    out.push(...section(l.diverged ? "Current policy (diverged: its author settles it)" : "Current policy", policyFacts(policy)));
    const log = await api(`/api/weave/log/${id}?${query({ limit: PAGE })}`);
    out.push(...section("Decisions, newest first", pagedList(log, (d) => d.decision, decisionRow, (after) => api(`/api/weave/log/${id}?${query({ after, limit: PAGE })}`))));
    const goalRow = (g) => h("li", { class: g.binds ? "green" : "" }, h("span", { class: "statement" }, shown(g.statement)), h("span", { class: "muted" }, shown(`by ${who(g.author, g.author_name)}${g.binds ? ", binds" : ", doesn't bind"}`)));
    out.push(...section("Goals", pagedList(l.goals, (g) => g.link, goalRow, (after) => api(`/api/weave/line/${id}?${query({ list: "goals", after, limit: PAGE })}`))));
    const versionRow = (v) => h("li", {}, h("span", { class: "id" }, short(v.version)), h("span", { class: "muted" }, shown(`${v.name}, by ${who(v.author, v.author_name)}`)));
    out.push(...section("Policy versions", pagedList(l.versions, (v) => v.version, versionRow, (after) => api(`/api/weave/line/${id}?${query({ list: "versions", after, limit: PAGE })}`))));
    return out;
  }

  async function decision(id) {
    const d = await api(`/api/weave/decision/${id}`);
    $("data-note").textContent = "";
    const verdict = d.verdict === "qualifies" ? h("span", { class: "pass" }, "qualifies") : h("span", { class: "fail" }, shown(d.verdict.unqualified ? `doesn't qualify: ${d.verdict.unqualified}` : String(d.verdict)));
    const out = [h("h2", {}, "Decision ", h("span", { class: "id" }, short(d.decision))),
      h("p", { class: "lede" }, "on ", go("line", d.line, d.line_name || short(d.line)), shown(` by ${who(d.author, d.author_name)}`))];
    out.push(facts([
      ["verdict", verdict],
      ["manifest", h("span", { class: "id" }, shown(d.manifest))],
      ["parents", d.parents.length ? h("span", {}, d.parents.map((p) => go("decision", p))) : null],
      ["policy in force", d.policy ? h("span", { class: "id" }, short(d.policy)) : null],
      ["git", d.source ? h("span", { class: "id" }, shown(d.source.git)) : null],
      ["proposals", d.proposals.length ? h("span", { class: "id" }, shown(d.proposals.map(short).join(" "))) : null],
      ["resolutions", d.resolutions.length ? h("span", { class: "id" }, shown(d.resolutions.map(short).join(" "))) : null],
    ]));
    if (d.note) out.push(...section("The integrator's note", h("p", { class: "text" }, shown(d.note))));
    out.push(...section("Checks, counted under the line's current policy", checkRows(d.counted.items)));
    if (d.breakage.length) {
      const ul = h("ul", { class: "rows" });
      for (const b of d.breakage) ul.append(h("li", { class: "failing" }, h("span", { class: "sc" }, shown(b.check)), h("span", {}, shown(b.detail)), b.paths.length ? h("span", { class: "argv" }, shown(b.paths.join(", "))) : null));
      out.push(...section("Known breakage it accepts", ul));
    }
    if (d.answers.length) {
      const ul = h("ul", { class: "rows" });
      for (const a of d.answers) ul.append(h("li", {}, h("span", { class: "id" }, short(a.objection)), h("span", { class: "sc" }, shown(a.disposition)), h("span", { class: "note" }, shown(a.reason))));
      out.push(...section("Objections it answers", ul));
    }
    const reviewRow = (r) => h("li", { class: r.kind === "objection" ? "failing" : "green" }, h("span", { class: "sc" }, r.kind), h("span", { class: "muted" }, shown(`by ${who(r.author, r.author_name)}`)), h("span", { class: "note" }, shown(r.note)));
    out.push(...section("Reviews", pagedList(d.reviews, (r) => r.review, reviewRow, (after) => api(`/api/weave/decision/${id}?${query({ list: "reviews", after, limit: PAGE })}`))));
    return out;
  }

  // ---- Showing ----

  async function render() {
    const r = route();
    const mine = ++shownRoute;
    const page = $("page");
    $("data-note").textContent = "Reading the Weave records…";
    let body;
    try {
      body = r.name === "line" ? await line(r.id) : r.name === "decision" ? await decision(r.id) : await home();
    } catch (e) {
      body = [h("h2", {}, "This page can't be shown"), h("p", { class: "problem" }, shown(e.message))];
    }
    if (mine !== shownRoute) return; // a later route won
    if (r.name !== "home") body.unshift(h("p", {}, go("home", "0".repeat(64), "All lines")));
    page.replaceChildren(...body.flat());
    window.scrollTo(0, 0);
  }

  // The home route is "#home/0000…": go() takes only checked ids, and the route reads it as home.
  window.addEventListener("hashchange", render);
  render();
})();
