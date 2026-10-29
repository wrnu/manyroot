// Placeholder page for `bracken view`: the Ground's name, its agents and heads, kept current by the
// event stream. Ground text is untrusted, so it only ever goes in through textContent.
"use strict";

function item(text) {
  const li = document.createElement("li");
  li.textContent = text;
  return li;
}

async function load() {
  const response = await fetch("/api/ground");
  const ground = await response.json();
  document.getElementById("name").textContent = ground.name;
  document.getElementById("summary").textContent =
    `${ground.contributions} contributions by ${ground.agents.length} agents, ${ground.heads.length} heads`;
  document.getElementById("agents").replaceChildren(
    ...ground.agents.map((a) => item(`${a.name ?? "~" + a.agent.slice(0, 8)}: ${a.contributions}`)),
  );
  document.getElementById("heads").replaceChildren(...ground.heads.map((h) => item(h.slice(0, 12))));
  return ground.cursor;
}

load().then((cursor) => {
  const events = new EventSource(`/api/events?since=${encodeURIComponent(cursor)}`);
  events.addEventListener("contributions", () => load());
});
