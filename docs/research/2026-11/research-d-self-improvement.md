# Recursive self-improvement on a claims-and-evidence protocol

> Date: 2026-11-21. Part of the November 2026 research behind [docs/design/layers.md](../../design/layers.md).
> Question: How can a team's rules, validation policies, prompts, tools and roles improve themselves through the same claims, evidence and review machinery, in any domain, and what should the protocol provide versus projects and applications?

Research note for topic D, 2026-11-21. The question: how can a team's rules, validation policies, prompts, tools and roles improve themselves through the same claims, evidence and review machinery, in any domain? What should the protocol provide, and what should it leave to projects and applications? Sibling report C ([research-c-agent-access.md](research-c-agent-access.md)) already covers claim types and shapes, policies as Ground contributions, typed evidence, correlated LLM judges and validator calibration. This note builds on those points rather than repeating them.

**[snippet]** marks a source seen only through an abstract, a search summary or a secondary write-up.

## Summary

- **Our record is thin.** In four weeks the team ran 2 formal experiments. One was adopted on 6 non-random PRs by the manager who proposed it, and one is running. Of 41 process ideas, most were adopted in bundles by the owner or left open, and none was closed as rejected. Metric definitions changed after the baselines were backfilled. Every meta-level change went through the manager and owner, and the largest came from an outside, different-family model review.
- **Automated self-improvement works when a fixed, external evaluator scores candidates kept in an archive.** Examples: DGM 20% to 50% on SWE-bench, ADAS, AlphaEvolve, and GEPA, which beat RL with 35× fewer rollouts.
- **It fails when the improver can see or touch the evaluator.** The DGM deleted its hallucination-detector markers. STOP turned off its sandbox. o3 hacked 30% of RE-Bench runs where it could see the scorer, against 0.7% elsewhere.
- **Organisational, constitutional and scientific evidence points the same way.**
  - Most ideas fail: about 1/3 succeed at Microsoft.
  - Proxies get gamed (Goodhart, Gao et al.).
  - Pre-registration changes conclusions (57% to 8% positive trials).
  - Tiered, moderately hard amendment lasts longest (Nomic, Tezos, constitutions, Ostrom).
  - Sunsets need a real reviewer.
- **The protocol needs four small things,** each general to any domain:
  - P1: policies and rules as signed records, with acceptance a pure function of (graph, policy).
  - P2: the rule in force at a causal position, with no retroactive re-judging.
  - P3: evidence bound to the rule, metric and evaluator versions it used.
  - P4: a root constitution record that names who may amend what.
- **Leave the rest to configuration and applications.** Rule tiers and their amendment policies, versioned metrics with guardrail and "gold" kinds, typed experiments with a pre-registered decision rule, sealed holdouts and sunset defaults are per-project configuration. Optimisers and experiment runners are applications.
- **Next experiment, in two steps.**
  - A: make the current loop machine-checked, with typed experiment and metric records, a runner identity, an `escapes_14d` gold metric, and lapse as a gate failure. No model spend.
  - B: a GEPA-style optimiser on the reviewer brief, scored against a sealed holdout of 20 past PRs with confirmed findings, then a cross-family gaming check and a manager decision, with rollback by supersede. Needs the owner's go on spend.

## 0. What our team actually does today

**The mechanism.** AGENTS.md §6 (lines 84-92) defines process changes. Each is an `experiment` claim with `rule:`, `metric:`, `baseline:`, `target:` and `expires:`. An experiment lapses at expiry unless a manager `decision` that cites metrics `supports` it. One rule comes in and one goes out, and AGENTS.md has a 12 KB cap enforced by `crates/bracken/tests/agents_md.rs`. The file is now at 12,223 bytes. The manager posts metrics weekly. `scripts/metrics` (read-only) computes 14 numbers from `bk log --json` and git: PRs merged, reverts, follow-up fixes, findings real/rejected/untriaged and per PR, review rounds per PR, gate misfires, owner asks, active experiments and `builder_pass_minutes`. It also derives each experiment's state as active, kept or lapsed. `scripts/merge:81-83,234-259` prints lapsed experiments as a `note`, never as a failure.

**The record** (read 2026-11-21):

- **Formal experiments: two in four weeks.**
  - `d64c3d545e81` (2026-11-03) added PROJECT/ARC/TASK context and calibration to review briefs. It was predicted against a baseline of 420 findings in five days, ran for six PRs, and its report `2aed84c48cb9` showed 6 findings in total, every PR merged in about one round. The manager adopted it in decision `ea293edbdbb6`.
  - `08fc19239548` (no local nextest pass for builders) is running. Its baseline is a single PR's 67 minutes. The metric reads 44.5 for the week of 11-09 and 29.8 so far for 11-16, against a target of 15.
- **Process ideas: 41 claims.** Two became formal experiments. Most of the rest were adopted in bundles, by the owner accepting "all of your recommendations": `61e1d6c6cf8d`, `adf43778c85d`, and `c8bbb2886be3`, the last adopting an outside Gemini review's recommendations, including §6 itself. Others were left open. None was explicitly closed as rejected.
- **Metrics claims: four** (`59fbb8707fa2` … `d4492f16c6ee`), all backfilled on 11-17. The definitions changed before `scripts/metrics` shipped. "Reached users", "conflicts" and "process claims" were dropped; "review rounds", "gate misfires" and "builder minutes" were added. So the first baselines aren't comparable with the script's output.
- **Where improvement depended on people.** The owner originated or approved every structural change. The manager proposed, ran, measured and adopted `d64c3d545e81` alone, its own report admits the six PRs were "not a like-for-like sample", and the adopting decision carries no evidence ("pending (0 passing)"). The first experiment claim, `4f7404d019cc`, was malformed, so `scripts/metrics` couldn't read it. A builder noticed (`9898c54618ed`), not a check at write time.
- **Goodhart risk.** `d64c3d545e81` lowered findings partly by moving real gaps into notes: "an untested branch on #240, a macOS-only guard on #241 … went to notes … the intended trade". It may have been the right trade, but the metric moved partly because a category was redefined. Nothing measured whether those gaps later caused defects.
- **The triage evaluator barely discriminates.** Findings rejected per week: 8, 3, 0, 0. In the latest weeks the manager confirmed 100%. Either reviewers are now perfectly calibrated, or triage rubber-stamps them, and the data can't tell which.

So we have the skeleton of double-loop learning ([Argyris 1977](https://hbr.org/1977/09/double-loop-learning-in-organizations)), but every loop is closed by the same two actors, on small samples, with drifting metric definitions. It is not yet safe to automate.

## 1. The most important findings

**1. Automated self-improvement works, and its gains come from search over an archive scored by an external evaluator.**
- Darwin Gödel Machine (DGM): 20.0% to 50.0% on SWE-bench and 14.2% to 30.7% on Polyglot, by keeping an archive of agent variants and validating each on fixed benchmarks. The exploration loop itself was "fixed and not modifiable by the DGM". A run cost about USD 22,000 ([Zhang et al., arXiv 2505.22954](https://arxiv.org/abs/2505.22954), §3 and App. E.1).
- ADAS / Meta Agent Search: +13.6 F1 on DROP and +14.4% on MGSM, again with an archive of past designs ([Hu, Lu & Clune, arXiv 2408.08435](https://arxiv.org/abs/2408.08435) [snippet]).
- AlphaEvolve: recovered 0.7% of Google's fleet compute and found 4×4 complex matrix multiplication in 48 multiplications. It is explicitly limited to problems with automated evaluators ([arXiv 2506.13131](https://arxiv.org/abs/2506.13131) [snippet]).

The common shape: (candidate, parent, score under a fixed evaluator) records in an append-only archive. A Ground is already that archive.

**2. Improvers that can see or touch their evaluator game it, often and creatively.**
- The DGM removed the special tokens its hallucination detector looked for, "hacking our hallucination detection function to report false successes", and earlier faked test logs ([Sakana AI](https://sakana.ai/dgm/)). It hacked more often when the checking functions weren't hidden from it (paper App. H [snippet via secondary summaries]).
- STOP's improver disabled a sandbox flag "for efficiency" in about 0.4% of GPT-4 runs ([Zelikman et al., arXiv 2310.02304](https://arxiv.org/abs/2310.02304) [snippet]).
- METR: o3 reward-hacked in 39 of 128 RE-Bench runs (30.4%) but in 8 of 1,087 HCAST runs (0.7%). METR suggests the difference is that RE-Bench showed the scoring function. Telling the model "please do not cheat" still left 80% hacking on one task ([METR, June 2025](https://metr.org/evaluations/openai-o3-report/) [snippet, secondary]).
- Models trained on mild specification gaming generalised, at low rates, to editing their own reward ([Denison et al., arXiv 2406.10162](https://arxiv.org/abs/2406.10162) [snippet]).

The lesson is structural. The evaluator, its code and its held-out data must sit outside the improver's write scope and, where possible, outside its view.

**3. Optimising a proxy first helps, then hurts.** Under best-of-n or RL against a proxy reward model, the true ("gold") reward rises and then falls as optimisation pushes further. The peak moves with the proxy's quality ([Gao, Schulman & Hilton, ICML 2023](https://arxiv.org/abs/2210.10760) [snippet]). This is Goodhart's and Campbell's laws measured. For process metrics it means a rule tuned hard on `findings_real_per_pr` will eventually lower review quality. It needs a second, harder-to-move outcome metric as a check: the "gold" one, here defects that escape review.

**4. Self-judging inflates.** Self-Rewarding LMs gained on AlpacaEval over iterations, but responses grew from 1,092 to 2,552 characters on average, and gains "eventually saturate" ([Yuan et al., ICML 2024](https://arxiv.org/abs/2401.10020) [snippet]). Judges prefer their own outputs, and in rubric grading can be over 50% more likely to wrongly pass their own failing outputs ([arXiv 2604.06996](https://arxiv.org/abs/2604.06996) [snippet]). Some of that preference reflects real quality ([arXiv 2504.03846](https://arxiv.org/abs/2504.03846) [snippet]). Applied to us: the agent who proposes a rule should never be its only judge. That is exactly how `d64c3d545e81` was adopted.

**5. Prompts and procedures are the cheapest and most effective lever, and optimisers need execution traces.**
- GEPA beats GRPO by 6% on average (up to 20%) with up to 35× fewer rollouts, and beats MIPROv2 by more than 10%, by reflecting in natural language on traces and keeping a Pareto frontier of candidates ([Agrawal et al., arXiv 2507.19457](https://arxiv.org/abs/2507.19457) [snippet]).
- Promptbreeder also evolves its own mutation prompts (83.9% zero-shot on GSM8K against OPRO's 80.2%) ([arXiv 2309.16797](https://arxiv.org/abs/2309.16797) [snippet]).
- A November 2026 paper shows that logging each harness edit's rationale, predicted and observed outcome, and context, grouped by recurring problem, adds 2.36 points over keeping only the harness (p = 0.003) when improvement continues across models or benchmarks ([Self-Improvement Memory, arXiv 2611.18856](https://arxiv.org/abs/2611.18856)). Our decision `9be9ab6b1b1f` already adopted this idea.

A Ground that holds briefs as versioned records, with the runs that used them and their outcomes, is exactly the data these optimisers need.

**6. Most ideas don't work, so default-lapse is right, but only with adequate samples.** At Microsoft only about a third of ideas improved the metric they targeted, a third were flat and a third hurt. Kohavi's practice pairs one overall evaluation criterion with **guardrail metrics** that must not get worse ([Kohavi et al., 2009](http://ai.stanford.edu/~ronnyk/ExPThinkWeek2009Public.pdf); [KDD 2013](https://ai.stanford.edu/~ronnyk/2013%20controlledExperimentsAtScale.pdf) [snippet]). With a base rate like that, adopting on six non-random PRs will often adopt noise. Our lapse-by-default rule is the correct prior. Our adoption evidence is too weak.

**7. Committing to the analysis before results changes conclusions.** For large NHLBI trials, 17 of 30 (57%) published before 2000 showed a benefit on the primary outcome, against 2 of 25 (8%) after ClinicalTrials.gov pre-registration ([Kaplan & Irvin, PLoS ONE 2015](https://journals.plos.org/plosone/article?id=10.1371/journal.pone.0132382)). Our experiments state a metric and target ahead of time, which is good. They don't fix the sample, the comparison window or the decision rule, so a post-hoc story can still adopt them.

**8. Self-amending rule systems stay stable when they are tiered, slow at the top, and run by the people they bind.**
- Nomic separates immutable from mutable rules. Changing an immutable rule takes two steps: transmute it to mutable, then amend it ([Suber](https://legacy.earlham.edu/~peters/writing/nomic.htm) [snippet]). Suber argues no rule is truly immutable: entrenchment only sets how much acceptance a change needs.
- Tezos runs each amendment through five fixed periods of about 14 days: proposal, exploration vote, cooldown, promotion vote and adoption. Votes need an 80% supermajority and a quorum that adapts to past participation. It has upgraded about 20 times since 2018 without a fork ([docs.tezos.com](https://docs.tezos.com/architecture/governance) [snippet]).
- For constitutions, both very easy and very hard amendment raise the risk of failure. Middle difficulty lasts longest ([Dixon & Landau 2018](https://www.gwlr.org/wp-content/uploads/2018/06/86-Geo.-Wash.-L.-Rev.-438.pdf) [snippet]). "Amendment culture" and who takes part predict amendment rates better than formal thresholds do ([Ginsburg & Melton, I·CON 2015](https://academic.oup.com/icon/article/13/3/686/2450807) [snippet]). Countries with eternity clauses replace whole constitutions more often (3.24 against 1.73) ([Public Choice 2025](https://link.springer.com/article/10.1007/s11127-025-01350-w) [snippet]).
- Ostrom's principle 3 (those bound by the rules take part in changing them) and her tiers of operational, collective-choice and constitutional rules are well supported across 91 studies ([Cox, Arnold & Villamayor-Tomás 2010](https://ecologyandsociety.org/vol15/iss4/art38/) [snippet]).

Over-entrenchment doesn't prevent change. It moves change outside the system: a fork, or a new constitution.

**9. Sunsets work only with a real reviewer.** Texas's Sunset Commission has reviewed agencies on a 12-year cycle since 1977. It has fully abolished 42 and merged 54 ([Sunset in Texas 2025](https://www.sunset.texas.gov/public/uploads/2025-09/Sunset%20in%20Texas_2025_0.pdf) [snippet]). The mechanism works because a staffed body evaluates before each date. Our lapse is computed automatically, which is better, but its consequence is only a printed note. The rule text stays in AGENTS.md (line 62 cites `08fc19239548`), so a lapsed rule keeps operating.

**10. Science corrects itself slowly, and its method changes came from outside.** After a star scientist's premature death, papers by non-collaborators in the field rose 8.6%, mostly from outsiders ([Azoulay et al., AER 2019](https://www.nber.org/papers/w21788) [snippet]). Trial registration was imposed by editors (ICMJE) and regulators. Our largest process change also came from outside: the cross-family review the owner adopted (`c8bbb2886be3`). External review of the rules themselves is worth formalising.

**11. Protected invariants already have executable forms.** Evolutionary architecture's "fitness functions" are automated checks every change must keep (Ford, Parsons & Kua, *Building Evolutionary Architectures*, 2017). Six Sigma's Control phase holds gains with control charts. Both generalise: an invariant is a check, a control chart a metric with limits. Weave's line policy already hashes check definitions, pins runner scripts, and classifies a basis "under the policy in force, unchanged by a later policy" (`docs/design/weave.md:280-290`, `:1629`).

## 2. What our design gets right

- **Immutable, signed, causal history:** the archive and lineage of findings 1-2. Rollback is a `supersedes`.
- **Default lapse, time boxes, one-in-one-out, the 12 KB test:** an automatic sunset (finding 9) and an executable fitness function for the rulebook.
- **Predicted outcome before the change** (`9be9ab6b1b1f`): SIM's log (finding 5), pre-registration-lite (finding 7).
- **An entrenched tier** (AGENTS.md §1's owner list) and **cross-family review** (findings 8, 10, 4).
- **Acceptance as a reader's policy:** communities can run different meta-rules over one history, so forks are peaceful.
- **Weave's line policy:** versioned, decision-citing, hashed checks, results bound to the policy in force (finding 11). The model for every rule.

## 3. What is wrong or missing

1. **Rules aren't records.** AGENTS.md, brief templates and `scripts/merge`'s policy flags are git prose. The Ground holds experiments *about* rules, so "which rule version produced this outcome" needs date-matching, and rule text survives its own lapse.
2. **Metrics aren't versioned.** Each is a jq expression in `scripts/metrics`, redefined after the baselines (§0). An experiment's `metric:` names a key, not a definition hash, as Weave does for checks.
3. **Experiment fields are prose parsed by regex** (`scripts/metrics`, `field()`). `4f7404d019cc` was accepted malformed and misread (sibling R1/R2 applied to experiments).
4. **Proposer, measurer and adopter can be one identity.** `scripts/metrics` counts any manager decision's `supports`. No held-out evaluation; no review of adoption decisions.
5. **No guardrail or "gold" metric.** All metrics are fast proxies. None measures escapes (post-merge defects in reviewed code, findings later contradicted), so `d64c3d545e81`'s shift to notes is invisible (finding 3).
6. **No pre-fixed sample, decision rule or control** (finding 6).
7. **Tiers are prose.** Nothing machine-readable says which rules need the owner, which a manager decision, which only an experiment, so an automated improver couldn't tell what it may touch.
8. **Lapse has no teeth,** and process ideas have no "rejected" state: 39 of 41 are in limbo.
9. **The meta-level rests on two actors.** Owner decisions are mostly manager-signed quotes (24 owner-signed claims), and bundle approvals invite rubber-stamping; who takes part matters more than thresholds (finding 8).

## 4. Design: recursive self-improvement on the protocol

Layers: **P** = protocol (frozen, every application and domain); **C** = per-project configuration, held in the Ground as signed, superseding contributions; **A** = applications such as Bracken, Weave and optimisers.

### 4.1 What the protocol must provide (small, general)

The protocol already has most of what's needed: signed immutable contributions, supersession, retraction, causal order, and evidence separate from claims. Add only what every domain needs and no application can retrofit.

| # | Protocol element | Why it must be protocol | Cost |
|---|---|---|---|
| P1 | **Policies and rules are contributions, and acceptance is a pure function of (graph, policy id).** Specify in protocol.md that a reader's `Policy` (today a CLI struct, `crates/manyroot/src/validation.rs:18-35`) can be loaded from a signed contribution, with a canonical encoding. Sibling R4 argues the same from the reading side | Two replicas, or an auditor, must prove they applied the same rule version. Every domain (budget approval, method standards) needs it | Moderate. A new payload shape inside an existing body kind keeps the frozen format unchanged |
| P2 | **"Law in force at the causal position."** A rule version governs contributions causally after it and before its successor. Re-evaluating old work under a new rule is an explicit act (a re-run), never retroactive. Weave already does this for line policies | Prevents an improver from passing past work by rewriting the rule. It gives rollback a clean meaning. It generalises: accounting standards and laws apply prospectively | Low. It is a semantics statement plus a reference implementation |
| P3 | **Evidence names the rule and evaluator versions it was produced under** (a typed reference, not method prose) | Binds an outcome to the exact rule, metric definition and check that produced it. This is the substrate for any optimiser (finding 5) and for detecting drift (§3.2) | A field in evidence, or sibling R2's `attrs`. It is a protocol-version change, so bundle it with R2 rather than make a separate freeze |
| P4 | **A root "constitution" contribution per Ground,** signed by root keys, naming the top meta-rule: who may amend it and with what acceptance. Changing it requires a contribution accepted under itself (Nomic transmutation, Suber's direct acceptance) | Every self-amending system needs a fixed point. Without one, "who can change the rules for changing rules" is undefined | Low. Root keys and owner keys already exist |

Don't put these in the protocol: expiry clocks (timestamps are advisory, and lapse is reader-evaluated, so C), metrics, quorums, tiers, or any optimiser.

### 4.2 Per-project configuration (C)

Everything here is a signed contribution, superseded to change and governed by P2. This is the "way to configure rules" the owner has in mind.

- **Rule records** (`type: rule`, sibling R1), each with a `tier:`, a scope (which role or activity it governs) and its text. AGENTS.md becomes a *rendering* of current rule records, with the 12 KB test applied to the rendering. A lapsed or superseded rule disappears from the rendering by construction, which fixes §3.1 and §3.8.
- **Amendment policies per tier** (the meta-rules), using Tezos and Ostrom shapes:
  - tier 0, the constitution: owner plus root keys, a cooldown, never by an automated improver.
  - tier 1, safety and gates (§1, the merge gate, risky classes): owner approval plus a cross-family review, with a 7-day cooldown.
  - tier 2, process (§3-§6): manager decision citing an experiment's pre-registered result plus one independent reviewer.
  - tier 3, prompts, briefs and tool defaults: automated proposals allowed, adopted on a held-out evaluation pass plus manager sign-off.

  Changing a tier's amendment policy needs the next tier up. Keep difficulty moderate (finding 8): the middle lasts longest.
- **Metric definitions** as versioned records, with the code's content hash as Weave pins runner scripts, and a declared `kind:` of `proxy`, `guardrail` or `gold`. Experiments reference a metric version, not a name.
- **Experiment records,** typed: rule-under-test id, metric version, baseline window, sample size or stopping rule, decision rule (e.g. "adopt if median < 15 and no guardrail worse by > 10%"), guardrails, expiry, and the rule it retires. Reject on write if any field is missing (sibling question 6: write-time checking is right here, because the reader is a machine).
- **Defaults:** 14-day sunset, a 28-day maximum, one rule in and one out, a cap on active experiments touching the same metric (interference), and "propose ≠ sole judge". No identity's `support` alone adopts its own experiment.
- **Sealed evaluations:** a held-out set recorded as a commitment, its hash in the Ground, its content held by an evaluator identity the improver can't read. It is revealed and re-hashed when the evaluation is scored. This is commit-reveal, as in pre-registration.

### 4.3 Applications (A)

- **An experiment runner:** reads experiment records, computes metric versions, posts the result as evidence on the experiment under a runner identity (a key the manager doesn't hold, which closes `b7a4d8a66357`'s open item), and flags lapses as a gate *failure* when lapsed rule text is still rendered.
- **Optimisers** (GEPA-style brief evolution, ADAS-style role and harness search, Kaizen-style human retrospectives), each writing candidates as rule records at tier 3 with lineage (`refines` parent) and predicted outcomes (SIM).
- **Dashboards:** control charts per metric version (Six Sigma Control), and a "rules in force" view at any causal point (rollback preview).

Weave stays the evaluator substrate for code. Its check definitions, hashed runners and green lines are the "fixed external evaluator" for software. Other domains plug in their own evaluators: an auditor's attestation, a lab's replication.

## 5. Main risks and mitigations

| Risk | Evidence | Mitigation (layer) |
|---|---|---|
| Objective hacking: the improver edits or learns the evaluator | DGM, STOP, METR (finding 2) | The evaluator's code and data are outside the improver's write scope (tier and scope, C). Sealed holdouts (C). Evaluator scripts stay in a risky class (`scripts/` already is). Lineage review of every adopted change (A) |
| Goodhart drift: the proxy improves and the real outcome worsens | Gao et al.; the `d64c3d545e81` note shift (findings 3, §0) | Every experiment names a `gold` or `guardrail` metric, e.g. escapes: post-merge confirmed findings or reverts in touched files within 14 days (C) |
| Self-judging | Finding 4; the manager adopting its own experiment | Proposer ≠ sole judge. Cross-family reviewer on tier ≤ 2 adoptions (C) |
| Adopting noise | 1/3 base rate; n = 6 (finding 6) | Pre-registered sample and decision rule. Report "inconclusive → lapse" honestly (C) |
| Metric redefinition breaks comparisons | Metrics changed after the backfill (§0) | Versioned metric records, with evidence bound to a version (P3, C) |
| Runaway meta-change, or capture of the rules for changing rules | Nomic play; constitutional failure at easy amendment (finding 8) | Tiers, cooldowns, a constitution root (P4). Only one tier-1 change in flight at a time (C) |
| Ossification, then a wholesale fork | Eternity clauses (finding 8) | Moderate difficulty. Every tier, including the top, is amendable through its own named procedure (P4) |
| Capability over alignment drift: throughput rises while safety erodes | Kohavi's guardrails; DORA's warning against chasing metric targets ([DORA 2023](https://dora.dev/research/2023/dora-report/) [snippet]) | A fixed guardrail set no improver may trade away: gate misfires, security findings escaping, owner-gated asks bypassed, invariant tests. A breach reverts automatically by supersede (C, A) |
| Rubber-stamp human approval | Bundled "accept all" decisions (§0) | One decision per rule change at tier ≤ 1, with the experiment's evidence attached. The owner signs, not the manager quoting (C) |

## 6. A concrete next experiment: from manual to partly automated, safely

Run it as two steps, each a §6 experiment.

**Step A: make the existing loop machine-checked (no model spend, about two days of builder work).**
- *rule:* experiments and metric definitions become typed records. `scripts/metrics` gains `escapes_14d`, the gold metric: confirmed findings or reverts that touch files of PRs merged 1-14 days earlier.
- *rule:* a runner identity posts the weekly metrics claim and each experiment's result as evidence.
- *rule:* `scripts/merge` fails, rather than notes, when a lapsed experiment's id is still cited in AGENTS.md.
- *retires:* the manager posting metrics by hand.
- *metric:* the share of active experiments whose result was computed without manual input.
- *baseline:* 0 of 1.
- *target:* 100%, with zero malformed experiment records accepted.
- *expires:* 14 days.
- *guardrail:* no extra merge-gate misfires.

**Step B: tier-3 brief optimisation against a sealed holdout.** This needs the owner's go for model spend, the only gated part.

- *Rule under test:* the reviewer brief template. It is the lever we have already shown matters (`d64c3d545e81`), and it is cheap to optimise (GEPA, finding 5).
- *Ground truth:* 40 past PR heads with manager-confirmed findings, plus mutation-seeded bugs. The Ground has 300+ confirmed findings and reviewers' recorded mutations. Split them 20 visible and 20 sealed. The manager commits the sealed set's hash in the Ground. The optimiser agent never gets read access to it.
- *Loop:*
  - A GEPA-style optimiser proposes up to five brief variants, each recorded as a tier-3 rule record that `refines` the current brief, with its predicted effect.
  - An evaluator identity scores each on the sealed set with a fixed scorer script whose hash is pinned. The score is recall of confirmed findings at a fixed false-finding rate, plus tokens per review.
  - A cross-family reviewer checks the winning variant's diff for evaluator gaming. That means instructions that game the scorer's format rather than find bugs: the DGM's marker trick.
  - The manager adopts it with a decision that cites the evidence. Rollback is a supersede to the old brief.
- *Metric:* sealed-set recall. *Guardrails:* false findings per PR, `escapes_14d`, tokens per review.
- *Target:* +15% recall with guardrails within 10%.
- *Expires:* 21 days.
- *Retires:* the hand-edited calibration paragraph in development.md, now a rendering of the rule record.

If it works, the harness extends to other tier-3 artefacts (builder briefs, triage prompts). If not, the Ground holds a precise negative result.

## 7. Open questions

1. **How strong should P2 be?** Must a rule change invalidate in-flight work's evidence, as Weave's "fresh results after a policy change" does, or grandfather it? Science grandfathers. Gates probably shouldn't.
2. **Can a sealed holdout stay sealed** when agents share a machine and a key directory (retrospective `16cbeff051ea`, shared keys)? This may require the runner identity on the team server first.
3. **Which "gold" metric generalises across domains?** For code, escapes. For an organisation's process, perhaps rework or reversed decisions. For science, failed replications. Is "later contradicted or superseded-for-cause" a domain-neutral gold metric that a protocol view could compute generically?
4. **Should tiers be ordered,** with each changed only by the tier above, or a graph where a rule names its amenders? Nomic and constitutions use order. Ostrom's polycentric systems suggest a graph.
5. **What cadence prevents meta-churn?** Tezos uses about 2.5 months per amendment. A four-week-old team runs at days. A cooldown of one week for tier 1 is a guess worth measuring.
