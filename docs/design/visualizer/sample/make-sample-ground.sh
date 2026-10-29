#!/usr/bin/env bash
# make-sample-ground: builds the throwaway Ground the visualizer mockups use, with this checkout's bracken.
#
#   docs/design/visualizer/sample/make-sample-ground.sh [bracken binary]
#
# A small, invented story (an "auth" Ground): seven agents, claims, intents, artifacts that implement
# them, passing and failing evidence, a supersession, one that doesn't count (carol replacing the
# manager's claim), a contradiction and a retraction. Alice, bob and carol each work in their own replica
# and sync with a hub. Every name and statement is synthetic. It never touches the team Ground: the
# replicas live in a fresh temporary directory, and BRACKEN_DIR points into it for every command. It
# prints the hub's directory; export-sample.py turns that into sample-data.js.
set -euo pipefail

bin=${1:-target/debug/bracken}
bin=$(cd "$(dirname "$bin")" && pwd)/$(basename "$bin")
work=$(mktemp -d)
unset BRACKEN_DIR BRACKEN_AGENT
cd "$work"

# The hub holds the manager, the owner and the two validators; alice, bob and carol each work in their own
# replica and sync with the hub now and then, so their threads run concurrently, as on a real team.
h() { BRACKEN_DIR="$work/hub/.bracken" "$bin" "$@"; }
r() { local who=$1; shift; BRACKEN_DIR="$work/$who/.bracken" "$bin" "$@" --as "$who"; }
sync() { BRACKEN_DIR="$work/$1/.bracken" "$bin" sync "$work/hub" >/dev/null; }
mkdir -p hub && (cd hub && "$bin" init --name auth >/dev/null)
for who in owner manager val maya; do h id new "$who" >/dev/null; done
vision=$(h claim "Sessions are short-lived and every token can be revoked" --tag vision --as owner)
h export "$work/seed.mrb" 2>/dev/null
for who in alice bob carol; do
  mkdir -p "$who" && (cd "$who" && "$bin" join "$work/seed.mrb" >/dev/null) && BRACKEN_DIR="$work/$who/.bracken" "$bin" id new "$who" >/dev/null
done

task1=$(h claim "Tokens expire after 15 minutes of inactivity" --tag task --as manager)
h relate "$task1" refines "$vision" --as manager >/dev/null
task2=$(h claim "Revoking a session ends it everywhere within a minute" --tag task --as manager)
h relate "$task2" refines "$vision" --as manager >/dev/null
task3=$(h claim "Log every refused token with its reason, never the token itself" --tag task --as manager)
h relate "$task3" depends_on "$task1" --as manager >/dev/null
for who in alice bob carol; do sync "$who"; done

# Three threads at once.
r alice intend "$task1" --for 2h >/dev/null
r bob intend "$task2" --for 30m >/dev/null
rival=$(r carol claim "Revocation can wait for the next token refresh; a minute is too strict" --tag proposal)
note=$(r alice claim "The session store already keeps last_seen; expiry can read it" --tag note)
r alice relate "$note" supports "$task1" >/dev/null
r carol relate "$rival" contradicts "$task2" >/dev/null
printf 'pub const IDLE: u64 = 15 * 60;\n' >ttl.rs
impl1=$(r alice artifact ttl.rs --name src/session/ttl.rs --media-type text/x-rust --implements "$task1" | head -1)
printf 'fn revoke(id: SessionId) { BUS.publish(Revoked(id)); }\n' >revoke.rs
impl3=$(r bob artifact revoke.rs --name src/session/revoke.rs --media-type text/x-rust --implements "$task2" | head -1)
oops=$(r bob claim "Store refused tokens in the audit log for replay")
for who in alice bob carol; do sync "$who"; done

# The validators check what arrived, while the builders carry on.
h evidence "$impl1" pass --method "cargo test session::ttl" --as val >/dev/null
h evidence "$impl1" fail --method "review: idle clock resets on refresh" --details "a refresh request extends the session" --as maya >/dev/null
h evidence "$impl3" pass --method "cargo test session::revoke" --as val >/dev/null
h evidence "$rival" pass --method "load test: refresh every 45s" --as val >/dev/null
r bob retract "$oops" --reason "would log the token itself; see the task" >/dev/null
alt1=$(r carol claim "Tokens expire after 30 minutes of inactivity")
r carol relate "$alt1" supersedes "$task1" >/dev/null
for who in alice bob carol; do sync "$who"; done

printf 'pub const IDLE: u64 = 15 * 60; // refresh no longer touches last_seen\n' >ttl2.rs
impl2=$(r alice artifact ttl2.rs --name src/session/ttl.rs --media-type text/x-rust)
r alice relate "$impl2" supersedes "$impl1" >/dev/null
r alice relate "$impl2" implements "$task1" >/dev/null
r carol intend "$task3" --for 1h >/dev/null
r carol claim "Refusals are counted per reason in /metrics" --tag note >/dev/null
h evidence "$task2" fail --method "chaos test: one replica kept serving for 3 minutes" --as maya >/dev/null
for who in alice bob carol; do sync "$who"; done

h evidence "$impl2" pass --method "cargo test session::ttl" --as val >/dev/null
h evidence "$impl2" pass --method "review" --as maya >/dev/null
h evidence "$task1" pass --method "acceptance: idle session expired at 15m" --as val >/dev/null
for who in alice bob carol; do sync "$who"; done

work="$work/hub"
echo "$work"
