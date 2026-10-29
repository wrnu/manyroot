#!/usr/bin/env python3
"""export-sample: turns the throwaway Ground from make-sample-ground.sh into sample-data.js.

    python3 -I docs/design/visualizer/sample/export-sample.py <hub dir> [bracken binary]

It reads the Ground only through `bracken ... --json`, and builds the shapes in docs/design/visualizer.md
section 1 the way the server will: /api/ground, /api/graph, /api/status and /api/root/<id> for each task.
The mockups load the result as a plain script (file:// can't fetch), so it's `window.BRACKEN_SAMPLE = {...}`.

The sample was written in a few seconds, so its timestamps are respaced for display. Contributions keep
their real causal order and their real relative order in time, spread over about a working day. Every
name and statement is synthetic.
"""

import json
import os
import subprocess
import sys

here = os.path.dirname(os.path.abspath(__file__))
hub = sys.argv[1]
binary = sys.argv[2] if len(sys.argv) > 2 else os.path.join(here, "../../../../target/debug/bracken")
env = {k: v for k, v in os.environ.items() if k not in ("BRACKEN_AGENT",)}
env["BRACKEN_DIR"] = os.path.join(hub, ".bracken")


def bk(*args):
    out = subprocess.run([binary, *args, "--json"], env=env, check=True, capture_output=True, text=True).stdout
    return json.loads(out)


log = bk("log")
by_id = {c["id"]: c for c in log}

# The hub names only the identities it holds; alice, bob and carol hold theirs in their own replicas (siblings
# of the hub), so the sample names them from there, as a replica with their agent entries would.
names = {}
for sibling in sorted(os.listdir(os.path.dirname(os.path.abspath(hub)))):
    d = os.path.join(os.path.dirname(os.path.abspath(hub)), sibling, ".bracken")
    if os.path.isdir(d):
        e = dict(env, BRACKEN_DIR=d)
        out = subprocess.run([binary, "id", "list", "--json"], env=e, check=True, capture_output=True, text=True)
        names.update({a["agent"]: a["name"] for a in json.loads(out.stdout)})
for c in log:
    c["author_name"] = c["author_name"] or names.get(c["author"])

# Respace time: keep the real order of timestamps, about 6 to 30 minutes apart, from 08:40.
day0 = 1761640800000  # 2025-10-28T08:40:00Z, an invented day
order = sorted(log, key=lambda c: (c["timestamp_ms"], log.index(c)))
t = day0
for i, c in enumerate(order):
    gap = (6 + (int(c["id"][:4], 16) % 25)) * 60_000
    t += gap if i else 0
    c["timestamp_ms"] = t
now = t + 20 * 60_000

# Agents in order of first contribution, named locally.
agents, index = [], {}
for c in log:
    a = c["author"]
    if a not in index:
        index[a] = len(agents)
        agents.append({"agent": a, "name": c["author_name"], "contributions": 0, "first_ms": c["timestamp_ms"],
                       "last_ms": c["timestamp_ms"]})
    e = agents[index[a]]
    e["contributions"] += 1
    e["first_ms"] = min(e["first_ms"], c["timestamp_ms"])
    e["last_ms"] = max(e["last_ms"], c["timestamp_ms"])

heads = sorted(h["id"] for h in bk("heads"))
pos = {c["id"]: i for i, c in enumerate(log)}
depth = []
for c in log:
    depth.append(1 + max((depth[pos[p]] for p in c["parents"]), default=-1))
retracted = [pos[c["id"]] for c in log if c["retracted"]]

ground = {
    "ground": "0" * 64,
    "name": "auth (synthetic sample)",
    "now_ms": now,
    "contributions": len(log),
    "heads": heads,
    "cursor": ",".join(heads),
    "policy": {"min_passes": 1, "validators": []},
    "agents": agents,
}
graph = {
    "cursor": ",".join(heads),
    "ids": [c["id"] for c in log],
    "kind": [c["kind"] for c in log],
    "author": [index[c["author"]] for c in log],
    "parents": [[pos[p] for p in c["parents"]] for c in log],
    "time": [c["timestamp_ms"] for c in log],
    "depth": depth,
    "tags": [[t[:64] for t in c["body"].get("tags", [])] if c["kind"] == "claim" else [] for c in log],
    "refs": [],
    "retracted": retracted,
}

statuses = {}
for c in log:
    if c["kind"] not in ("claim", "artifact"):
        continue
    a = bk("show", c["id"])["assessment"]
    if a["status"] != "pending" or a["passes"] or a["fails"]:
        statuses[c["id"]] = {"status": a["status"], "passes": len(a["passes"]), "fails": len(a["fails"]),
                             "superseded_by": a["superseded_by"]}
status = {"policy": ground["policy"], "statuses": statuses, "conflicts": bk("conflicts")}


def first_line(text):
    return (text.splitlines() or [""])[0][:200]


def short(c):
    text = c["body"].get("statement") or c["body"].get("name") or ""
    return {"id": c["id"], "statement": first_line(text), "author_name": c["author_name"],
            "timestamp_ms": c["timestamp_ms"], "status": statuses.get(c["id"], {"status": "pending"})["status"]}


def lite(c):
    v = dict(c)
    if c["kind"] == "artifact":
        v["body"] = {k: x for k, x in c["body"].items() if k != "text"}
    return v


def evidence_on(target, assessment):
    counted = set(assessment["passes"]) | set(assessment["fails"])
    return [{"id": e["id"], "verdict": e["body"]["verdict"], "method": e["body"]["method"], "author": e["author"],
             "author_name": e["author_name"], "timestamp_ms": e["timestamp_ms"], "counted": e["id"] in counted,
             "retracted": e["retracted"]}
            for e in log if e["kind"] == "evidence" and e["body"]["target"] == target]


def relations(kind, to=None, frm=None):
    return [r for r in log if r["kind"] == "relation" and r["body"]["kind"] == kind
            and (to is None or r["body"]["to"] == to) and (frm is None or r["body"]["from"] == frm)]


def root(cid):
    shown = bk("show", cid)
    claim = lite(by_id[cid])
    claim["assessment"] = shown["assessment"]
    impls = []
    for rel in relations("implements", to=cid):
        art = by_id[rel["body"]["from"]]
        a = bk("show", art["id"])["assessment"]
        impls.append({"artifact": lite(art), "relation": rel["id"], "relation_retracted": rel["retracted"],
                      "assessment": a, "evidence": evidence_on(art["id"], a)})
    intents = []
    for rel in relations("refines", to=cid):
        i = by_id[rel["body"]["from"]]
        tags = i["body"].get("tags", []) if i["kind"] == "claim" else []
        until = [t for t in tags if t.startswith("until:")]
        if "intent" not in tags or not until or rel["author"] != i["author"]:
            continue
        # Respaced like everything else: the intent's own --for, from its respaced start.
        real_for = {"alice": 120, "bob": 30, "carol": 240}.get(i["author_name"], 60) * 60_000
        until_ms = i["timestamp_ms"] + real_for
        ended = [r for r in relations("implements", to=cid) if r["author"] == i["author"]
                 and r["timestamp_ms"] > i["timestamp_ms"] and not r["retracted"]]
        ended_ms, ended_by = None, None
        if i["retracted"]:
            ended_by = "retracted"
        elif ended:
            ended_ms, ended_by = min(r["timestamp_ms"] for r in ended), "implemented"
        elif until_ms <= now:
            ended_ms, ended_by = until_ms, "expired"
        intents.append({"id": i["id"], "author": i["author"], "author_name": i["author_name"],
                        "from_ms": i["timestamp_ms"], "until_ms": until_ms, "ended_ms": ended_ms, "ended_by": ended_by})
    rivals = []
    for rel in relations("contradicts"):
        other = rel["body"]["from"] if rel["body"]["to"] == cid else rel["body"]["to"] if rel["body"]["from"] == cid else None
        if other:
            rivals.append({**short(by_id[other]), "how": "contradicts", "relation": rel["id"]})
    lineage = {
        "replaces": [short(by_id[r["body"]["to"]]) for r in relations("supersedes", frm=cid)],
        "replaced_by": [{**short(by_id[r["body"]["from"]]), "counts": r["author"] == by_id[cid]["author"]}
                        for r in relations("supersedes", to=cid)],
    }
    return {"now_ms": now, "claim": claim, "lineage": lineage, "implementations": impls,
            "evidence": evidence_on(cid, shown["assessment"]), "intents": intents,
            "refinements": [short(by_id[r["body"]["from"]]) for r in relations("refines", to=cid)
                            if "intent" not in by_id[r["body"]["from"]]["body"].get("tags", [])],
            "rivals": rivals}


tasks = [c["id"] for c in log if "task" in c["body"].get("tags", [])]
sample = {
    "note": "Synthetic sample from a throwaway Ground (make-sample-ground.sh); timestamps respaced for display.",
    "ground": ground,
    "graph": graph,
    "status": status,
    "contributions": {c["id"]: lite(c) for c in log},
    "roots": {cid: root(cid) for cid in tasks},
}
out = os.path.join(here, "../sample-data.js")
with open(out, "w") as f:
    f.write("// Generated by sample/export-sample.py from a throwaway Ground. Synthetic data; don't edit.\n")
    f.write("window.BRACKEN_SAMPLE = ")
    json.dump(sample, f, indent=1, sort_keys=True)
    f.write(";\n")
print(out, len(log), "contributions,", len(agents), "agents")
