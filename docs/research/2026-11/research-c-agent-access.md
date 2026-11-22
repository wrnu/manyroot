# Agent and human access to a shared knowledge graph: reading, writing and evaluating it cheaply

> Date: 2026-11-21. Part of the November 2026 research behind [docs/design/layers.md](../../design/layers.md).
> Question: How can agents and people search, read, write and evaluate a shared, signed, append-only knowledge graph cheaply in tokens, latency and correctness, and how should evaluation and acceptance be represented so they are computed and explained cheaply?

Research note for topic C, 2026-11-21, expanded after the owner lifted the length limit. Scope: how agents and people search, read, write and evaluate a shared, signed, append-only knowledge graph efficiently in tokens, latency and correctness; what structure makes that efficient; and how evaluation and acceptance can be represented so it is computed and explained cheaply. Judged against the owner's addenda: Manyroot is for information structures of any kind (software, financial design systems, organisations, business processes), and its founding model is the scientific method and peer review.

Sources marked **[snippet]** were seen only through an abstract, search summary or secondary write-up; **[memory]** marks well-known sources I cite from prior knowledge without re-reading them for this note; the rest were read at the source. Token counts are estimated at 4 characters per token. All Ground measurements were taken read-only through `scripts/bk` on 2026-11-21.

## Summary

1. **Reading is the cost, and length is also an accuracy problem.** An agent's start-of-task reads (AGENTS.md section 3) are about 12k tokens in compact form, which cuts every statement at 120 characters, or about 48k tokens in full. Studies show accuracy falling with context length even when retrieval is perfect (13.9% to 85% losses), worst with near-duplicate distractors, which is what a Ground full of related decisions and findings is.
2. **Our typed structure is an unused advantage.** Memory systems that pre-filter beat full context (Zep: 71.2% against 60.2% on LongMemEval, with 1.6k context tokens instead of 115k), but most pay hundreds of thousands of tokens to have an LLM *extract* structure. Our authors write it, so deterministic views are enough.
3. **But the structure stops one level too early.** Kinds are typed; claim *types* (decision, task, finding, report) and attributes (dates, severity, sha, owner) are free-text tags and prose. On this Ground, 148 of 164 current decisions carry no tag but `decision`, 150 put their date in the text, and 340 of 869 findings have no parseable severity. As a result `status` puts 24-day-old reports at the top of the review queue: reports and code share the `implements` relation, and the queue has 247 waiting entries.
4. **Recommended read interface:** one-call, role-aware views (`brief`, `queue`, `status --type`) with a token budget, three nested detail levels (`ids`, `summary`, `full`), deterministic order and absolute times so output is a stable prefix that caches. Each derived status carries a `because` listing the ids that made it hold. A builder's brief drops from ~12k–48k tokens to an estimated 1.5k–4k (section 4.1 has worked examples).
5. **Recommended write interface:** typed claims checked against per-project *shapes* (SHACL-style), `key:value` attributes (Wikidata-style qualifiers), and one-call writes that create a subject with its relations. Agents get named, parameterised views instead of a query language: text-to-query still fails 40% to 80% of the time on realistic graphs.
6. **Evidence needs more than pass/fail.** It needs a method *class* (executed, measurement, review, replication, attestation, confirmation), an *independence key* (model family, operator, organisation, runner) and an optional ordinal *strength*. Nine LLM judges from seven families gave about two effective votes, so counting distinct authors, as `min_passes` does now, overstates independence. Section 4.3 sketches the vocabulary, borrowing from audit standards (ISA 500) and eLife's assessments as well as from code review.
7. **Acceptance semantics.** Keep the core rule. Make policies first-class Ground contributions recorded before the evidence they judge, which causal parents can prove, as Registered Reports do. Add two optional, polynomial-time terms: grounded argumentation over `contradicts` (only admissible attackers count) and a truth-maintenance recheck that names which evidence classes must re-pass. Section 4.4 computes a worked example by hand.
8. **The science mapping fits, with gaps.** Supersede is correction, retract is retraction, a finding is an expression of concern, a policy recorded first is a pre-registration, evidence is review and replication, a digest is meta-analysis, and `needs_recheck` does structurally what citation networks fail at: only 5.4% of post-retraction citations acknowledge the retraction. The gaps are graded strength, independence, reviewer calibration, and the severity of a test relative to the claim.
9. **Where things belong.** Protocol: keep it minimal. Re-frame Acceptance as a reference policy; consider a generic `attrs` map only at a future version. Per-project layer: types, shapes, evidence vocabulary, independence keys, policies and argumentation and recheck terms, all as signed, superseding Ground contributions that readers select by id. Application: views, cursors, caches, indexes, explanations, calibration metrics and MCP.
10. **Humans and other domains get the same structures.** A finance team's fact with entity, period and unit, an organisation's resolution with a quorum, and a process step with a conformance check all fit types plus attributes plus evidence classes plus policies. The web view renders the same `because` and assessment data that agents read.

## 0. What our agents get today (this repo's Ground: 13,981 contributions)

### 0.1 Sizes and latency

| Read | Output | ≈ tokens |
|---|---|---|
| Start-of-task reads (`log --tag decision/gotcha/preference/reference --current`), compact | 164 + 116 + 11 + 10 items, 47.8 KB | ~12k |
| The same, statements in full (114 + 59 + 4.2 + 4.1 KB of statement text, plus headers) | ~191 KB | ~48k |
| The same, first sentence only (measured: 11.4 + 5.1 + 0.5 + 0.4 KB) | ~17 KB | ~4.3k |
| `show <task>` for a current builder task | 5.5 KB | ~1.4k |
| `status --as builder-eval-backend` | 6.7 KB | ~1.7k |
| `status` (default, 40 lines) | 7.9 KB | ~2k |
| One day of `log --compact` (2026-11-20 onward) | 251 lines, 31 KB | ~7.8k |
| `status --json` | 1.77 MB | ~440k |
| `log --json` (everything) | 19.8 MB | ~5M |

Every read loads the 16 MB verified snapshot and rebuilds the graph and its derived state; a tag-filtered read takes about 0.65 s wall time. Contributions are 45% evidence (6,352), 27% relations (3,719), 26% claims (3,680), 2% retracts and 3 artifacts.

### 0.2 What the structure does and doesn't let a reader filter on

- **Types are tags, and topics mostly aren't tagged.** 148 of the 164 current decisions carry only the tag `decision`. The rest use `models` (4), `forge` (2), `roadmap` (2) and single others. A builder therefore can't ask for "decisions about the eval harness" or "decisions that bind builders". It must read all 164.
- **Time is in prose.** 150 of the 164 decisions carry a date in their first 80 characters, and 69 begin "Owner, YYYY-MM-DD:". The protocol's `timestamp_ms` is advisory system time, so the real date of a decision exists only as text.
- **Findings' severity is unstructured.** Of 869 findings, 315 say "low", 140 "medium", 26 "high", 31 use upper-case variants, 4 say "blocking", and 340 have no severity a pattern can find.
- **Evidence methods mix kinds in one string.** Among the latest 400 evidence records (367 pass, 33 fail): clippy (92), fmt (45), "security review" (36), "manager triage" (30), test-table runs (64), "independent review" (29), nextest (33), doc tests (22), "merge gate" (17). Executed checks, judgements by models, a manager's triage and a gate's own record are all one `method: text` field, distinguished only by convention.
- **The review queue mixes subjects that need review with records that never will.** `status --json` has 478 queue entries: 247 `waiting`, 231 `accepted`. The default view sorts waiting work oldest first (`crates/bracken/src/main.rs:297`), so its first screen is manager and builder reports from 24 days ago, each `implements` a task. `show` on a decision says `pending (0 passing, 0 failing)`, which is meaningless for a claim whose authority comes from who made it.
- **Current isn't the same as open.** 249 task claims are current: finished tasks stay current because nothing supersedes them. "Open" is a further derivation (nothing current implements it).

These are the Phase 1 costs (phase1-bracken-lessons.md, section c) in a different form. The cursor, compact and short-id work fixed the unbounded reads. What remains is that the reader can't say *which kind* of thing it wants before reading it.

## 1. The most important findings

### Finding 1. Context length itself lowers accuracy, so bounded reads are a correctness issue, not only a cost issue

- **Chroma's "context rot" study** (18 models, including Claude 4, GPT-4.1, Gemini 2.5, Qwen3) found performance falling at every one of eight input lengths, not only near the limit. Semantically similar but irrelevant text hurt more than length alone, and there was little position effect in its main experiments ([Hong, Troynikov & Huber, 2025](https://www.trychroma.com/research/context-rot) [snippet]).
- **NoLiMa** removed literal overlap between question and answer. At 32K tokens, 11 of 13 models claiming 128K+ windows fell below half their short-context score. GPT-4o fell from 99.3% to 69.7%, and Llama 3.1 70B's effective length (≥85% of base) was about 2K tokens. With literal overlap restored, scores stayed near perfect: models find exact strings, not meanings ([Modarressi et al., ICML 2025](https://arxiv.org/abs/2502.05167) [snippet]).
- **Length alone, with perfect retrieval.** Even after distractors were replaced by whitespace or masked, performance fell 13.9% to 85% as input grew within the claimed window. Llama-3.1-8B lost 24.2% on MMLU at 30K tokens while retrieving 97% of what it needed ([Du et al., EMNLP Findings 2025](https://arxiv.org/abs/2510.05381) [snippet]).
- **Monitoring degrades too.** LLMs used as monitors lose recall as transcripts lengthen; incremental monitoring and reminders help ([arXiv 2605.12366](https://arxiv.org/pdf/2605.12366) [snippet]). Our reviewers and the manager read long transcripts and long Ground listings.

**For us:** A Ground's typical read is the worst case these studies describe: many statements on the same subsystem, worded alike, with the one that matters (the current decision, the open finding) somewhere in the middle. The 70k-token contexts of Phase 1's Bracken agents, and today's 12k–48k-token task start, cost accuracy as well as money. The fix is not a bigger window but a reader that asks for less: typed filters first, then summaries, then full text on demand.

### Finding 2. Pre-filtered, structured memory beats full context, but only when the structure is cheap to build

- **Zep/Graphiti on LongMemEval** (read at the source): 71.2% against 60.2% for full context with GPT-4o, and 63.8% against 55.4% with GPT-4o-mini. Average context fell from 115k tokens to 1.6k, and latency from 28.9 s to 2.58 s. It got worse on one category (single-session-assistant questions, −17.7% with GPT-4o), which shows a summarising layer losing detail the raw text had ([Rasmussen et al., arXiv 2501.13956](https://arxiv.org/html/2501.13956v1), Table 2).
- **LongMemEval itself.** Long-context models lose 30% to 60% across sustained interactions. The authors frame memory as indexing, retrieval and reading, and the fixes that helped were structural: session decomposition, fact-augmented keys and time-aware query expansion ([Wu et al., ICLR 2025](https://arxiv.org/abs/2410.10813) [snippet]).
- **The cost of building structure from text.** Mem0 reports 7k tokens to build its memory per LoCoMo conversation, 14k for its graph variant, and over 600k for Zep, which writes summaries at every node. Mem0 answered at 1.44 s against 17.12 s for full context ([Chhikara et al., arXiv 2504.19413](https://arxiv.org/abs/2504.19413) [snippet]). A-MEM's linked "notes" cut retrieval to about 1.2k–2.5k tokens against ~16.9k, but every insertion costs several LLM calls ([Xu et al., arXiv 2502.12110](https://arxiv.org/abs/2502.12110) [snippet]; critique in [SAGE, 2605.30711](https://arxiv.org/pdf/2605.30711) [snippet]).
- **Graph RAG spans two orders of magnitude.** Per-query cost runs from ~1k tokens (HippoRAG 2) through ~100k (LightRAG) to ~331k (Microsoft GraphRAG, global mode); indexing from 178k to 655k tokens on the same corpus ([GraphRAG-Bench, arXiv 2506.05690](https://arxiv.org/html/2506.05690v3) [snippet]). LLM-free index construction (LinearRAG, LiteSemRAG) is the recent trend ([2510.10114](https://arxiv.org/pdf/2510.10114) [snippet]).

**For us:** Nearly all of these systems' cost is LLM *extraction*: turning prose into entities, edges and validity intervals. Manyroot's authors write typed contributions directly: kind, relations, parents, author, tags. We can have the accuracy benefit of a structured store at the price of a database query, *if* the structure covers what readers filter on (finding 4, section 0.2). LLM-written summaries remain useful, but as authored, sourced contributions (digests, recommendation R9), not as a hidden index.

### Finding 3. Supersession with history is the right model; real systems also separate "true when" from "recorded when", and keep a ranked "truthy" projection

- **Graphiti's bi-temporal edges.** Every fact carries `t_valid`/`t_invalid` (when it held in the world) and `t'created`/`t'expired` (when the system learned and retired it). A contradicting fact sets the old edge's `t_invalid` instead of deleting it, so history stays queryable ([Zep §2.2](https://arxiv.org/html/2501.13956v1)).
- **Wikidata's ranked statements.** Every statement has a rank (preferred, normal, deprecated), qualifiers (point in time, start and end time, applies-to-part, and so on) and references. The query service publishes a "truthy" projection, `wdt:`, holding only the best non-deprecated rank per property. Simple queries never see deprecated or outranked values, while the full statement nodes (`p:`/`ps:`/`pq:`/`prov:wasDerivedFrom`) stay available ([Wikibase RDF format](https://www.mediawiki.org/wiki/Wikibase/Indexing/RDF_Dump_Format) [snippet]; [Help:Ranking](https://www.wikidata.org/wiki/Help:Ranking) [snippet]). Deprecated means "known wrong, kept on record", not deleted.
- **Event sourcing and ledgers** take the same stance: corrections are new entries (a reversing journal entry, a compensating event), never edits [memory].

**For us:** `--current` (live and not superseded) is our truthy projection, and its being deterministic on every replica is a strength. Two things are missing. (a) **World time.** "Owner, 2026-10-26:" is valid time written as prose, while `timestamp_ms` is transaction time and advisory. A financial fact for Q3, a policy effective from a date, or a role held from January to June all need valid time as data. (b) **Rank short of retraction.** A contribution is either current, superseded or retracted. There is no "kept but deprecated by consensus" for a claim whose author won't retract it. In our model that is an acceptance status ("rejected under policy P"), which is the right place for it, but readers need it in the projection: `--current --accepted-under P`.

### Finding 4. Typing claims by role prevents agents confusing sources; we type the kind but not the role

- **MemIR** names the failure *provenance-role collapse*: in flat memories, agents treat retrieval cues, quoted claims and their own past guesses as established facts. Its typed intermediate representation separates raw evidence, retrieval cues and truth-bearing claims, and only *supported* claims may serve as facts. It improves LoCoMo and BEAM results, most on source tracking and temporal grounding ([Jin et al., arXiv 2605.25869](https://arxiv.org/abs/2605.25869) [snippet]). Follow-ups make the same point for authority ("stored is not supported", [2609.02127](https://arxiv.org/html/2609.02127v1) [snippet]; "when memory becomes authority", [2608.01679](https://arxiv.org/html/2608.01679) [snippet]).
- **Scientific knowledge models agree.** Micropublications separate a claim, the argument for it, and the evidence (data, methods, materials) that `supports` or `challenges` it ([Clark, Ciccarese & Goble, J. Biomed. Semantics 2014](https://jbiomedsem.biomedcentral.com/articles/10.1186/2041-1480-5-28) [snippet]). SEPIO separates the *assertion*, the *evidence lines* (arguments) and the *evidence items* (information) behind it, and notes that a publication is only a proxy for evidence, not evidence itself ([SEPIO overview](https://github.com/monarch-initiative/SEPIO-ontology/wiki/SEPIO-Overview) [snippet]; [Brush et al., ICBO 2016](https://ceur-ws.org/Vol-1747/IT605_ICBO2016.pdf) [snippet]). Nanopublications split assertion, provenance and publication info into separate graphs ([Kuhn et al., PeerJ CS 2016](https://peerj.com/articles/cs-78/) [snippet]).

**For us:** Our body kinds already make MemIR's and SEPIO's top-level split: claims and artifacts are assertions, evidence is evaluation, relations are arguments. That is a real advantage over flat agent memories. Below the kind there is no role. A *decision* (authoritative because of who made it), a *finding* (an objection awaiting triage), a *report* (a record), a *spec* (a requirement to be met) and a *note* (a lead) are all `claim`, differing only by a free-text tag. The consequences are in section 0.2: a queue that can't tell what needs review, a `pending` label on decisions, and AGENTS.md telling agents in prose that "a `note` is a lead". That is the role distinction pushed into every agent's reading of text.

### Finding 5. Agents should get typed tools and named views, not a query language

- **Text-to-query accuracy on realistic graphs.** On CypherBench (11 property graphs from Wikidata, 7.8M entities), the best model reached 60.2% execution accuracy, and no model under 10B parameters passed 20%. The authors convert RDF to property graphs because full SPARQL schemas were harder still ([Feng et al., ACL 2025](https://aclanthology.org/2025.acl-long.438.pdf) [snippet]). The best Spider 2.0 agent solved 21.3% of enterprise SQL workflows, against 91.2% on Spider 1.0 ([Lei et al., ICLR 2025](https://arxiv.org/abs/2411.07763) [snippet]). SPARQL is hard partly because ids must be resolved exactly, the same failure as Phase 1's id problem ([Instruct-to-SPARQL](https://dl.acm.org/doi/10.1145/3698204.3716476) [snippet]).
- **Tool-design guidance.** Anthropic recommends tools that consolidate a workflow, a `response_format` of `concise` or `detailed` (72 against 206 tokens in their Slack example, with ids dropped from concise output), meaningful names or small ids instead of UUIDs ("significantly improves … precision in retrieval tasks by reducing hallucinations"), and truncation with instructions on how to narrow ([Anthropic, 2025](https://www.anthropic.com/engineering/writing-tools-for-agents)).
- **Phase 1** showed both effects: 134 fetches of the spec list to recover full ids, 25 calls refused for short ids, and 23% of `log` calls too large to return (phase1-bracken-lessons.md).

**For us:** "CLI first, MCP later as a thin layer" (agents.md) is right. The interface agents see should be a few *named, parameterised views* whose results are already shaped for the next action: brief, queue, status by type, show with a detail level. Behind them a query engine is worth having (Datalog fits, section 4.6), and people and tools can use it directly; agents should not have to write queries to do routine work.

### Finding 6. Prompt caching rewards append-only, deterministically serialised output

- **Manus** calls KV-cache hit rate "the single most important metric for a production-stage AI agent". Its agents read about 100 input tokens per output token, and cached input costs about a tenth of uncached. Any byte that changes in a prefix (a timestamp at the top of a prompt, non-deterministic JSON key order) invalidates the cache from that point on. Hence: stable prefixes, append-only context, deterministic serialisation, tools masked rather than removed, and the file system as restorable memory, where content can be dropped from context because its path or URL stays ([Ji, 2025](https://manus.im/blog/Context-Engineering-for-AI-Agents-Lessons-from-Building-Manus) [snippet]).
- **Phase 1's Bracken model calls** read 15,100 cached tokens on average, against 6,100 (CRDT) and 5,700 (Git) (phase1-results.md). Nearly all input arrived as cache reads and writes, so the size of what is re-read, and whether it stays byte-identical, set the bill.

**For us:** `log --since <cursor>` is the right shape: it appends. Several current outputs aren't stable. Relative ages ("24d ago") change between reads, age-ordered sorts reshuffle when anything arrives, and the 40-line cap moves its cut-off. A view whose output for the same heads is always the same bytes, ordered by causal position, with absolute times, lets a harness cache it and lets the application memoise it (finding 12, R10). Contribution ids are content hashes, which makes "same heads, same bytes" cheap to guarantee.

### Finding 7. Counting distinct validators overstates independence when the validators are LLMs

- **Nine judges, two votes.** Nine frontier LLMs from seven families, judging NLI items with 100 human labels each, carried the information of about two independent votes (Kish effective sample size). Dawid-Skene and accuracy-weighted voting recovered at most 11% of the gap to an independent panel, even with oracle labels, and Dawid-Skene underperformed majority vote on MNLI ([Kohli, arXiv 2605.29800](https://arxiv.org/abs/2605.29800) [snippet]).
- **Judge errors correlate at about 0.71 to 0.78,** and the paper argues for a "correlated error floor" that adding judges can't remove ([JudgeMoE, arXiv 2610.07109](https://arxiv.org/abs/2610.07109) [snippet]). Modelling shared confounders such as verbosity and style recovers some of it (CARE, [2603.00039](https://arxiv.org/html/2603.00039v1) [snippet]), and IRT-style aggregators do better on small sets ([AGORA, 2611.05556](https://arxiv.org/abs/2611.05556) [snippet]).
- **The truth-discovery literature knew this.** Classic models (the Latent Truth Model, [Zhao et al., PVLDB 2012](http://vldb.org/pvldb/vol5/p550_bozhao_vldb2012.pdf) [snippet]; [Li et al.'s survey, SIGKDD Explorations 2016](https://arxiv.org/abs/1505.02463) [snippet]) estimate source reliability from agreement, and *assume sources are independent*. Dong, Berti-Équille and Srivastava ([PVLDB 2009](https://doi.org/10.14778/1687627.1687690) [memory]) showed that copying between sources must be detected and discounted, or the copied answer wins.

**For us:** The reference policy's `min_passes` counts distinct *authors* (protocol.md, Acceptance). Our gate already distrusts that in practice: it requires named validators with different roles, and decision `b1b56d75221b` puts the independent reviewer on a different model family from security. That knowledge lives in the merge script and AGENTS.md, not in anything a reader of the Ground can evaluate. Two passes from two identities on the same model, prompt and harness are, by this evidence, close to one.

### Finding 8. Human peer review is noisy too; the field responded with graded, multi-axis, public assessments rather than a single accept bit

- **Reproducibility of decisions.** In the NeurIPS 2021 experiment, 882 papers were reviewed by two independent committees. They disagreed on 23%, and 50.6% of the papers one committee accepted, the other rejected. The figure was the same as 2014's 49.5%, although submissions had grown more than fivefold. Restricting to orals and spotlights made it worse, 88.8% ([Beygelzimer et al., arXiv 2306.03262](https://arxiv.org/abs/2306.03262) [snippet]; [NeurIPS blog](https://blog.neurips.cc/2021/12/08/the-neurips-2021-consistency-experiment/) [snippet]).
- **eLife's reviewed-preprint model** dropped accept/reject. Each paper carries public reviews and an eLife Assessment on two fixed ordinal scales. Significance: useful, valuable, important, fundamental, landmark. Strength of evidence: inadequate, incomplete, solid, convincing, compelling, exceptional ([eLife](https://elifesciences.org/about/elife-assessments) [snippet]). Across revisions, 76% of significance terms stayed the same while evidence terms moved more: authors can strengthen evidence, not importance ([eLife analysis](https://elifesciences.org/inside-elife/e9d530fc/the-elife-model-comparing-elife-assessments-across-revisions) [snippet]).
- **Words need definitions.** A preregistered study found readers interpreted eLife's terms inconsistently, and an alternative vocabulary read more consistently ([PLOS Biology 2024](https://www.ncbi.nlm.nih.gov/pmc/articles/PMC11340897/) [snippet]).
- **Failure modes.** Undisclosed reviewer collusion ("collusion rings") in computer-science conferences ([Littman, CACM 2021](https://doi.org/10.1145/3429776) [memory]). An estimated 6.5% to 16.9% of review text at 2023–24 ML conferences was substantially modified by LLMs ([Liang et al., arXiv 2403.07183](https://arxiv.org/abs/2403.07183) [memory]). Low author response on PubPeer: 7.5%, with 85.6% of comments anonymous ([Ortega 2022, via Wikipedia](https://en.wikipedia.org/wiki/PubPeer) [snippet]).

**For us:** A single binary verdict per validator, from one or two LLMs, is noisy evidence. That is acceptable only because our verdicts are cheap to repeat and executed checks dominate the gate. The lesson is not to abandon pass/fail. It is to record *what kind* of evidence each verdict is and *how strong*, on a defined, machine-readable ordinal scale, so policies can require, for example, "an executed check plus a judgement of at least `solid` from an independent class".

### Finding 9. Retraction doesn't reach dependants unless the system pushes it

- Only 5.4% of 13,252 post-retraction citation contexts in PubMed Central acknowledged the retraction, and 35% of retracted papers were cited after the year of retraction ([Hsiao & Schneider, QSS 2021](https://direct.mit.edu/qss/article/2/4/1144/107356/) [snippet]). Other studies find 2% to 9% (the Wakefield paper, at 71.7%, is the exception) ([Scientometrics 2024 review](https://link.springer.com/article/10.1007/s11192-024-05147-4) [snippet]).
- PubPeer comments preceded 20,784 retractions, with a median of 10 months from first comment to retraction ([Retraction Watch, 2026-11-11](https://retractionwatch.com/2026/11/11/do-pubpeer-comments-make-a-difference-a-small-study-tried-to-find-out/) [snippet]). About 5% of the 93,563 papers discussed on PubPeer from 2012 to 2021 were retracted ([arXiv 2310.16059](https://arxiv.org/pdf/2310.16059) [snippet]). Objections arrive long before the formal record changes, and most readers never see either.

**For us:** This is the strongest argument for `needs_recheck` and for `log --open`/`watch --for-me`. A status that changes *structurally* when something it depends on is superseded or retracted, with the change pushed to the people whose work depends on it, does what scholarly publishing has failed to do for 30 years. Ours is opt-in (`follow_changes`, off by default; protocol.md, "Following changes"). It follows only `depends_on`, `implements` and `refines`. It doesn't treat a retracted *method* or *artifact* cited by evidence as a change. An open, untriaged finding (PubPeer's comment, an "expression of concern") changes nobody's status until a triager acts.

### Finding 10. Committing to evaluation criteria before results changes outcomes

- Registered Reports, reviewed and accepted before results exist, had 44% positive first hypotheses against 96% in the standard psychology literature (71 against 152 papers) ([Scheel, Schijen & Lakens, AMPPS 2021](https://doi.org/10.1177/25152459211007467) [snippet]). The authors caution that some of the gap may be riskier hypotheses, but the direction is what pre-commitment predicts.
- Preregistration alone (without in-principle acceptance) showed no significant difference in one later study ([van den Akker et al. 2023, via Collabra](https://online.ucpress.edu/collabra/article/11/1/137035/209911/) [snippet]). The *binding* commitment of the judge matters, not the paperwork.

**For us:** Causal parents make pre-commitment provable without trusted clocks. If a policy or a "done when" criteria claim is an ancestor of the work and of the evidence, every replica can verify that the criteria came first. Weave's line policies citing an owner decision already do this for code. Tasks state "Done when:" only in prose, and reader-side policies are CLI flags (`Policy`, `crates/manyroot/src/validation.rs:20`) that leave no record.

### Finding 11. Formal models fit our relations closely and are cheap at our scale

- **Truth maintenance.** A JTMS keeps, for each belief, the justifications (in-lists and out-lists) that support it, and relabels incrementally when a premise changes ([Doyle, AIJ 1979](https://doi.org/10.1016/0004-3702(79)90008-0) [memory]). An ATMS instead labels each belief with the minimal sets of assumptions under which it holds, so "under which assumptions is this accepted?" is a lookup ([de Kleer, AIJ 1986](https://doi.org/10.1016/0004-3702(86)90080-9) [memory]). `needs_recheck` is a JTMS label whose justification is "these passes, given these dependencies as they were".
- **Argumentation.** Dung's frameworks define which arguments survive a set of attacks. The *grounded* extension is unique, skeptical and computable in polynomial time by iterating "accept the unattacked, reject what they attack" ([Dung, AIJ 1995](https://doi.org/10.1016/0004-3702(94)00041-X) [memory]). Bipolar frameworks add support (Cayrol & Lagasquie-Schiex 2005 [memory]). ASPIC+ separates strict from defeasible rules and adds preferences, so a stronger argument's attack succeeds and a weaker one's doesn't (Modgil & Prakken 2014 [memory]). Recent LLM work builds quantitative bipolar frameworks to make verdicts explainable and *contestable*: a user can see and attack the argument that decided a case ([ArgLLMs, arXiv 2405.02079](https://arxiv.org/abs/2405.02079) [snippet]; [ArgRAG, 2508.20131](https://arxiv.org/html/2508.20131) [snippet]; [MArgE, 2508.02584](https://arxiv.org/html/2508.02584) [snippet]).
- **Provenance semirings.** Annotate each base fact with a variable, and a query result's annotation is a polynomial recording exactly which combinations of facts derived it. Why-provenance, lineage, counts and trust scores are all evaluations of that one polynomial ([Green, Karvounarakis & Tannen, PODS 2007](https://doi.org/10.1145/1265530.1265535) [memory]).
- **Peer-review argument structure.** Micropublications' `supports`/`challenges` and CiTO's typed citations (`supports`, `disputes`, `extends`, `usesMethodIn`, `confirms`, …; [CiTO](https://sparontologies.github.io/cito/current/cito.html) [memory]) are close to our seven relation kinds.

**For us:** Our acceptance rule is already a labelling algorithm over an evidence graph. Naming the formal models gives three things cheaply. Explanations: provenance as the set of ids behind a status. Principled treatment of `contradicts`: grounded semantics instead of the bespoke triage logic in `findings`. A principled answer to "what must be re-checked", from the TMS. All three run in time linear or polynomial in the subgraph around a subject, and all are deterministic, so they fit "derived state, never replicated" (protocol.md, Derived state).

### Finding 12. Shared-board coordination works, but clutter, contention and access control are its known failures

- **Blackboards for LLM agents.** A central agent posts requests and specialists volunteer. This beat RAG and master–worker baselines on data-discovery tasks, by 13% to 57% relative according to a secondary summary ([arXiv 2510.01285](https://arxiv.org/abs/2510.01285) [snippet]). Another blackboard system replaces per-agent memory with a public board and picks the next agent from its content ([LbMAS, arXiv 2507.01701](https://arxiv.org/abs/2507.01701) [snippet]). Tuple spaces (Linda) and stigmergy (marks in a shared environment that steer later work) are the older forms [memory].
- **Known failures.** A 2025 survey of agent memory lists memory clutter, write contention and the lack of role- or permission-aware access as the open problems of shared memory ([arXiv 2512.13564](https://arxiv.org/pdf/2512.13564) [snippet]). Collaborative-memory work adds dynamic access control per user ([arXiv 2505.18279](https://arxiv.org/abs/2505.18279) [snippet]).
- **Phase 1 is our own case study.** Pending work was invisible, 52% to 61% of output duplicated work already delivered, and about 60% of passes went to work that was already accepted (phase1-bracken-lessons.md).

**For us:** The Ground is a blackboard with a durable, signed history. Intents with expiry are stigmergic marks, and the work queue (agents.md) is the control component. The remaining clutter problem is the one in section 0.2: the board can't tell its readers which entries are requests, which are answers and which are minutes.

## 2. What our design gets right

- **Typed, immutable, signed contributions, with evaluation separate from assertion.** These are nanopublications without RDF: assertion (claim or artifact), provenance (author, parents) and publication info (signature), behind a self-verifying content id, as Trusty URIs are. The split between claims and evidence is what MemIR, SEPIO and micropublications argue for (finding 4).
- **Supersession, never deletion, with a deterministic `--current` projection.** This is Wikidata's truthy view, Graphiti's invalidation and a ledger's reversing entry (finding 3). Only an author can retire their own contribution, so a vandal can't, and anyone can propose a replacement the author adopts. That works for organisations and records as well as for code.
- **Acceptance as the reader's policy over shared evidence** (protocol.md, Goals 4). This is eLife's "publish the reviews, let readers weigh them", and Wikidata's verifiability rather than truth. It avoids a global notion of truth that no federated system can hold.
- **Popperian asymmetry with a Duhem-Quine escape.** One counted fail rejects. A fail with `--depends-on` blames the failure on an auxiliary premise and is released when that premise is accepted. This is a faithful encoding of how a failed test is interpreted in practice.
- **`needs_recheck` is a justification-based TMS label** that propagates without anyone noticing (finding 9).
- **Bounded, incremental reads.** Cursors, `--limit`, compact lines, shortest unique ids, `watch --for-me` and `status --as`, each fixing a measured Phase 1 cost (findings 5 and 6).
- **Intents with expiry, and a queue defined as a derived view** (agents.md). These are a blackboard's control and stigmergy without a central scheduler.
- **CLI first, MCP as a thin layer later**, with JSON for every read (AGENTS.md section 2). There is one surface to secure, and fewer, better tools is what the tool-design guidance favours.
- **Grounds as bounded workspaces that reference each other** (vision.md). Filtering starts at the Ground boundary, as Wikidata subsetting and federated knowledge graphs do.

## 3. What it gets wrong or is missing

1. **One relation kind carries several meanings, and claims have no type.** A report `implements` a task and code `implements` a spec, and both enter the review queue (section 0.2: 247 waiting, oldest first, `crates/bracken/src/main.rs:297`). Decisions show `pending (0 passing)`. In a finance or organisational Ground it would be worse: a budget line, a board resolution and a meeting minute would all be claims, with nothing saying which ones evidence decides.
2. **No machine-readable attributes.** Valid time, owner attribution, PR numbers, shas, severities, file:line and amounts live in statement text (decisions start "Owner, 2026-10-26:", and `pr` claims follow a prose template in AGENTS.md section 3). Nothing can filter, index or check them. The intent convention (`until:<time>` as a tag, cli.md, Intents) shows the cheap fix we haven't generalised.
3. **Evidence is a bare pass/fail with a free-text method** (protocol.md, Body kinds). It can't state its kind (executed, review, triage, attestation), its strength or its independence class. The gate encodes all three in `--method "independent review (<model>) @ <sha>"` strings and in `scripts/merge` arguments.
4. **`supports` counts for nothing, and `contradicts` only yields "conflicts".** No general semantics combines them, so "this is contested by an unanswered finding" is computed by an application rule (`findings`, cli.md) specific to the merge gate.
5. **Policies live in CLI flags, outside the Ground** (`crates/manyroot/src/validation.rs:20`). Two readers can't show they used the same policy, and criteria can't be recorded before the work they judge (finding 10), except on Weave lines.
6. **`follow_changes` is off by default and narrow:** it doesn't treat a retracted method or artifact cited by evidence as a change, and an open finding changes no status until triaged (finding 9).
7. **Reads are O(Ground) every time.** Each command loads a 16 MB snapshot and recomputes derived state. That's fine at 14k contributions. At 1M it is a snapshot near 1 GB and seconds per call, for every agent, on every read. There are no persistent indexes by type, tag, target or relation, and no memoised view keyed by heads.
8. **Views are unstable for caching:** relative ages, age-ordered sorts, a moving 40-line cut-off.
9. **No middle detail level and no digests.** Compact (120 characters) and full (4x) are the only choices, and nothing consolidates 116 current gotchas. MemGPT's small core memory over a large archive ([Packer et al., arXiv 2310.08560](https://arxiv.org/abs/2310.08560) [memory]) and Generative Agents' periodic *reflection*, which writes higher-level observations citing the memories they came from ([Park et al., UIST 2023](https://arxiv.org/abs/2304.03442) [memory]), both point to sourced consolidation.
10. **No validator calibration.** Nothing records whether a validator's passes were later contradicted, so reviewer quality is invisible (finding 8). The graph holds what's needed to compute it.
11. **No severity relative to the claim.** A `cargo fmt --check` pass and an end-to-end test pass count the same toward `min_passes`. In Mayo's terms, a test the claim would pass even if it were false has low severity and confirms little [memory]. Nothing ties method classes to the claim types they can confirm.

## 4. Design

### 4.1 The read interface: views, detail levels, cursors and caching

**Principles**, each from a finding:

- **Filter by type and attribute before reading text** (findings 1 and 4).
- **Return what the next action needs:** ids for the next call, a short summary, and the reason (`because`) (finding 5).
- **Three nested detail levels.** `ids` ⊂ `summary` ⊂ `full`, where each level's output for an item is a prefix of the next level's. A harness can then upgrade one item without re-reading the rest (finding 6).
- **Deterministic order** (causal position, then id), absolute UTC times and no relative ages, so the same heads give the same bytes (finding 6).
- **A token budget parameter.** The view fills the budget in priority order and ends with a line saying what it left out and how to get it, so truncation is never silent (Phase 1's 50,000-character failures).
- **Every view takes `--since <cursor>`** and has the same output in JSON.

**`brief`: what an agent reads at the start of a task.** It replaces AGENTS.md's four "read whole" commands plus `show <task>`. It is illustrative: the lines below are condensed from real Ground entries, and the format is the proposal.

```
$ bracken brief --as builder-eval-backend --task 61911c91 --budget 3000
brief 2026-11-21T09:14Z heads 3f2a…(4)  policy gate-builder@a1b2c3d4  budget 3000, used 2210
TASK 61911c91 (manager, 2026-11-20) Eval stage 4: integrate every turn; refuse unknown tool calls.
  done-when: fmt, clippy x2, test-table --remote recorded on the head | branch eval-stage4
  state: implemented-by pr 6073a0a6 (PR #51 @ 3b9830cf) — accepted: manager e…, security f… (2 classes)
BINDING DECISIONS (type:decision, applies:builder or applies:all, 12 of 164; --level full for text)
  b1b56d75 2026-11-19 owner  Risky classes get an independent reviewer; every PR a security review.
  c0948037 2026-11-19 owner  Independent reviewer stays on Gemini 4 Neon (High); briefs say review by reading.
  851b5ebc 2026-11-20 owner  If Gemini can't run, reviews run on Claude Opus 5.5 at medium effort.
  …9 more
GOTCHAS (type:gotcha, topic:eval or topic:cargo, 7 of 116)
  …
PREFERENCES (11, all shown) …
FOR YOU since your last cursor: 2 findings contradict pr 6073a0a6 (1 untriaged), 0 questions answered
next: bracken show <id> --level full | bracken brief … --since 3f2a…
```

Token arithmetic, measured on this Ground:

| What the agent reads | Today | With `brief` |
|---|---|---|
| Decisions | all 164: 26 KB compact (~6.5k tokens, cut at 120 characters) or 123 KB full (~31k) | first sentences of all 164 is 11.4 KB (~2.9k tokens); filtered by `applies:`/`topic:` to ~10–30 items, ~0.3k–0.9k |
| Gotchas | all 116: 19 KB compact (~4.7k) or 65 KB full (~16k) | first sentences of all 116 is 5.1 KB (~1.3k); filtered to ~5–15, ~0.15k–0.4k |
| Preferences and references | ~3 KB (~0.7k) | first sentences ~0.9 KB (~0.2k) |
| The task | `show`: 5.5 KB (~1.4k) | ~0.3k (statement head, done-when attributes, state with `because`) |
| What concerns me | `status --as`: 6.7 KB (~1.7k) | ~0.1k–0.3k (counts plus ids) |
| **Total** | **~15k compact, ~50k full** | **~1.5k–4k**, with full text one `show` away |

The filtered figures depend on recommendation R1/R2 (types and `applies:`/`topic:` attributes), which this Ground lacks today: 148 of 164 decisions have no topic tag. Without them, `brief` can still deliver the first-sentence column (~4.3k tokens for all four tags), a 3x saving over compact with no truncation mid-clause.

**`queue`: what needs doing, by role.** This is the agents.md queue with types:

```
$ bracken queue --role validator --type spec,design --json --level ids
{"heads":"3f2a…","policy":"gate-builder@a1b2c3d4",
 "needs_pass":[{"subject":"e41c…","type":"design","for":"9d02…","author":"builder-x",
                "has":{"executed":1,"review":0},"needs":{"review":1,"independent_of":["anthropic"]}}],
 "contested":[{"subject":"77ab…","because":{"open_attackers":["f3e1…"]}}],
 "needs_recheck":[{"subject":"5c90…","because":{"changed":["a8d1… supersedes 2b44…"],"recheck":["executed"]}}],
 "omitted":0,"cursor":"3f2a…"}
```

Each entry costs about 40–80 tokens at `ids`, and says *what kind of pass from whom* would move it. Phase 1's agents assembled this from 27k–52k characters of reads per turn. Reports, notes and decisions never appear, because their types say evidence doesn't decide them (R1).

**`status --type <t>` and `show --level`:** the existing commands, restricted by type, with `because` on every status (R5):

```
$ bracken show 6073a0a6 --policy gate-builder --json --level summary
{"id":"6073a0a6…","type":"pr","attrs":{"pr":"51","branch":"eval-stage4","sha":"3b9830cf…"},
 "status":"accepted","policy":"gate-builder@a1b2c3d4",
 "because":{"passes":{"executed":["0c1d…","9e77…"],"review:security":["e0c3…"],"triage:manager":["cd87…"]},
            "classes_met":["executed","review"],"attackers":{"f3e1…":"out (rejected by triage 4a2b…)"}}}
```

**Cursors and caching.**
- **Memoise by heads.** A view's result is a pure function of (view, arguments, policy id, heads). The application caches it under that key (R10). Two agents reading the same brief at the same heads cost one computation.
- **Make output cache-friendly for harnesses.** Agents get prompt-cache hits when a harness re-reads a brief and only the tail changed (the `FOR YOU` and `since` sections come last for that reason).
- **Extend the cursor's reach.** Cursors are already full-id head sets and replica-independent (`crates/bracken/src/listing.rs:20`). Keep them, and add `--since` to `queue`, `brief` and `status`, so "what changed in my queue" is an append, not a re-read.

**MCP and the web.** When MCP comes, each view maps to an MCP *resource* (a stable URI such as `ground://<ground>/brief?as=…&task=…`) with change subscriptions fed by `watch`, and writes map to tools. Resources are application-controlled context that a harness can cache and subscribe to, which matches views keyed by heads [memory: MCP resources and subscriptions]. The web view's `/api/status` (docs/design/visualizer.md §1.3) should serve the same `because` objects, so people and agents see one explanation.

### 4.2 The write interface: typed claims, attributes, shapes and one-call writes

**Types and attributes as tags, checked by shapes.** A claim gets `type:<name>` and `key:value` tags (`type:decision`, `applies:builder`, `valid_from:2026-11-19`, `source:owner`, `severity:medium`, `sha:3b98…`). A project's *schema* (section 4.3) declares, per type:

- required and optional keys, their value types (date, enum, id, integer, decimal with unit), and allowed values;
- which relations may leave and enter it (`report` may `implements` a `task`; `finding` must `contradicts` something);
- how its acceptance is decided: `evidence` (policy over evidence), `authority` (valid when its author holds a role, with an optional quorum of co-signers via `supports`), or `record` (never pending, never queued);
- which identities may write it (the server's `scopes` TOML, cli.md "Keys a server holds", moved into the Ground).

This is SHACL's model (shapes over a graph; violations reported with the focus node, path and message) without RDF. SHACL validation is already used as the "unit test" for LLM-written graphs, with an LLM repairing each reported violation until none remain ([arXiv 2610.09644](https://arxiv.org/pdf/2610.09644) [snippet]; [KG repair with SHACL, 2507.22419](https://arxiv.org/pdf/2507.22419) [snippet]). Explaining violations in plain language is its own line of work ([xpSHACL, 2507.08432](https://arxiv.org/pdf/2507.08432) [snippet]).

- **Enforce at write, report on read.** `claim` refuses a write that breaks its type's shape, naming the missing key and an example. That is cheap for an agent to repair, and the cheapest point to catch it. Reads never refuse: they flag violations (`bracken check-shapes`), because a Ground merges contributions written under older schema versions and from other Grounds. Wikidata takes the same stance, with property constraints reported, not enforced ([Help:Property constraints portal](https://www.wikidata.org/wiki/Help:Property_constraints_portal) [memory]).
- **One-call writes.** `claim --type finding --contradicts <pr> --attr severity=medium …` writes the claim and its relations together, as `artifact --implements` already does. Phase 1 stranded 46% to 64% of implementations without their relation when turns ended mid-batch.
- **Ids.** Writes accept unique prefixes and echo the full id once (AGENTS.md: "a write prints one labelled line per record"). Agents should carry 8–12-character prefixes in their notes.

**Why tags first, and when a protocol field.** `key:value` tags need no format change, already sort and filter, and the intent convention proves the pattern. Their limits are no typed values in the encoding, no structured values (lists, units), and the parsing on every read. If those bite, the generic fix is an `attrs` map on claims and evidence (text keys to scalar values) at the next protocol version. That is a frozen-format change and needs owner approval (AGENTS.md section 1), so it should come only after the convention has shown which keys matter.

### 4.3 The evidence vocabulary and independence classes

Evidence today is `{target, verdict, method: text, details: text}`. The proposal keeps that body and adds, by convention, `class:`, `strength:`, `independence:` and `method_id:` tags on the evidence, or a reference to a method claim. A project's schema defines the vocabulary.

**Method classes.** They generalise across domains. The class list is adapted from audit evidence (ISA 500's inspection, observation, external confirmation, recalculation, reperformance, analytical procedures and inquiry; [IAASB ISA 500](https://www.iaasb.org/publications/international-standard-auditing-500-revised-audit-evidence) [memory]), from SEPIO and ECO evidence types, and from our own gate:

| Class | Meaning | Software example | Finance / organisation / process example | Reproducible by a reader? | Default reliability rank |
|---|---|---|---|---|---|
| `executed` | A deterministic procedure run on the exact subject | `cargo test`, test-table on the head | recalculation of a model; reperformance of a control | yes, re-run | high |
| `measurement` | An observation with uncertainty | benchmark timings | sales figures, survey results, process-mining conformance on an event log | partly (data cited) | medium–high |
| `replication` | An independent re-derivation of the result | a second implementation agreeing | an independent re-model of the forecast | yes, with effort | high |
| `confirmation` | Agreement from an external source | upstream CI, a vendor's attestation | bank confirmation; registry lookup | depends on source | high if external |
| `review` | A judgement by a person or model reading the subject | independent review, security review | a peer's review of a pricing design | no (another review is another judgement) | medium |
| `attestation` | A role-holder signs off; authority, not truth | manager triage, owner approval | board resolution, auditor's sign-off | no | n/a: decides `authority` types |
| `inquiry` | Testimony | "the author says it was tested" | management representation | no | low |

**Strength.** An ordinal scale per project, defined in the schema with a sentence per step, as eLife's is. The PLOS Biology study shows the definitions matter. The default is eLife's six-step evidence scale (inadequate … exceptional). A `fail` needs no strength, and any counted fail still rejects (Popper's asymmetry is kept). Strength lets a policy say "a review counts only at `solid` or above". It is optional: an executed check is pass/fail.

**Severity relative to the claim.** The schema says which classes and methods can confirm which types. `cargo fmt --check` can confirm `type:style` but not `type:spec`, and a recalculation can confirm a financial fact but not a policy's appropriateness. This encodes Mayo's severity requirement [memory] and the Bayesian point that evidence confirms in proportion to its likelihood ratio, P(pass | claim true) / P(pass | claim false). A check that passes either way has a ratio near 1 and should not count.

**Independence classes.** Each validator identity has a *profile*: a claim of `type:agent-profile` with `model_family:`, `model:`, `harness:`, `operator:` and `org:` attributes, written by the identity and attested (`supports`) by the Ground's root or the role-holder who created it. A policy names an *independence key*, a subset of those attributes, and counts passes by distinct key values instead of distinct authors:

```toml
# A per-project schema fragment: an artifact or claim of type:schema, superseded like any decision
[evidence.classes]
executed    = { reproducible = true,  recheck_on_change = true }
review      = { reproducible = false, strength_scale = "elife-evidence" }
attestation = { reproducible = false, decides = "authority" }

[independence]
review   = ["model_family", "harness"]   # two reviews count twice only if these differ
executed = ["runner"]                    # the same check on two runners: a second execution, not a second witness

[confirms]                               # which classes and methods may confirm which types
spec   = { classes = ["executed", "review", "replication"] }
style  = { methods = ["cargo fmt --check"] }
budget = { classes = ["executed", "confirmation", "review"] }
```

Two Claude Opus reviewers in the same harness then count once. Gemini plus Claude counts twice. A human reviewer (`model_family:human`) is its own class. This is the minimum the "nine judges, two votes" result (finding 7) demands. It can't detect hidden correlation, such as two families trained on similar data. For that, R11's calibration measures agreement after the fact, as source-dependence detection does in truth discovery (Dong et al. 2009 [memory]).

**Sybil caution.** Independence classes are only as good as the profiles. Profiles must be attested by an identity the policy trusts, or anyone can mint "independent" validators. This is where vision.md's "trust at scale" problem lands. EigenTrust-style transitive trust ([Kamvar, Schlosser & Garcia-Molina, WWW 2003](https://doi.org/10.1145/775152.775242) [memory]) is a later option, computed by the reader, never by the protocol.

### 4.4 Acceptance semantics: a worked example

A small Ground in a non-software domain (a pricing design for a sales organisation) shows that the same machinery serves any domain. The identities: `finance-lead` (role: goal setter), `analyst-a` (builder), `checker-bot` (runs a spreadsheet check; profile `runner:sheets-ci`), `reviewer-x` and `reviewer-y` (profiles `model_family:anthropic`, same harness), and `auditor-h` (`model_family:human`).

| Id | Kind and type | Author | Content |
|---|---|---|---|
| S1 | claim, `type:spec` | finance-lead | "Tier B discount never exceeds 15%." |
| R0 | claim, `type:reference` | finance-lead | "Q3 FX table", `valid_from:2026-07-01` |
| C4 | claim, `type:rule` | finance-lead | "Promo codes don't apply to tier B." |
| D2 | artifact, `type:design` | analyst-a | Tier B discount table v2; relations: D2 `implements` S1, D2 `depends_on` R0 |
| P  | claim, `type:policy` (ancestor of all evidence below) | finance-lead | for `type:design`: needs ≥1 `executed` and ≥1 `review` in distinct independence classes; contradictions by `finding`s count when untriaged or confirmed; `follow_changes` on, `recheck: [executed]` |
| E1 | evidence pass on D2, `class:executed` | checker-bot | `check-discounts` on D2 |
| E2 | evidence pass on D2, `class:review`, `strength:solid` | reviewer-x | read the table |
| E3 | evidence pass on D2, `class:review`, `strength:convincing` | reviewer-y | read the table |

**Step 1. Counting.** Under today's reference policy with `min_passes 2`, D2 is accepted on three distinct authors. Under P, the passes fall into classes {executed: `sheets-ci`} and {review: `anthropic`/same harness}. E2 and E3 share one independence key, so they count as one review. P needs one executed and one review, so D2 is **accepted**. The `because` is {E1; E2 or E3}, a provenance polynomial E1·(E2 + E3) in semiring terms: removing E2 alone doesn't change the status, but removing E1 does.

**Step 2. An objection: grounded argumentation.**
F1 is a claim `type:finding` by auditor-h: "Row 14 gives 18% under a promo code", with F1 `contradicts` D2. Arguments and attacks: F1 → D2. Grounded labelling iterates:

- Round 1: F1 is unattacked, so **IN**. D2 is attacked by an IN argument, so **OUT**.
- D2's status under P is **contested** (accepted on evidence, defeated in argument), and `queue` lists it under `contested` with `open_attackers: [F1]`.

analyst-a answers with R1, `type:rebuttal`: "Row 14's promo path is unreachable: C4 says promo codes don't apply to tier B", R1 `contradicts` F1, R1 `depends_on` C4.

- Round 1: R1 unattacked, **IN**. F1 attacked by IN R1, **OUT**.
- Round 2: D2's only attacker is OUT, so D2 is **IN**. Status back to **accepted**, `because` now includes "attacker F1 out, defeated by R1".

auditor-h replies with F2: "C4 was superseded on 2026-11-02 by C4′, which allows promo codes on all tiers", F2 `contradicts` R1. Suppose that is true in the graph: C4′ `supersedes` C4, by finance-lead.

- Round 1: F2 unattacked, **IN**. R1 **OUT**.
- Round 2: F1's only attacker is OUT, so F1 **IN**. D2 **OUT**: **contested** again.

Two further points. First, TMS propagation would have caught this without F2. R1 `depends_on` C4, and C4 is superseded, so R1 itself is `needs_recheck`. Under P a rebuttal that needs a recheck stops counting as an attacker, so F1 comes back IN mechanically. Second, the evaluation is linear in the number of attack edges in D2's neighbourhood. Here it is five contributions and three rounds.

**Step 3. Which attacks count.** Unrestricted, any identity could stall any subject by posting contradictions. So P says an attack counts only if its source is *admissible*: a `finding` from an identity with a validator role, or any finding once a triager confirms it, and never one the triager rejected. This generalises what `findings` already does for the merge gate (cli.md: untriaged and confirmed block, rejected don't) into a rule any Ground can configure. Grounded semantics is the right default for gates: it is unique and skeptical, so an unresolved objection blocks. A Ground that wants to *explore* alternatives can ask for preferred extensions (all maximal consistent positions), which list the coherent "camps" instead of blocking. That suits an exploration layer (vision.md, "Exploration and acceptance").

**Step 4. A dependency changes: the TMS recheck.**
finance-lead supersedes R0 with R0′ (the Q4 FX table). D2 `depends_on` R0, by D2's own author, so R0 is in D2's dependency closure (protocol.md, "Following changes"). A JTMS sees that D2's justification {E1, E2|E3, R0 current} has lost a premise, so the label goes to **needs_recheck**. The ATMS view gives D2's label as the environment {R0}: D2 holds given R0, and nothing yet establishes it under {R0′}.

What must re-pass? Today's rule asks for `min_passes` passes with the change among their ancestors. P says `recheck: [executed]`: an FX change invalidates the calculation, not the reviewers' judgement of the table's design. So one new `executed` pass whose ancestors include R0′ returns D2 to **accepted**. The `because` shows `{"recheck_satisfied_by":"E4","after":"R0′ supersedes R0"}`. Per-class recheck turns "every change sends everything back to review", which Phase 1's agents would have experienced as churn (acceptance-follows-changes.md), into a cheap re-run for the class that actually depends on the changed input.

**Step 5. Explanation cost.** Every label above can be explained with a handful of ids: the counted passes, the IN attackers, the changed dependency. An agent asking "why is D2 contested?" gets `{"open_attackers":["F1"],"F1_in_because":"R1 needs_recheck: C4 superseded by C4′"}`, a few dozen tokens, instead of reading the thread.

**What stays where.** The labelling algorithms are application code, deterministic and pure. P is per-project configuration. Nothing in the protocol changes. supports: in a bipolar framework support can transmit attacks (if A supports B and C attacks A, C indirectly weakens B). The simplest useful rule is that `supports` from an admissible identity counts as a `review`-class pass on the target if P says so; otherwise it is shown, not counted. Quantitative (gradual) semantics of the kind ArgLLMs use are an option for exploration views, not for gates, because their numbers aren't comparable across Grounds.

### 4.5 Humans and non-software domains

The same structures serve people, and domains other than code, if the configuration layer carries the domain:

- **Finance and financial design systems.** XBRL's model of a *fact* is a value with a concept, an entity, a period, a unit and a precision, defined by a taxonomy [memory]. That is a claim with `type:fact` and attributes `concept:`, `entity:`, `period:`, `unit:`, `decimals:`, with the taxonomy as the schema's shapes. Double-entry corrections are reversing entries (supersede), not edits. An audit's evidence classes are section 4.3's table. A "close" of a period is an `attestation`-decided claim that the period's facts are accepted under policy P, and later adjustments `supersede` with `valid_from` inside the closed period, which `needs_recheck` then flags on the close.
- **Organisations and governance.** A resolution is `type:resolution`, decided by `authority`: valid when its author holds the role at the time (a `type:role-assignment` with `valid_from`/`valid_to`) and co-signed by a quorum (`supports` from other role-holders). Delegation is `refines`. Minutes are `record`. The Ground already behaves this way for owner decisions and approvals (AGENTS.md section 1). The schema would make it explicit and checkable rather than a habit of tags.
- **Business processes.** A BPMN-style process is a set of `type:process-step` claims with `depends_on` order. Process-mining *conformance checking*, comparing an event log against the model, is `measurement` evidence on the process claim [memory]. A changed step puts every dependent step's conformance into `needs_recheck`.
- **Science.** Section 4.7.

**For people**, three things matter beyond what agents need:

1. **Narrative.** A person reads a subject's *life*: the claim, its versions, its evidence and objections in order. `bracken view`'s Root view (visualizer.md §1.5) is that. It should show the same `because` and argument labels, so the person and the agent see one explanation.
2. **Assessments in words.** eLife's significance and strength scales are written for humans. A schema's ordinal scales carry a one-sentence definition per step, shown beside the label, and a person writing evidence picks from the scale rather than typing a method string.
3. **Forms instead of conventions.** The shape that refuses an agent's malformed write is the same thing that renders a human's input form. Required keys become required fields, enums become menus, and `applies:` becomes a picker of roles. AGENTS.md's prose templates for `pr` and `finding` claims are what shapes make unnecessary.

### 4.6 Query engine and performance

- **Datalog behind the views.** Our derived state is already rules over a graph: retraction, supersession, acceptance, the dependency closure, the argumentation fixpoint and shapes. Datalog expresses these as stratified, recursive rules with deterministic answers, and incremental engines maintain their outputs as facts arrive. Differential dataflow and DBSP's incremental view maintenance are examples (McSherry et al., CIDR 2013; [Budiu et al., VLDB 2023](https://doi.org/10.14778/3587136.3587137) [memory]). Agents don't write Datalog (finding 5); the views are named Datalog queries, and people and tools can run their own.
- **Indexes.** Persistent indexes by type, `key:value`, target-of-evidence, relation endpoints and author, maintained on write and sync, held in `cache/` beside the snapshot and never replicated. Each view then reads O(answer), not O(Ground).
- **Memoised views keyed by heads** (section 4.1).
- **Scale estimate.** At today's 14k contributions a full recompute is ~0.65 s. At 1M (vision.md's 100–1,000-agent phase), the snapshot would approach 1 GB, and a recompute on every agent read would dominate wall time. Incremental maintenance makes a read's cost proportional to what changed since the last cached heads, which `watch` already tracks.

### 4.7 The scientific method and peer review mapped onto the Ground

| Scientific practice | What it encoded | What worked and failed in practice | Our construct | Gap and recommendation |
|---|---|---|---|---|
| Claim and hypothesis | An assertion with attribution | Nanopublications and micropublications work at small scale; adoption was slow outside curated bio databases [snippet] | claim | `type:hypothesis` versus `type:result` (R1) |
| Pre-registration and Registered Reports | Criteria fixed before results | 44% against 96% positive results (finding 10); preregistration without binding review showed no difference | Weave line policies, task "Done when" prose | Policies and criteria as Ground contributions that are ancestors of the evidence (R4) |
| Peer review | Expert judgement before publication | Half of accepted papers would be rejected by a second committee; collusion; LLM-written reviews (finding 8) | evidence with `review` | class, strength, independence (R3); calibration (R11) |
| Public, post-publication review (eLife, OpenReview, PubPeer) | Reviews published; review continues after acceptance | Readers get reviews; low author response (7.5% on PubPeer); 10 months from comment to retraction (finding 9) | evidence and findings stay attached and live; `log --open`, `watch --for-me` | Push open findings to dependants' authors, not only the target's (R7) |
| Replication | Independent re-derivation | Replication projects find many effects don't replicate; replications are rarely published [memory] | evidence `executed` by a non-author | A `replication` class with its own independence key; policies can require it for high-significance types |
| Correction and erratum | A new version supersedes | Crossref's Crossmark shows updates to readers who look [memory] | `supersedes` by the author | Works; `follow_changes` propagates it |
| Retraction | The record is withdrawn | Only ~5% of later citations acknowledge it (finding 9) | `retract` | `needs_recheck` on by default for derived types, including evidence citing a retracted method or artifact (R7) |
| Expression of concern | Unresolved doubt, flagged | Rarely issued; weakly correlated with later retraction ([Ortega 2021](https://www.researchgate.net/publication/346264747) [snippet]) | an untriaged finding that `contradicts` | `contested` status under grounded semantics (R6) |
| Meta-analysis and systematic review | Aggregating many studies, weighted | Garbage in, garbage out; publication bias [memory] | — | `type:digest` that `refines` the sources, with an aggregate strength, superseded when sources change (R9) |
| Typed citations (CiTO) | Why one work cites another | ~90 properties; little used by authors, used by curators [memory] | 7 relation kinds | Enough for computation. A relation's subtype goes in its `note` by convention, or in a refining claim |
| Consensus | What the field accepts | Implicit, slow, contested | the accepted layer under a named policy | Name the policy in every answer, as `because` does |
| Bayesian confirmation | Evidence updates credence by its likelihood ratio | Hard to elicit priors in practice [memory] | — | Method classes plus the `confirms` table approximate diagnosticity without numbers (section 4.3) |
| Popper and falsification | One sound refutation defeats | Duhem-Quine: which premise failed? | a counted fail rejects; `--depends-on` blames an auxiliary | Already right |
| Lakatos's research programmes | A hard core protected by a belt of auxiliary hypotheses; progressive programmes predict new facts, degenerating ones only patch | Judged in hindsight | `refines`/`supersedes` chains around a core claim | A derived metric per core claim: the ratio of new passing evidence to patches made in answer to contradictions (open question 5) |
| DeSci (token-incentivised review and funding) | Pay reviewers and replicators on chain | Small; incentive gaming remains the open problem [memory, not researched in depth here] | — | Keep incentives out of the protocol; calibration (R11) is the measurable part |

### 4.8 Recommendations

Layers: **P** = protocol (frozen, every application); **C** = per-project configuration, stored in the Ground as signed, superseding contributions that readers select by id; **A** = application (Bracken CLI, web view, Weave, a future MCP layer).

| # | Recommendation | Expected benefit | Cost | Layer |
|---|---|---|---|---|
| R1 | **Claim types and shapes:** `type:<name>` plus a schema of required keys, allowed relations, acceptance mode (`evidence` / `authority` / `record`) and writers | Clean queues (247 noise entries gone), correct statuses for decisions and reports, exact filters before reading, forms for people | Moderate; tags exist, so no format change | C (schema), A (enforcement, views) |
| R2 | **Attributes as `key:value` tags** (`valid_from`, `applies`, `topic`, `severity`, `sha`, domain keys); a protocol `attrs` map only at a later version if needed | World time, filtering and indexing without parsing prose; generalises to finance, organisations and processes | Low now; a format change later needs owner approval | C now; P later, if ever |
| R3 | **Typed evidence:** class, optional ordinal strength, independence key, and a `confirms` table tying classes to types | Policies can require kinds and independence; severity relative to the claim; human-readable assessments | Low to moderate | C (vocabulary), A |
| R4 | **Policies as Ground contributions,** ancestors of the evidence they judge; counting by independence class; default per type | Reproducible verdicts across readers; provable pre-commitment; correlated validators stop counting twice | Moderate | C, A |
| R5 | **`because` on every derived status:** counted passes, attackers and their labels, changes behind a recheck | One-call explanations for agents and people; audits | Low (the code collects these already) | A; the reference policy's outputs could be specified in protocol.md |
| R6 | **Grounded argumentation over `contradicts`** with admissible attackers; `supports` counting only if a policy says so | One general rule replaces the gate's bespoke findings logic; contested status for any domain | Moderate; run an experiment first | C (opt-in term), A |
| R7 | **`follow_changes` by default for derived types,** including retracted methods and artifacts cited by evidence; per-class recheck; push to dependants via `watch --for-me` | Closes the retraction gap; rechecks cost a re-run, not a re-review | Low to moderate; measure false-recheck rates on our Ground first | C (defaults), A |
| R8 | **`brief`, `queue`, `status --type`:** budgeted, three nested levels, deterministic order, absolute times, `--since` everywhere | Task-start reads from ~15k–50k tokens to ~1.5k–4k; fewer `show` crawls; cache-friendly | Low to moderate | A |
| R9 | **Digest claims:** sourced consolidations that `refine` what they summarise and are flagged stale by R7 | Bounded "core memory" with provenance; meta-analysis for any domain | Manager time, plus R1 and R7 | C (type), A |
| R10 | **Indexes and memoised views keyed by heads;** incremental maintenance | Reads proportional to the answer at 1M contributions | Moderate; a cache only | A |
| R11 | **Validator calibration** per identity and independence class: passes later contradicted, failed or retracted; agreement between classes | Makes weak or colluding reviewers visible; tests whether independence classes are real | Low (a derived metric) | A; C decides if weights ever count |
| R12 | **No text-to-query for agents;** Datalog behind named views; MCP resources mirror the views | Accuracy (finding 5); one engine for views, policies and shapes | Moderate | A |
| R13 | **Re-frame protocol.md's Acceptance as the reference policy** over the core, with R4's policies as peers | Keeps the protocol domain-neutral; policies evolve without format changes | Documentation; no code | P (text only) |

**On the protocol boundary.** The protocol should stay as small as it is: kinds, relations, validity, supersession, retraction, causal parents. Those carry every domain in section 4.5, because they are the shared core of ledgers, event logs, governance records and the scholarly record. What varies by domain (types, shapes, evidence vocabularies, independence keys, policies) belongs in a configuration layer whose own artifacts are Ground contributions: signed, superseded, causally ordered, selected by readers by id. The owner suggested that the most powerful thing may be a way to configure rules. This makes the configuration itself knowledge, governed by the same machinery it configures.

**Weave specifically.** Weave's line policy is already R4 for code, and its check results are `executed` evidence with a runner. It would gain from:
- R3's classes (`box` and `mac` checks as `executed` with `independence: runner`);
- R5's `because` in `weave green` and `weave review`;
- R7's per-class recheck: a manifest changing only `docs/` needs no re-run of checks that don't read `docs/`, if checks declare their inputs, as Bazel's do [memory].

Weave is also where `implements` is most overloaded today (proposals, decisions and reports). R1's types would let its review queue show only proposals.

## 5. Open questions

1. **Does `brief`/`queue` move Phase 1's measures?** The test is to replay a Phase 1 replicate with `brief` and `queue` as the only reads, against the three replicates' ranges: tokens (14.7M), improvements per million tokens (2.69), duplicated output (52% to 61%) and integration time (75 to 97 s). It needs the owner's go for model spend.
2. **How many independence classes are real?** Is a different model family enough, or do the prompt and harness matter as much? R11's calibration on our own ~6,000 evidence records could estimate the effective number of independent validators before any policy changes.
3. **One shared strength scale, or one per project?** eLife's words were read inconsistently. Grounds that reference each other may need a shared numeric scale with per-project labels.
4. **Grounded or preferred semantics?** Grounded semantics is skeptical and unique, which suits gates. Preferred semantics lists the consistent camps, which suits exploration. One per layer may be the answer.
5. **Lakatos in a Ground.** Can "progressive versus degenerating" be measured over `refines`/`supersedes` chains (new passing evidence against patches answering contradictions), and does it predict anything useful?
6. **Write-time shapes across Grounds.** A shape refused at write keeps a Ground clean, but couples writers to a schema version. How should contributions imported from a Ground with a different schema be shown: flagged, translated, or held apart?
7. **Who attests agent profiles?** Independence classes are only as good as the profiles, and the profiles are a Sybil target. Is the Ground root's attestation enough, or does each policy name its own attesters?
8. **Embeddings and discovery.** Exact typed filters cover routine agent reads. Cross-Ground discovery (vision.md, "Discovery") will need semantic retrieval. Should that be a local, non-replicated index in the application, so the protocol never depends on a model?
9. **How much of this is convention before code?** R1, R2, R3 and R9 can begin as tag conventions with `check-shapes` reporting only. Would three weeks of that, measured with `scripts/metrics`, show which keys and types earn their place before anything is enforced?
