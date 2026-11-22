# Software factories with AI coding agents: the field, and where Manyroot/Bracken/Weave stands

> Date: 2026-11-21. Part of the November 2026 research behind [docs/design/layers.md](../../design/layers.md).
> Question: What do software factories built from AI coding agents look like across the field, what evidence backs them, and where do Manyroot, Bracken and Weave stand among them?

*Research report for the owner, 2026-11-21. External sources are linked next to the claims they support. **Vendor-reported** means the company is describing its own system and nobody else has measured it. **Unverified** means I found it only in secondary coverage or a search snippet. Our own figures come from the repo docs and Ground claims, cited by id.*

---

## 1. Executive summary

1. **The field.** "Software factory" now means a planner or manager agent, workers isolated in their own worktree, container or VM, automated verification, and a human who merges or sets goals. OpenAI, Cursor, Anthropic, Cognition, Factory and StrongDM all run versions of it.
2. **The evidence is thin.** The large-scale successes (OpenAI's roughly 1M-line product, Cursor's browser, Anthropic's C compiler) are self-reports on the builders' own projects, with no control group and no defect rates.
3. **Independent evidence leans against parallel agents.** CooperBench finds two coordinating agents 30% *less* successful than one agent doing both tasks. MAST attributes most multi-agent failures to specification and verification. Agent-authored PRs merge less often than human ones.
4. **The field is converging on what we already do.** Writes stay single-threaded per task. An independent verifier shares no context with the author, a holdout check can't be gamed, and AGENTS.md and docs are the system of record.
5. **Our real distinctive piece is the gate.** Acceptance is computed from signed, causally ordered, independent evidence, and `needs_recheck` withdraws acceptance when a dependency changes. Nobody else has this as a first-class, queryable layer. The nearest are attestation proposals (in-toto, Agent Trace) and a 2026 paper proposing "content-addressed evidence graphs".
6. **What isn't proven.** That the Ground or Weave improves outcomes. Phase 1 showed Bracken mode at 3–4x the tokens. The ledger pilot shows Weave at 101/101 vs git's 91/101, but on one run per arm, a toy project, and an integrator-timing artefact.
7. **Our weakest point is throughput and ceremony.** Our gate produces about 5 findings per PR and 3–21 PRs merged a week. OpenAI reports 3.5 PRs per engineer per day with agent-to-agent review and minimal blocking gates.
8. **What to borrow.** Holdout and scenario tests, written "done" contracts before code, worker-to-planner handoffs, reviewer precision tracking, "green branch" tolerance in place of serialising every merge, and doc-gardening agents.
9. **Git isn't the wall.** Anthropic replaced git internally (Scion, about 30k merges a day) for *scale* reasons, not semantics. Most agent pain is coordination and verification, not the storage format. That weakens the "replace git" pitch for Weave and strengthens the "evidence layer" pitch.
10. **Recommendation.** Make the evidence layer the product. Run bigger and harder evals before investing further in Weave as a git replacement, and cut process cost.

---

## 2. The landscape

### 2a. Hierarchical "factories" (planner → workers → verifier)

| System | Coordination | Verification | Measured vs claimed |
|---|---|---|---|
| **OpenAI "harness engineering"** ([post](https://openai.com/index/harness-engineering)) | Several Codex agents. Each git worktree boots its own app plus an observability stack. Runs often last over 6 hours. | Agents review each other until satisfied. Humans "may review… aren't required to". Custom linters' error messages tell the agent how to fix the problem. Blocking gates kept minimal, flaky tests re-run. | 3 people, later 7, over 5 months. About 1M lines, about 1,500 PRs, 3.5 PRs per engineer per day, "0 lines of manually-written code". Vendor-reported, no control group. They spent 20% of the week on "slop" cleanup until they automated it. |
| **Cursor long-running agents** ([scaling-agents](https://cursor.com/blog/scaling-agents), [self-driving codebases](https://cursor.com/blog/self-driving-codebases)) | Six architectures tried. Flat peers with lock files failed: 20 agents worked like 2–3. What worked: a root planner that writes no code, recursive sub-planners, isolated workers that hand off upward. The integrator and judge agents were *removed* as bottlenecks. | No formal verification. They accept a small, constant error rate and propose a periodic "green branch" fix-up, because requiring every commit to be correct serialised the whole system. | About 1,000 commits an hour, and over 1M lines for a browser in about a week. Critics: the browser is not production-grade ([Willison](https://simonwillison.net/2026/Jan/23/fastrender/)). Vendor-reported. |
| **Anthropic C compiler** ([post](https://www.anthropic.com/engineering/building-c-compiler)) | 16 agents in Docker containers with no orchestrator. Tasks are claimed with lock files in git, and push conflicts serialise the claims. | "The verifier must be nearly perfect." GCC is used as an oracle, and a fast deterministic test subset. | About $20k and about 2,000 sessions. Boots Linux and passes 99% of the GCC torture suite, but optimised output is slow and "new features… frequently broke existing functionality". Self-reported but concrete and checkable. |
| **Anthropic long-running app harness** ([post](https://www.anthropic.com/engineering/harness-design-long-running-apps)) | Planner, generator and evaluator. | **Sprint contracts**: generator and evaluator agree testable done-criteria before coding. A separate evaluator drives the app with Playwright, because self-graders "confidently prais[e]… mediocre" work. | One task: solo agent took 20 minutes and $9 and produced a broken game; the harness took 6 hours and $200 and the game worked. n=1. |
| **Cognition Devin** ([what's working](https://cognition.com/blog/multi-agents-working), [Devin Bots](https://dev.to/paullovelace/cognition-just-gave-devin-a-manager-devin-bots-run-teams-of-ai-coders-for-weeks-21ch)) | Moved from "Don't build multi-agents" ([2025](https://cognition.com/blog/dont-build-multi-agents)) to "writes stay single-threaded; extra agents contribute intelligence, not actions". Managers of Devins, then persistent Devin Bots (launched 17 Nov 2026). | Devin Review works best when the reviewer shares *no context* with the coder, and the coder decides which findings to act on, to avoid endless review loops. | "2 bugs per PR, 58% severe". "50→1,900 PRs/week with one human". Both vendor-reported. The 4.5x figure counts PRs per message, not quality ([critique](https://cellcog.ai/blog/devin-bots/)). |
| **Factory Missions** ([news](https://factory.ai/news/missions)) | An orchestrator splits work into milestones. | Validators with no worker context check against a validation contract written before the code. | Terminal-Bench scores self-reported. Factory admits the orchestrator over-scopes. |
| **StrongDM "software factory"** ([Willison](https://simonwillison.net/2026/Feb/7/software-factory/), [site](https://factory.strongdm.ai/)) | "Code must not be written by humans… must not be reviewed by humans". At least $1,000 of tokens per engineer per day. | **Scenarios kept outside the repo as a holdout set.** An LLM-judged "satisfaction" score. A "digital twin" of SaaS dependencies. | No results published. Stanford CodeX questions independence when the same model writes and judges ([link](https://law.stanford.edu/2026/02/08/built-by-agents-tested-by-agents-trusted-by-whom/)). |
| **Gas Town** (Yegge; [explainer](https://justin.abrah.ms/blog/2026-01-05-wrapping-my-head-around-gas-town.html)) | A Mayor that writes no code, disposable workers, a "Refinery" merge queue, a "Witness" that watches for stuck agents. Work tracked in git-backed JSONL issues ("Beads"). | Quality gates in the Refinery. | About $100 an hour ([DoltHub](https://www.dolthub.com/blog/2026-01-15-a-day-in-gas-town/)). "100% vibe coded". No quality data. |

### 2b. Parallel single-task agents (one task, one sandbox, one PR)

- **Tools.** Codex cloud and app ([intro](https://openai.com/index/introducing-codex/)), GitHub Copilot coding agent with Agent HQ ([docs](https://docs.github.com/en/copilot/concepts/agents/coding-agent/about-coding-agent), [Agent HQ](https://github.blog/news-insights/company-news/welcome-home-agents/)), Google Jules ([launch](https://blog.google/innovation-and-ai/models-and-research/google-labs/jules/)), Cursor cloud agents, and Amp. Launchers such as Conductor, Vibe Kanban and claude-squad run one worktree per agent.
- **Shared design.** None coordinates agents with each other. The human is the integrator, and branch protection or CI is the gate.
- **Claude Code agent teams** ([docs](https://code.claude.com/docs/en/agent-teams)) add a shared task list, mailboxes and hook-based gates. There is no file isolation by default, and the docs recommend 3–5 teammates.

### 2c. Research frameworks

- **Role-play and conversation frameworks.** MetaGPT, ChatDev, AutoGen, CrewAI and LangGraph have no factory-scale results on real repos.
- **Agentless** ([paper](https://arxiv.org/abs/2407.01489)) is the standing counterexample: a fixed localise → repair → validate pipeline is competitive at low cost.
- **OpenHands SDK** ([paper](https://arxiv.org/abs/2511.03690)) runs large refactors by dependency order, leaf modules first, in PR-sized batches with humans reviewing each batch ([blog](https://www.openhands.dev/blog/automating-massive-refactors-with-parallel-agents)).

### 2d. Independent measurement (the part to trust most)

**Parallel agents on one repo**
- **CooperBench** ([arXiv 2601.13295](https://arxiv.org/abs/2601.13295), verified): over 600 tasks. Two agents on features that might conflict score on average **30% lower** than one agent doing both. Their messages are "vague, ill-timed, and inaccurate", and they break commitments. Communication eats up to 20% of the budget; it cuts conflicts but doesn't raise success (that detail is from the repo, not the abstract).
- Similar results: ParallACT ([2611.00314](https://arxiv.org/abs/2611.00314), unverified snippet: Claude Code fell from 62% to 16% on contradictory task pairs without an orchestrator) and "Passes Alone, Fails Together" ([2609.25396](https://arxiv.org/pdf/2609.25396)), on semantic conflicts that hide behind clean textual merges.

**Why multi-agent systems fail**
- **MAST** ([2503.13657](https://arxiv.org/abs/2503.13657)): about 42% of failures are specification or system design, about 37% are misalignment between agents, and about 21% are verification.
- Better base models alone won't fix them.

**Cost**
- Anthropic's research system used about 15x the tokens of chat, and token usage explained 80% of performance variance ([post](https://www.anthropic.com/engineering/multi-agent-research-system)). Anthropic says the approach suits interdependent coding *less* well.
- At equal budget, single agents often match multi-agent setups: [2601.12307](https://arxiv.org/abs/2601.12307), [2611.12953](https://arxiv.org/abs/2611.12953).

**Agent PRs in the wild**
- **AIDev** ([2507.15003](https://arxiv.org/abs/2507.15003)): merge rates are Codex 64%, Devin 49% and Copilot 35%, against 76.8% for humans.
- Reverts of agent PRs run 0.7% (Codex) to 7.6% (Copilot) ([ACM](https://dl.acm.org/doi/10.1145/3793302.3793587)).

**Developer productivity**
- **METR**: experienced developers were 19% *slower* in early 2025 ([study](https://metr.org/blog/2025-07-10-early-2025-ai-experienced-os-dev-study/)).
- The Feb 2026 follow-up shows likely speedups, but both confidence intervals cross zero and METR calls it "very weak evidence". The design broke down because developers refused to work without AI and couldn't time parallel agents ([update](https://metr.org/blog/2026-02-24-uplift-update/)).

**Benchmarks are unreliable**
- OpenAI stopped using SWE-bench Verified: 59% of the audited tasks had defective tests, and every frontier model showed contamination ([post](https://openai.com/index/why-we-no-longer-evaluate-swe-bench-verified/)).
- Scores on SWE-bench Pro are 10–30 points higher with a vendor's own scaffolding than with the standard one.

**Reviewers and test gaming**
- LLM judges prefer their own output ([2404.13076](https://arxiv.org/abs/2404.13076)).
- Field data on review bots: CodeRabbit had 36% of comments accepted, and 43% of its rejected comments were false positives ([2607.03316](https://arxiv.org/abs/2607.03316)). PRs reviewed only by agents merged 45% of the time, against 68% with human review ([2604.03196](https://arxiv.org/html/2604.03196v1)).
- **I found no controlled study showing that cross-model review beats same-model review.** It is plausible, not measured.
- **Test gaming.** On ImpossibleBench, GPT-5 exploited contradictory tests 76% of the time, including by editing them. Hidden or read-only tests largely stop it ([2510.20270](https://arxiv.org/abs/2510.20270)).

**Organisations**
- DORA 2025 found AI adoption associated with higher throughput *and* higher instability ([DORA](https://dora.dev/insights/balancing-ai-tensions/)). These are survey correlations.

---

## 3. Common patterns and known failure modes

### Patterns that recur across independent builders

1. **Isolation per task.** A worktree, container or VM for each task, which every vendor does. Worktrees isolate files but not ports, databases or caches. Trigger.dev moved to GitButler partly for that reason ([blog](https://trigger.dev/blog/parallel-agents-gitbutler)).
2. **Hierarchy over swarm.** A planner that doesn't code, workers that don't talk to each other, and handoffs up the tree (Cursor, Cognition, Gas Town, Factory). Flat peer coordination failed at Cursor and in CooperBench.
3. **Single writer per unit of work.** Cognition says it outright, and Cursor and OpenAI follow it in practice.
4. **Verifier separate from the author, sharing no context with it.** Cognition, Anthropic, Factory and StrongDM all do this. Increasingly the verifier is *executable*: Playwright, a GCC oracle, scenario holdouts.
5. **Contracts before code.** Anthropic's sprint contracts and Factory's validation contracts.
6. **The repo is the memory.** A short AGENTS.md acts as a table of contents into `docs/` (OpenAI). Evidence on AGENTS.md is mixed: ETH found context files raise cost by 20–23% with no general gain ([2602.11988](https://arxiv.org/abs/2602.11988)), and another study found 28.6% faster runs ([2601.20404](https://arxiv.org/abs/2601.20404)).
7. **Gates that teach.** Linter messages that tell the agent how to fix the problem, few blocking gates, and fix-forward over block (OpenAI, Cursor).
8. **Background maintenance agents.** Doc-gardening, small refactor PRs, and "golden principles" (OpenAI).

### Failure modes that recur

- **Duplicated work and conflicting implicit decisions.** Cognition's Flappy Bird example, CooperBench's broken commitments.
- **Integrator and merge-agent bottlenecks.** Cursor removed theirs. Our own e3gc run lost four items when the integrator ran out of calls mid-merge (note `3dbc98125511`).
- **Managers that over-prescribe or assume shared state.** Reported by Cognition and Factory.
- **Grading one's own work, and editing tests.** Anthropic's self-graders, ImpossibleBench.
- **Slop accumulating.** OpenAI's 20% cleanup tax, and the C compiler breaking existing features.
- **Cost blow-ups.** Anthropic's 15x tokens, StrongDM's $1k a day, Gas Town's $100 an hour.
- **Benchmark contamination.** It makes most leaderboard deltas uninformative.

---

## 4. How our system maps onto a software factory

| Factory function | What others use | Our piece | Fit |
|---|---|---|---|
| Work queue, claiming | Lock files in git (Carlini), JSONL Beads (Gas Town), Claude task list | Ground `task` claims, `intend` (with expiry), queue view (agents.md), `bracken watch` | Good. It is signed and survives restarts. The queue view is still partly unbuilt. |
| Isolation | Worktrees, containers, VMs | T3 worktrees, boat.dev boxes for tests, the eval runner's sandbox | On par. Nothing new here. |
| Shared memory / knowledge | AGENTS.md + docs/, NOTES.md, Devin Bot memory, StrongDM's cxdb | The Ground: decisions, gotchas, preferences and references as typed, superseding claims with `--current` views | **Stronger than most.** It is structured, queryable, attributable and versioned. Cost is the risk: Phase 1 showed reading the Ground dominated token use. |
| Handoffs | Worker → planner reports (Cursor), trajectory sharing (Cognition) | `report` claims, `question` → `refines` answer, ids in messages | Good. Handoffs are explicit and auditable. |
| Verification / gate | CI, agent reviewers, sprint contracts, holdout scenarios | `scripts/merge`: policy over independent evidence, cross-model reviewer for risky classes, security reviewer, counted run by the manager, author evidence never counts | **Distinctive.** The acceptance rule is formal, computed, and shows what has and hasn't been verified. |
| Staleness of approvals | Re-review on rebase (ad hoc); "Assay" paper ([2609.36170](https://arxiv.org/pdf/2609.36170)) proposes recomputing it | `needs_recheck` via dependency closure and causal order (acceptance-follows-changes.md) | **Ahead of the field.** Opt-in, not yet on our own merge gate. |
| Provenance / audit | in-toto/SLSA predicates under discussion ([#604](https://github.com/in-toto/attestation/issues/604)), Git AI notes, Agent Trace RFC, gitsign | Every contribution signed and content-addressed with causal parents; evidence bound to the exact sha | Conceptually ahead. In practice keys are shared in v0, so signatures record intent, not proof (weave.md). |
| Human approval | Branch protection, approval tools (`needs_approval`), Cedar/OPA policies | A short owner-gated list, signed `approval` claims, everything else a manager `decision` | Clear and narrow. Better scoped than most. |
| Code substrate / merge | git + worktrees, GitButler, jj, Graphite stacks; Scion at Anthropic for scale | Weave: file versions, manifests, lines, integrator decisions, main and green as queries, per-file merges, all as Ground records | Interesting but unproven. It overlaps git and jj and adds evidence-bound lines. |
| Process improvement | Mostly informal ("remove one harness piece per model") | Time-boxed experiments with metric, baseline, target and expiry; weekly metrics claims | **Unusual and good.** Nobody else publishes this discipline. |
| Evaluation | Internal evals; SWE-bench variants; CooperBench | Eval suite: hidden tests, golden solution, configurations, replicates; Phase 1 and 2; the ledger pilot | The right shape. Still small n. |

**Concretely, for a factory:**
- **Manyroot** is the *control plane and record*: the queue, the knowledge, the evidence and the acceptance policies.
- **Bracken** is the *agent-facing interface* to it: the CLI, status views and `watch`.
- **Weave** is an *optional code store* whose main advantage is that a line's head and "green" are defined by evidence in the same graph.

The first two are useful with git underneath. Weave is the part that has to prove its value.

---

## 5. What we do that others don't, or do better (and how sure we are)

1. **Acceptance as a computed policy over independent, signed evidence.**
   - Others have reviewers. We have a rule: author evidence never counts, the evidence is bound to the sha, and the policy is per class.
   - The field's direction (Cognition's reviewers with no shared context, Factory's validators, StrongDM's holdouts) arrives at the same principle informally.
   - *Unproven:* whether it lowers defects. Our metrics show 0 reverts, but nothing is shipped to users, so the "reached users" figure is 0 by construction.
2. **`needs_recheck`.** Acceptance is withdrawn when something upstream changes. The replay on the Phase 2 pilot would have flagged 9 of the 14 wrong functions.
   - This answers the "Passes Alone, Fails Together" and merge-skew problem at the level of the record.
   - *Unproven:* it is off on our own gate, and it has not been tested in a live run.
3. **Memory that persists and can be queried, with supersession.**
   - Decisions, gotchas and preferences are first-class and carry `--current`. Others use Markdown files and chat.
   - *Edge plausible, cost proven:* Phase 1 Bracken mode used 3–4x the tokens, mostly from reads (phase1-bracken-lessons.md). The fixes (status view, `--since`, short ids) since landed but haven't been re-measured at scale.
4. **Cross-model independent review on risky classes.**
   - Gemini reviews Claude's work. Few others publish this; Cursor's best-of-n and Amp's Oracle are the nearest.
   - *Unproven:* there is no controlled evidence that cross-model review beats a fresh-context reviewer on the same model. Our d64c3d545e81 baseline (420 findings in 5 days, many low) suggests precision is the bigger problem.
5. **Governed process change.** Experiments with metrics and expiry, the one-in-one-out rule, and weekly metrics. No vendor describes anything this disciplined. The value is in avoiding process creep, and our 12 KB AGENTS.md cap is evidence that it works.
6. **Vendor and harness neutrality.** A CLI-only door, and the eval runs both Pi with DeepSeek and Claude Code. Most factories are tied to one vendor.
7. **Honest evaluation design.**
   - Hidden tests and golden solutions are kept out of agents' reach, with replicates, and results are stated with their limits.
   - That is *better methodology* than every vendor write-up above. It is also why our results are modest: we measure.

**Where an apparent edge is not real:**
- **Isolation, worktrees and task-per-PR.** Everyone does this.
- **Weave's conflict-free recording.** The field's experience (CRDT studies [2510.18893](https://arxiv.org/abs/2510.18893), [AgentRoom](https://arxiv.org/pdf/2608.23740v1)) is that converging bytes doesn't converge meaning. Our own Phase 1 CRDT mode broke the library 32–47 times a run.
- **The 101/101 vs 91/101 ledger result.** It is one run per arm, and git's loss came from integrator timing, not git itself (note `63ca18dbfdf7`).

---

## 6. What we're missing or doing worse, with ideas worth borrowing

| # | Idea | Source | What it would mean here | Cost / benefit |
|---|---|---|---|---|
| 1 | **Holdout / scenario verification for our own product**, not only for eval projects | StrongDM ([Willison](https://simonwillison.net/2026/Feb/7/software-factory/)), ImpossibleBench ([2510.20270](https://arxiv.org/abs/2510.20270)) | A set of end-to-end CLI scenarios builders can't see or edit, run by the manager as a counted check. Builders can edit tests today; the rule "never weaken a test" is only a norm. | Medium cost (write about 30 scenarios, keep them outside the repo). High benefit: the strongest known defence against test gaming. |
| 2 | **Done-contracts before code** | Anthropic sprint contracts ([post](https://www.anthropic.com/engineering/harness-design-long-running-apps)), Factory validation contracts | The task brief carries numbered, testable done-criteria, and reviewers verify against them rather than reading freely. That directly addresses our review-overfit problem (d64c3d545e81). | Low cost. Likely fewer, sharper findings and fewer review rounds. |
| 3 | **Measure reviewer precision** | Field data on review bots ([2607.03316](https://arxiv.org/abs/2607.03316), [2604.03196](https://arxiv.org/html/2604.03196v1)) | We already triage findings as confirmed or rejected. Publish per-reviewer and per-model precision, and severity of confirmed findings, in `scripts/metrics`. Then decide whether the Gemini reviewer earns its cost. | Very low cost. It turns "cross-model review" from belief into data. |
| 4 | **Fewer blocking gates; fix forward, with a "green" line** | OpenAI harness engineering, Cursor ([self-driving](https://cursor.com/blog/self-driving-codebases)) | Our gate is heavy: every PR gets security review plus manager counted run, risky classes add a reviewer, up to 3 rounds. Merges were 21, 4 and 3 a week, against OpenAI's 3.5 per engineer per day. Weave's main/green split *already* models tolerated breakage, so use it: low-risk classes merge on builder evidence plus one independent pass, and green guards releases. | Low engineering cost, moderate risk. Potentially large throughput gain. Run it as a time-boxed experiment. |
| 5 | **Gates that tell the agent how to fix the problem** | OpenAI (custom linter messages) | Each `scripts/merge` refusal and each guard names the exact next command. The eval stage 4 already does this ("refusals name their way out"); extend it to the team's tooling. | Low cost. Fewer manager round-trips. |
| 6 | **Background gardening agents** | OpenAI doc-gardening and golden-principles refactor PRs | A scheduled low-risk agent that prunes stale docs (`docs/archive`), dead gotchas (several duplicate the `--supersedes` two-ids gotcha) and superseded process claims. | Needs owner approval for a schedule and budget. Medium benefit: Ground reads stay cheap. |
| 7 | **Worker → planner handoff format** | Cursor (done / concerns / deviations), Cognition (coder decides which review findings to act on) | Standardise the `report` claim's fields. Let builders *decline* findings with a reason, which the manager adjudicates, rather than answering every one. | Low cost. Shorter review loops. |
| 8 | **Provenance in an interoperable format** | in-toto attestation proposals ([#604](https://github.com/in-toto/attestation/issues/604)), Agent Trace RFC, gitsign/Sigstore | Export a merged PR's Ground evidence as an in-toto statement (DSSE-signed). That makes our evidence legible to outsiders and positions Manyroot as the evidence store behind those formats. | Medium cost. Strategic benefit, since this is where the standards fight is. Per-agent keys are a prerequisite for it meaning anything. |
| 9 | **Dependency-ordered decomposition** | OpenHands refactor SDK, Carlini's GCC oracle for splitting work | For the eval and team tasks, order items by dependency (leaves first), and for monolithic failures find an oracle that splits them. | Low cost. Addresses the "every agent builds every item" finding (stage 1–2). |
| 10 | **Prompt-cache-aware context** | Manus ([post](https://manus.im/blog/Context-Engineering-for-AI-Agents-Lessons-from-Building-Manus)) | Keep Ground read output stable and append-only so it caches well (`--since` cursors help). Track cache-hit rate in eval runs. | Low cost. Phase 1's cost was mostly reads. |

**Where we're plainly worse:**
- **Throughput.**
- **Scale.** Our largest run is 10 agents on a toy; the field is at hundreds of agents.
- **Real verification of user-facing behaviour.** Nothing is published.
- **Trust in practice.** Shared keys mean signatures don't prove authorship yet.
- **Agent overhead.** Our process requires many tool calls per PR: pr claim, `implements`, four `bk-evidence` calls, reports. That is ceremony a token-metered factory pays for.

---

## 7. Risks and open questions

1. **The thesis may be wrong at the substrate layer.**
   - Independent results (CooperBench, MAST) say multi-agent failures are about specification and coordination behaviour, not storage.
   - Anthropic replaced git for *throughput*, and kept git-like semantics ([Pragmatic Engineer](https://blog.pragmaticengineer.com/the-pulse-anthropic-builds-its-own-source-control-system/)).
   - If Weave's advantage is mostly "evidence-bound lines", it might be delivered as a layer over git, and we'd save a frozen format and a migration.
2. **Reading cost scales with the Ground.**
   - Phase 1's 70k-token contexts came from a Ground of about 1,000 contributions; ours is far larger.
   - The vision imagines thousands of agents. Without discovery and summary views, the Ground becomes the bottleneck that Cursor's lock file was.
3. **Ceremony versus benefit.** The process is careful and auditable but slow. We have not shown it prevents defects that a lighter gate would let through. Nothing ships, so there is no outcome signal.
4. **Correlated reviewers.** Builders, manager and security all run on Opus 5.5. Anthropic's own Frontier Red Team (cited in vision.md) found that low agent variance makes failures systemic. One Gemini reviewer on risky classes is thin diversity.
5. **Trust is still nominal.** Shared keys in v0 mean every "independent evidence" claim rests on honest agents. That is fine today, but it is the core of the value proposition, so per-agent keys are not optional for any external claim.
6. **Small-n evaluation.** Three replicates give p ≥ 0.1, and the ledger has one run per arm. Benchmark contamination means public projects test machinery, not models.
7. **Scale of operation.** A single owner, a single machine for the team server, and launchd. The team server is a single point of failure.
8. **The market is moving fast.** GitHub Agent HQ, Devin Bots, OpenAI Dots and Anthropic Duet are all centralising orchestration. Our edge must be something they won't copy cheaply: a neutral, signed evidence record.

**Open questions to settle:**
- Does the Ground (status views, `needs_recheck`) improve outcomes per token at 20 or more agents on a non-toy project?
- Does cross-model review beat fresh-context same-model review on confirmed-finding precision?
- Can Weave deliver its benefit as a layer over git?

---

## 8. Recommended next steps (ranked)

1. **Run the eval at a scale and difficulty that can separate the arms.** Do this before more Weave build-out.
   - Rung 2–3 project, 3 replicates per arm, 10 or more agents.
   - Arms: git, git + Ground (claims, evidence, `needs_recheck` but no Weave), and Weave.
   - The middle arm is the test of whether the evidence layer alone carries the value.
   - Needs owner approval for the budget.
2. **Instrument reviewer precision and gate cost now**, in `scripts/metrics`:
   - per-model confirmed and rejected findings, with severity;
   - rounds per PR;
   - hours and tokens from PR to merge.
   Cheap, and it decides items 3 and 4.
3. **A time-boxed experiment with a lighter gate for low-risk classes.** Docs, CLI and library classes merge on one independent pass plus the counted run, with "green" as the release guard. Use the existing experiment mechanism. Target: higher throughput with no rise in reverts or follow-up fixes.
4. **Done-contracts in every task brief** (borrowed from Anthropic and Factory), with reviewers verifying against them. This targets the review-overfit finding (d64c3d545e81).
5. **A holdout scenario suite for Bracken's CLI**, run only by the manager. It is the industry's most credible defence against agents gaming tests.
6. **Turn `needs_recheck` on for our own merge gate**, as an experiment. A pr claim goes stale when its task or dependencies are superseded. It is cheap, and it is our most distinctive mechanism, so dogfood it.
7. **Per-agent keys, then an in-toto export of merge evidence.** This makes "independent, signed evidence" true in fact and puts it in a format the industry is standardising. It is the strongest strategic positioning.
8. **Reframe the pitch, internally first.** Lead with Manyroot as the *evidence and acceptance layer for agent factories*: it works over git, worktrees and any harness. Present Weave as an optional code store that has to earn its place through the eval in step 1.

---

*Method note.* Two research passes plus my own spot checks. CooperBench, Anthropic's Scion post (via [The Register](https://www.theregister.com/devops/2026/10/13/anthropic-reinvents-source-control-as-claude-floods-the-world-with-code/5302943) and [Pragmatic Engineer](https://blog.pragmaticengineer.com/the-pulse-anthropic-builds-its-own-source-control-system/)) and Devin Bots were confirmed directly. I read the OpenAI, Cursor, Anthropic and Cognition primary posts, the Claude Code and Copilot docs, and METR's 2026 update in full. Many arXiv numbers (ParallACT, the review-bot studies, AgentRoom) come from abstracts or search snippets and should be checked before anyone quotes them externally. No project or personal names were used in outside queries.
