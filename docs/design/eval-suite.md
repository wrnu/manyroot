# An evaluation suite: Bracken and Weave on real projects

> **Proposal, 2026-10-30.** The owner's idea: once Bracken and Weave form a
> full development system, test it repeatedly on real software projects, each
> with a spec, a hidden test suite and a golden solution. The owner set two
> things: projects in **Python** (cheap to run, no compile step), and an
> **exploratory** start: run it, watch the behaviour, and decide what to
> optimise from what we see, rather than fixing metrics in advance. The budget
> is open until the cost estimate below is discussed. Nothing here runs a
> model; each run needs the owner's go, as every model run does.

It builds on [agents.md](agents.md) (agents drive Bracken; the experiment
crate becomes a harness-neutral evaluation runner, decision `adf43778c85d`)
and on [phase2-task.md](phase2-task.md)'s Candidate B, which already worked
out grading and sandboxing for a Python library with a held-out test suite.

## The idea in one paragraph

A **project** is a spec, a starting repository, hidden tests and a golden
solution. A **configuration** is how the team works: how many agents, who
integrates, the guardrails, the test cadence, the harness and model. A
**run** puts one configuration on one project, records everything that
happens, and grades the result at checkpoints. Running the same pair several
times shows the spread; running different configurations on the same project
shows what they change. The first runs are for looking, not for scoring; after that, each
improvement is judged against our own earlier runs.

## A project

| Part | Who sees it | What it is |
|---|---|---|
| Spec | agents | a README-style description of what to build, any required interface, and a few public example tests |
| Seed repo | agents | the starting state: empty, a skeleton, or an existing codebase to extend |
| Hidden tests | the grader only | pytest cases and property tests, written from the spec, that judge behaviour, not structure |
| Golden solution | the grader only | proof that the spec can be met and that the hidden tests are right; never shown to agents, never graded against directly |

**Tests come from the spec, not the solution.** A test written from the
golden solution's internals punishes valid alternatives. The hidden suite is
written against the spec, then run against the golden solution, which must
pass all of it. Where the spec leaves a choice open, the tests accept every
reasonable answer, or the spec is tightened.

**Python only** (the owner's choice), and, as a proposal, **standard
library only at first**. No build step, pytest as
the grader, and a sandbox that needs only a pinned interpreter. Third-party
packages come later, if a project needs them, with a pinned, offline wheel
set.

## A ladder of projects

Start small, so a failure points at one cause.

| Rung | Example | What it exercises |
|---|---|---|
| 1. Small, existing conformance suite | a TOML or CommonMark subset, graded by a slice of `toml-test` or the CommonMark spec tests | the whole loop end to end, cheaply enough for many runs |
| 2. Medium, written by us | a small domain library or CLI with about 100 hidden tests (phase2-task.md's interval sets is one) | real parallel work and integration, with no chance a model has seen it |
| 3. A feature on an existing codebase | add a feature to a seed repo of a few thousand lines | reading before writing; not breaking what works |
| 4. A spec that changes mid-run | rung 2, with a requirement revised halfway | whether the integrator adapts, as the owner asked an integrator to |

Rung 1 suites are public, so models may have seen them; they test the
machinery, not the models. The projects that tell us most are rungs 2 to 4,
written for this suite and never published.

**Where projects live.** Specs and seed repos can live in this repository.
Hidden tests and golden solutions can't: this repository's tree is readable
by every agent, goes into the dogfood Weave Ground (decision
`16ff5718c166`), and would be published if the repository ever is. They live
in a separate store that only the grader and a project's authors can read
and that no Ground imports.

**Who writes them** (a proposal): agents can draft a project, with
independent review and the owner approving each one before use. Their
review records only ids and verdicts in the Ground, never test content, and
agents that drafted or reviewed a project never run on it.

## Project format

As built (task `9345098e82aa`, split note `842c4f4821e9`; owner decision
`594307ea44a2`, manager's answers `c008b959c572`). A project is a directory
under `crates/experiment/projects/<name>/`, all of which agents may read;
`crates/experiment/tests/fixtures/project-tiny` is a small example.

```
<name>/
  project.json     the manifest, below
  spec.md          what to build, the public interface, the open choices
  items/<id>.md    one work item each, written as an issue
  seed/            the starting repository, the run's first tree
    <package>/     the code: Python 3.12, standard library only
    tests/         the public tests, test_*.py, standard library unittest
```

`project.json` (format 1) holds:
- `name`; `python`, the minor version, such as `"3.12"` (the host's
  pinned interpreter must match it; the project names no path); `package`,
  the importable package under the seed; and `spec`, `seed` and
  `public_tests` (default `<seed>/tests`), paths inside the project.
- `items`, each with `id`, `kind` (feature, refactor, fix or change),
  `title`, `text` (its issue's path) and `release`: `{"at": "start"}`, or
  `{"after_share": x}` with 0 < x < 1, the share of the run's agent-turns
  used before it reaches the agents. Every item not released at the start
  has the same share: a project releases items at the start and at one share
  after it, no more (owner decision `de365aae05fd`). A change, never released
  at the start, lists in `revises` the items whose requirement it changes.
  Each item is revised by one change at most.
- `limits` on each run of agent code: `cpu_seconds`, `memory_mb`,
  `output_bytes`, `wall_seconds` and `scratch_mb`, by default 10, 512,
  1 MiB, 60 and 256.
- `hidden`: `{"id", "digest"}`, a reference only. The digest is blake3 over
  every entry of the hidden set in path order: each directory, empty ones
  included, by its path, and each file by its path and the blake3 of its
  bytes. So the grader refuses any set but the approved one. No path, test
  name or count is in the project.

**The private store** holds a project's hidden set at `<store>/<hidden.id>/`
and is given to the grader by path when it grades. The set must be a
directory, not a symlink, and is refused if it, or the store, overlaps the
project, is inside a git checkout, or holds a `.git` anywhere, in any case
(as macOS's file systems ignore it), or a file with a second, hard link.
So it never enters this repository, a Ground or a run directory:

```
<hidden.id>/
  hidden.json      format 1; project; groups: [{item, file, applies}]
  tests/*.py       the hidden tests, standard library unittest
  golden/          the golden solution, laid out like seed/
  golden-before/   for a project with a change: the golden solution just
                   before the first change, laid out like seed/
```

A group judges one item with one test file. `applies` is `always`, or, for
an item a change revises, `before-change` or `after-change`: its old tests
until the change is released, its new ones after.

Tests are standard library `unittest`, public and hidden alike, with
property tests as seeded loops, so the sandbox needs only the pinned
interpreter (decision `c008b959c572`).

`bracken-experiment project check <dir>` checks a project: every path a plain
relative path inside it, no symlink or special file anywhere, every item's
issue present, ids unique, changes released after the start and revising
existing items, limits in range, and a seed holding the package and the
public tests. With `--hidden <store>` it checks the hidden set too: the
digest, the manifest's project and items, each group's file under `tests/`,
and the golden solution's package, before the change too if the project has
one. It prints counts only: groups, test files, golden files (and how many
before the change), and items no hidden test judges. `--json` prints the
same as JSON.

The set is read once. Each file is opened and checked to be the regular file
the walk found, and the manifest is parsed, and each group's file looked up,
from the very bytes the digest covers, which the grader is given too.
Errors about the set name nothing inside it: they say what kind of problem,
and give a group by its index.

## Configurations

What a run varies, taken from weave.md's configurations and agents.md:
- **Team:** one agent, or several; and how many.
- **Integrator:** dedicated, rotating, elected, several; its goals.
- **Guardrails and windows:** how much breakage the integrator may accept,
  and for how long.
- **Which head agents build on**, and **test cadence**.
- **Harness and model:** Claude Code first, Pi second (agents.md).

**No external baselines.** The goal is to improve our own system steadily
from what the runs show, not to compare it with other tools (owner, decision
`6f17c6a66d8b`). Each change to Bracken, Weave or a configuration is
compared with our own earlier runs on the same project. A single agent is
still a configuration we can run when a question needs it.

## Record everything, decide metrics later

Because the start is exploratory, a run keeps a full trace, so any metric we
think of later can be computed over every past run, not just future ones:
- every contribution in the run's Grounds, which are already a signed,
  causally ordered log;
- Weave's heads and decisions over time;
- each agent's transcript, tool calls and token use (the runner already
  records these);
- the hidden tests' results at regular checkpoints and at the end, per test;
- wall time, and every cap or failure that stopped a turn.

**Metrics we expect to look at first** (a starting list, not a commitment):
- hidden tests passing over time, not only at the end;
- how long the main head stayed broken, and how it recovered;
- tokens and time per newly passing test;
- duplicated or discarded work;
- review findings, and how many were real;
- how often the integrator chose to accept breakage, and what followed;
- interventions a human had to make.

After the first runs we look at the traces together, decide what we want to
optimise, and write that down before comparing configurations in earnest.
Only then do replicates and spread matter.

## Isolation

The grader runs agent code, and agents must never see hidden tests or the
golden solution. Candidate B's design applies in full:
- a pinned interpreter (as built, an image pinned by digest) run as
  `-I -S -B`, with a cleared environment, reads only of the copy under test and the standard library,
  a fresh scratch directory for each graded version, no network, no process
  but the interpreter, and limits on CPU, memory and output;
- hidden tests stay in the grader's own process: it sends one case's inputs
  to the child and compares the result itself;
- hidden tests and the golden solution never enter any Ground, Weave's
  included, or any directory an agent can read.

Requirements for building the runner (from security's review, evidence
`bfa24c63c84f`):
- **pytest runs in the grader**, calling agent code through a proxy to the
  sandboxed child. Hidden tests never import agent code into the grader's
  process, where it could read expected values, patch asserts or exit early.
  As built, the runner is `unittest`, in a sandboxed host of its own (Grading).
- **The limits are enforced by the operating system**, not in Python:
  `ctypes`, `os` and `subprocess` are all in the standard library.
- **Hidden-test results stay away from agents for the whole run:** out of
  every run Ground, out of the integrator's guardrails and out of every
  prompt. Otherwise the hidden suite becomes an oracle.
- **No access to earlier runs** of the same project: their Grounds, traces
  or results.
- **Isolation is proven before rung 2.** Rung 1's suites are public, so they
  can't show it; the forbidden-access tests must.

That execution profile is a new risky surface, and gets its own tests and
security review, as Candidate B planned.

**The execution profile as built** (`crates/experiment/src/python.rs`, tasks
`9345098e82aa` and `879a58e77bde`, decision `ac22cedd8fee`): agent code runs
in a container on Docker (Docker Desktop on the Mac), where the kernel
enforces the limits that `sandbox-exec` couldn't (disk, memory and
processes). It runs on Linux too. The agent harnesses and Weave's check
runner stay on `sandbox-exec` for now.
- **The image** is pinned by digest with `--python <name>@sha256:<hex>`:
  `python:3.12@sha256:17b9724493d9…` by default (`python::IMAGE`). A pin
  refuses an image not named by its digest, one that isn't here by that
  digest (it isn't pulled: `docker pull` it first), and one whose Python
  isn't the project's minor version. Every container runs it by that name,
  so by that digest, with `--pull never`, so an image removed since the pin
  fails the run rather than being pulled.
- **In and out:** a run's inputs (the tree, the runner, a command's working
  files) are written to a fresh `0700` directory of the host's, mounted
  read-only at `/in`, and copied into the scratch directory, `/scratch`,
  before the code starts. Nothing else of the host's is mounted, so no path
  of the owner's exists in the container. What comes out is the code's
  output, its exit, and, for a grader command, the regular files of its
  working directory, collected once it has ended without following a
  symlink. The host's directory is removed after the run, and the container
  with it.
- **The container:** `--network none`, `--read-only`,
  `--security-opt no-new-privileges`, `--cap-drop ALL` but `SETUID`,
  `SETGID` and `SETPCAP`, which the supervisor needs to start the code as
  another user and gives up, with the whole bounding set, before any code
  runs. The code runs as `nobody` (65534), with no capability, as
  `python -I -S -B -X utf8`, with only `HOME` and `TMPDIR` (both the scratch
  directory), `PATH` and `LANG` set. It can fork and exec inside the
  container, within its process limit.
- **Limits:** CPU time (`SIGXCPU`), file size (16 MiB or the output limit if
  larger) and open files (256) as rlimits. The rlimit is each process's, and
  the code may fork, so the container's CPU time in all, as the supervisor
  says it every 0.1 s, has the same limit: past it the runner kills the run,
  which ends as `SIGXCPU` too. It's counted from the supervisor's first
  report, made just before it starts the code, so the supervisor's own start
  and the inputs' copy aren't the code's, and none of the code's is left out
  however late the supervisor runs after; what the supervisor uses after,
  relaying the output, is the code's, a little. Memory is the container's cgroup
  limit, with no swap: the run's `memory_mb`, **plus its `scratch_mb`**,
  since the scratch directory is a `tmpfs` held in memory and its pages
  count against the container's limit, plus 16 MiB for the container's init
  and the supervisor. A run killed for it ends as `memory` (Docker's
  `OOMKilled`). Processes: 64 in the container. The scratch directory is a
  `tmpfs` of `scratch_mb` and 10,000 files and directories: a hard bound, so
  a write past it fails with `ENOSPC`, and that holds for what no file's
  size shows, too (files unlinked but held open, mapped and closed,
  extended attributes, which take from the files' share, a KiB a file; on
  the scratch directory itself they're refused, as it isn't the code's).
  So a run's scratch use is at most `scratch_mb` plus 10 MiB of
  attributes. Output is read up to its limit, then the container is
  killed; so is one past its wall time, counted from when the code starts,
  as a container takes a second or more to start on a loaded machine.
  Killing a container ends everything in it, and a container a killed
  runner left behind is removed by the next runner's first run.
- **The supervisor** (`python/supervisor.py`), the container's first
  process under its init, copies the inputs in and starts the code, both as
  the code's user, then becomes a user of its own (65533), which the code
  can't signal, trace or read. It relays the code's output to the runner in
  frames the code can't forge, as the code never writes to the container's
  output itself, and it says the container's CPU time every 0.1 s (the
  grader's budget for each call) and, once the code has ended, whether the
  scratch directory was full (less than 64 KiB or 128 files left) at any
  check: a run that ended of itself, or by an error or a signal, with it
  full ends as `scratch`. That is all its word decides: it never widens a
  limit or marks a pass. A mapped page past the limit is `SIGBUS`, which
  ends the code at once and frees its pages, so no check may see the
  directory full: that run ends as the signal, with its write refused.

`run_tests` runs a tree's public tests there with `unittest`, and reports
each test's outcome (pass, fail, error or skip) and the last 16 KiB of their
output. It is feedback for the agent, not a grade: the tests share the
container with the code under test. `bracken-experiment run-tests --project
<dir> [--tree <dir>] --python <image>` runs it, on the seed by default.

**Not defended (finding `a1e72cb9c89d`, deferred to task `6202d01435a9`):**
the grader's server is the code's own process, so the code can reach the
server's state: its names through `__main__`, its sealed copy of them
through frames or `gc`, or the builtins it uses. That can forge anything the
server reports: a plain value, as any of its answers could, or the name it
sends for one of the code's exception classes or the package's modules, the
only values of the code's besides plain ones that compare, so that two of
them match as one (decision `0f7f0b4b048b`, finding `899e1616f407`). A proxy
stays out of its reach: it is never compared.

A run's plan records the image, by digest, and its Python's version
(`python`), and every run of agent code pins it from there again, refused
if the image isn't here by that digest or its version has changed
(`Python::from_plan`). A project run recorded before agent code ran in
containers pinned a host interpreter, which can't be pinned again, so it
can't be graded again: `grade-run` refuses its plan, saying so.

**Overhead** (measured for task `879a58e77bde` on this Mac's Docker
Desktop, at load averages of 150 to 370 from other agents' builds): a run
costs 0.6 s to 4.3 s on top of the code's own time, to make, start, inspect
and remove its container: a median of 2.3 s over ten runs at load 236
(`python::tests::macos_only::overhead`, run by hand). The grader starts one for each
group's host, one for its server (again after a crash) and one for each
command a test runs: while the host waits on one, its wall time counts only
the code's own time there, not the containers' starts and removals, and a
call's doesn't count its server's start. A command's own time ends when
the supervisor says it has ended, so its container's removal isn't counted
either. The group's first server start moves the host's deadline by however
long it takes, a cost of grading the group at all. What else is left out,
the code's server restarts and commands, moves the deadline by one more of
its wall time at most, so code that crashes its server on every call can't
stretch a group without end.

## Grading

The grader (`crates/experiment/src/project_grader.rs`, task
`9345098e82aa`) runs a project's hidden tests on a tree, and reports counts
per item and per group, by index: no test's name, source, expected value or
message. The tests and the code under test never share a process:
- **The host** runs one group's test file with `unittest`, in the Python
  profile, in a scratch directory holding the hidden set's `tests/` as read
  and digested, and nothing of the tree. It works in an empty directory of
  its own there, never the tests'. Importing the package gives a proxy:
  each attribute, call, iteration, comparison and so on is a line to the
  grader. Only the package and its submodules become proxy modules, only
  when the host imports them, and the host refuses a package whose name
  something else there already answers to (finding `734e5389e188`).
- **The server** answers them: the code under test, in the same profile, in
  another scratch directory with the tree. Values of exactly the plain
  types (see "Only plain values are compared" below) come back as data,
  decoded and compared by the host. Anything else, a subclass of a plain
  type too, comes back as a handle, a proxy that is never compared, nor
  hashed, nor ordered, not even with itself: each raises in the test
  (decision `d1fc04a4f896`), but for one case of Python's own, below. Its
  `in` the host answers from its items, so the code's own `__eq__` or
  `__contains__` never answers for a test. A
  server that dies is started again for the next call. Each start is a new
  generation, whose number tags its handles. A handle got as an attribute of
  a module of the package, such as a test file's `from pkg.mod import f`, is
  found again by name on the new server; any other of one that has ended is
  that test's error, never another object (findings `fe2549db89ee`,
  `5f5c3b4b25a9`). The server may run as long as the host, and each call as
  long, and use as much CPU, as one run of the project's code: a budget per
  call, not for the group.
- **An exception** comes back as its class, with its module, name and
  bases. The host raises one class per class of the code's, of that name,
  whose bases are the mirrors of its bases, so `issubclass` holds along the
  code's own hierarchy. A class goes by its name only if it is the object
  its module and name give in the server, and the first sent under that
  name; any other gets a class of its own, never Python's of that name nor
  another of the code's (decision `0f7f0b4b048b`): one class for the
  server's life, however often it comes, and the first such under its module
  and name carries across a restart by them, as a named class does (finding
  `90ae40b8ad52`). A class of Python's own, builtin or of the standard
  library, is raised as itself, but strictly: only from a module the host
  has already loaded, the name walked through classes only, and only a class
  whose own module and name are the ones claimed (finding `d1f7d7880644`).
  One whose constructor takes other arguments, such as
  `json.JSONDecodeError`, is made with the message as its arguments (finding
  `ed723694b473`). So `assertRaises` with the package's class or one of its
  bases, a builtin or a standard library class works, and so do checks of
  the class's name (answer `c473d137f5b7`, finding `621c185d3e6f`). Never as
  themselves: `StopIteration`, `StopAsyncIteration` and anything of
  `unittest`, which would stop the host's iterator or skip a test (finding
  `d077f100ac33`). The exception keeps its own attributes that are plain
  data or the package's objects, such as a parse error's line (task
  `6a80d88373a5`).
- **Operators and equality:** the code's own operators never answer a
  test's check of a value (finding `c823af485f19`). Arithmetic, reflected
  and unary operators on the code's objects are done by the code, and their
  result stays the code's, a proxy, even when it is a number there: so
  `assertAlmostEqual`, whose `round` of a proxy raises, can't be forged by a
  `__sub__` that says 0. Ordering a proxy raises, against anything.
  `float()`, `int()`, `round()` and `math`'s functions on
  a proxy raise, and its truth is Python's own (true) unless its class
  defines `__bool__` or `__len__`, when asking it raises: a test checks the
  plain values the code returns, such as an attribute's or a method's.
  **Only plain values are compared** (decision `d1fc04a4f896`, superseding
  `4990ee876e00`'s dataclass and named-tuple rule; findings `70e0656020fd`,
  `4df9d8089543`, `56250a88505b`). Every fact about one of the code's
  objects comes from the code's own process, so every structural rule could
  be lied to: the host compares only values of exactly the plain types, int,
  float, complex, bool, None, str, bytes, `Decimal`, `Fraction`, date, time,
  datetime and timedelta, and list, tuple, dict, set and frozenset of those,
  each type checked by identity (a subclass, of int or str too, is never
  plain). Such a value the code gives back, an attribute's, a method's,
  `str()`'s or `repr()`'s, comes to the host as data, and is compared there.
  Anything else stays the code's, a proxy, and equality, ordering or hashing
  with one raises in the test, never answering: against another of the
  code's objects, itself, or a plain value, or inside a plain container. The
  one exception for a proxy is Python's own (finding `c9626960bb86`): a
  list's or a tuple's comparison and `in` check identity before `__eq__`, so
  one and the same proxy object, held twice by a test, matches itself there:
  `[p] == [p]`, `(p,) == (p,)` and `p in [p]` are true. Every value got from
  the code is a new proxy, so two never match, not even two got from the
  same object: the code can't make a test pass this way with a proxy. Two
  kinds of the code's values aren't proxies, as `assertRaises`, `except` and
  `import` need them to be the host's own: its exception classes and the
  package's modules. The host keeps one of each for a name, a module only
  once this process has imported it (another is a new module each time it
  comes, finding `e611253cfb91`), and they compare by identity, so within
  one server one name stands for one object (decision `0f7f0b4b048b`): a
  class or a module goes by its name only if it is the object that name
  gives there, by its module and qualified name or by `sys.modules`, and the
  first sent under it. Otherwise a module is a proxy, and a class is one of
  its own, matching nothing else, one class for the server's life however
  often it comes. So two of them match only when they are one object in the
  server, with two residuals. A name carries across a server's restart, as a
  test file binds the code's classes when it imports them, so code that ends
  its own server between two fetches can have two objects, one in each
  generation, match as one. Of several classes under one module and
  qualified name, such as two a function made by being called twice, only
  the first sent in a generation carries: after a restart the first of them
  sent then is that class, whichever object it is, so a test's class of one
  may catch another, or not catch its own (finding `30eb693032b7`). Nothing
  can carry them better: a new server can't tell which of its objects an
  earlier one sent, nor sends them in the same order, which follows the
  test's calls. A correct solution meets this only when it makes two classes
  under one name and a test crosses a crash with them. And the server is the
  code's own process: code that rewrites the server's state, such as its
  record of names (`__main__`'s), can send any object under any name, as it
  could forge any plain value the server reports (finding `899e1616f407`;
  "Not defended" above). A builtin type is known by identity alone, among
  Python's own types taken before any of the code is imported, never by
  `in`, a hash or `==`, nor by a class's `__module__` (findings
  `98fb18bf3e9c`, `a2c5943deae7`). The server's functions run over a copy of
  its names, and of Python's builtins, taken then too, so code that rebinds
  one, such as `is_builtin` or `type`, changes nothing (finding
  `0b5b233009ef`).
  **The rule for a project's hidden tests:** compare attributes, return
  values or `str()`, never the code's objects: `assertEqual(entry.amount,
  Decimal("5"))`, not `assertEqual(entry, Entry(5))`. A hidden test that
  compares objects errors, even for a correct solution: the cost the owner
  accepted.
- **Introspection:** an object's `__module__`, `__name__`, `__qualname__`
  and `__doc__`, and `dir()` of it or of a module of the package, are the
  code's; `importlib.util.find_spec` finds a module of the package only if
  the tree has it. A mirrored exception's `str()` is the code's, and its
  fields come with it: its own, a slot's, a dataclass field's and those its
  classes declare or default (task `6a80d88373a5`). An exception a test
  makes of the code's class is made by the class's own `__init__` on the
  server and takes its arguments, attributes and `str()` from there. A
  submodule imported in the test is never bound on its package here, so a
  package attribute of the same name, such as a function its `__init__`
  imported from it, is the package's own, as in Python.
- **A text stream:** an `io.StringIO` given to the code, or standard output
  or error redirected to one around a call, is given as one of the
  server's holding the same text, and holds what the code wrote there once
  the call returns or raises; so a command line's in-process
  `main(argv, out=io.StringIO())` is tested as it would be run (task
  `6a80d88373a5`).
- **A path in a call:** a string or path-like argument that is an absolute
  path inside the host's own directory or its temporary one (in the
  profile, the same scratch directory), such as a temporary file's in an
  argv list, and not the tests' or one holding them, is given as one in a
  fresh directory of the server's, as a command's are; what the code wrote
  there goes back after the call, and the reply names the test's own path
  wherever it names the server's (task `6a80d88373a5`). Any other path,
  such as the store's, goes as a plain string, which the server's sandbox
  can't read. The host's temporary directory is one of its own, holding no
  test, so what the code writes to a test's `mkstemp` or
  `NamedTemporaryFile` comes back (finding `8221427759ca`). And the host
  never imports a module from its own directory, where everything the code
  can write lands, so no module the code planted runs there, whatever a
  test puts on `sys.path` (finding `8908d9699820`).
- **A command:** a test's `subprocess` runs only `python -m <package> ...`
  (`sys.executable` or a `python`, with harmless interpreter flags such as
  `-B`, `-I` or `-E` before `-m`; its environment is ignored, so nothing
  such as `PYTHONPATH` changes what runs), and not in the host. The grader runs it in
  a fresh child under the profile, with the tree, the working directory's
  files (at most 32 MiB) and the input, and gives back its exit code, its
  output and the working directory's files after it. A command's working
  directory is the host's empty one unless the test gives another, and the
  files it gives back are written only inside it, through no symlink
  (findings `7f3dc66a8589`, `f13d1ff05b5f`). One that is the tests'
  directory or holds it, as a helper's project root from `__file__` does,
  is given as an empty one, and nothing written there is kept. An argument
  that is an absolute path outside the tests, such as a temporary file's,
  is given inside the command's directory (a file's bytes, or a
  directory's files) and named so there; what the command writes there
  goes back to that path's directory, and the output says the test's own
  path again (task `6a80d88373a5`). So agent code never runs in the host's
  process and never sees a hidden file (note `9891289fb39c`).
- **The tests' own imports:** the hidden tests' directory is on the host's
  path, and is the package `tests` too: a group's file is the module
  `tests.<its path>`, so helpers beside the tests import by name, as
  `tests.<helper>` or relatively (task `6a80d88373a5`). A module of the
  package has a `__file__`, and a package its `__path__`: their places in
  the tree, as the server says them relative to it, in the host's own copy
  of the tree's layout, which holds its directories and no file. So a
  helper that finds the project from `ledger.__file__` gets a directory of
  its own, never the tree's real one or one holding the tests.
- **Limits:** the server and each command have the project's. The host has
  four times its CPU and wall time, 1 GiB and 16 MiB of output: it runs a
  whole file, waiting on each call. The host reports the number of tests,
  each outcome and each test's end as they happen, so a group whose host
  ends early keeps them: it is unfinished, with how it ended, and the tests
  that hadn't ended count as failures (finding `e7d128464936`). A group's
  grade also lists one outcome per test, in the suite's order, the worst of
  the test's own and its subtests': by index, never by name. The host names
  each test's index as it starts, so an error outside any test, such as a
  class fixture's, is the outcome of the tests it kept from running, never
  of the next test that ran (finding `bcecc7c3f6e3`).

A group applies at a share of the run only once its item is released, and a
before- or after-change group only on its side of its change. **The approval
check** grades `golden/` at the end of the run, where every group that
applies must pass, and, for a project with a change, `golden-before/` just
before the first change is released, where every group that applies then
must pass (answer `c473d137f5b7`). Both must, before a project's hidden set is
approved.

`bracken-experiment project grade <dir> --hidden <store> --python <image>
(--tree <dir> [--share <s>] | --golden) [--diagnose <file>] [--json]`
grades a tree, or runs the approval check. It prints counts per item only.
`--diagnose`, the manager's alone, writes to a new file, outside the
project, the store, any Ground and any checkout, one line per group with an
error outside any test (one that kept its tests from running, such as an
import's or a class fixture's) or a test that erred or failed: the
exception's class, message and frames, file:line, a hidden test's by its
path under `tests/`, and a test's index. It is agents' text, so its first
line says so, and each message is cut to 300 characters, a class to 100, a
frame's file and function to 200, with every character of Unicode's
categories Cc, Cf, Co, Cs, Cn, Zl and Zp (the tag characters included), and
the invisible ones of other categories (variation selectors, the combining
grapheme joiner, the Hangul and Braille blanks, and any whose name says
BLANK or FILLER), shown as an escape by the host's tables (findings
`052914fb0445`, `7df26ede5a8b`), and at most 50 errors of a group with the
count of all (finding `9c10a74d8c2a`). Nothing else ever holds it: not the
output, nor a grade (task `6a80d88373a5`).

## Running a project

As built so far (task `e4970250b3e4`, design note `43554a820efb`, answer
`07959d6d9bc0`): `bracken-experiment run --mode weave --project <dir>
--python <image> [--hold-change]`, and `mock-run`
the same way, runs a project's agents through **Weave mode**
(`src/weave_mode.rs`). `grade-run` grades the run after it ends.

- **The plan** records the project (its directory, a digest of all of it,
  each item's release share, its hidden reference and the hash of each text
  agents get), the pinned Python image, and the configuration's
  **integrator setting**: for now `any-agent`, every agent an integrator.
  The run refuses a project without `--python`, Weave mode without a
  project, and any other mode with one. Neither the plan nor anything else
  in the run holds the private store's path.
- **The hub** is a throwaway Weave Ground under the run directory, never
  `~/bracken-weave`. The driver reads the project and gives the hub its
  files, so no worker reads the project directory. The setter imports the
  seed as `main`'s first decision, so the seed is the run's first tree;
  states the spec and the items released at the start as claims; records
  the integrator setting as a decision claim; and, once every copy has a
  key, writes `main`'s policy naming the setter and every agent as
  integrators, citing that claim.
- **An agent's copy** is a replica of the hub's Ground, synced as Bracken
  mode syncs, beside a git working tree that only the store touches. Its
  tools: `task` (the spec and the items released so far: only claims the
  setter's key signs, the seed's author as the copy joined), `list_files`,
  `read_file`, `write_file`, `edit_file` and `delete_file` on its working
  tree, every path a plain relative one with no symlink on the way and none
  in `.git` or `.bracken`; `run_tests`; `changes`, its tree against the
  decision it started from, proposed or not; `head`, every head of the line;
  `update`, its tree replaced by a head, refused while it has edits not yet
  proposed; `propose`; `proposals`; `diff`; `integrate`, which merges
  proposals onto the head; `resolve`, which settles a conflict that stopped
  such a merge, with the line's side, the proposal's, the agent's own file
  or a deletion, or, given no proposals, a conflict of the agent's own
  `merge`, with its file as it stands (refused while it holds a conflict
  marker) or a deletion; and `merge`, on a forked line.
- **Recording a tree:** `integrate` given no proposals records the agent's
  tree, with every head as a parent, only if nothing would be lost (finding
  `9f4e48c4c547`): the tree started from the line's one head, or, on a
  forked line, it is the agent's `merge` of exactly the heads there now,
  with every conflict it reported settled by `resolve`, so one with no
  markers to find, such as a binary file's, never passes unsettled (finding
  `e26a1c74e007`). `merge` makes the tree every head merged by git's
  three-way merge, against where the heads parted, through any earlier
  merge: the store mirrors the line's decisions as git commits, once each,
  and puts the tree back as it was on any failure. A tree from a head that
  is no longer the line's reaches it as a proposal, which merges.
- **`run_tests`** runs the public tests on the agent's working tree, in the
  Python profile with the image pinned from the plan. The driver runs
  it, not the agent's worker, since a sandboxed process can't start another
  sandbox.
- **The change:** the items not released at the start go out together once
  their share of the run's agent-turns is used, rounded up after rounding
  the product to 1e-9, so 0.1 of 3 turns of 10 agents is 3 (finding
  `d8167e497c9d`) (owner decision
  `de365aae05fd`: at most one release after the start), and the hub states
  them as claims. The event records the agent-turns used, the share and the
  item ids. `--hold-change` holds them back: the plan records it, with no
  change point and no held item's text among what agents get; the hub is
  never given those items, and nothing is released.
- **Checkpoints:** after every turn of all agents, and at the end, the event
  log holds the line's head as files (on a forked line, its first head), and
  every head's decision and manifest id with whether the line is forked
  (finding `01e0ce2648ad`); so do snapshots and the final `integrated`
  event.
- **Grading:** `bracken-experiment grade-run --dir <run> --hidden <store>
  --grades <file> [--bracken <path>] [--diagnose <file>] [--json]` grades
  each checkpoint's tree, and the line at the end as `end`, with the hidden set
  (`src/grade_run.rs`), checking each out of the run's hub by the manifest
  id its event names (on a forked line, the first head's, and the grade says
  it was forked). It takes the project and the image from the plan,
  refusing a project changed since, and the store only from `--hidden`,
  which it checks against the project's digest. A tree is graded at share 1
  if the change had gone out before it, else at share 0, so the change's
  groups apply only once agents had it. Each tree is one line of the grades
  file: the checkpoint's turn or `end`, the share of turns done, whether the
  change was out, the decision and manifest graded, and its grade, groups
  and tests by index alone; or, for a tree that couldn't be graded, such as
  one whose checkout was refused, why, and every other tree is still graded
  (finding `aad6a4f0057b`). The grades file must be new, and outside the
  run, the project, the store, any Ground and any git checkout; it is
  written beside its name, by its resolved parent, and given its name only
  once every tree is graded, never replacing a file. No run reads it, and
  nothing records the store's path.

## Cost estimate

From Phase 1 and 2's measured use, and Candidate B's estimates for Python
code (whose files are several times larger than Phase 1's): an agent-turn
costs roughly **150,000 to 300,000 tokens** for git- or CRDT-like reading,
more for Bracken before phase1-bracken-lessons.md's fixes. Weave's own figure
is unknown until measured; the first runs measure it.

| Run | Agents × turns | Tokens per run (rough) |
|---|---|---|
| Pilot, rung 1 | 3 × 6 | 3 to 6 million |
| Small team, rung 2 | 5 × 8 | 6 to 12 million |
| Full team, rung 2 | 10 × 8 | 12 to 25 million, more if Weave reads heavily |

Phase 2's measured runs used 37.8, 40.0 and 34.3 million tokens for three
modes together, so one mode at a time is roughly a third of that. Turning tokens into money needs
the current price of the chosen model; that, and a monthly figure, is the
budget discussion still to have.

## Order

After Weave's build steps and `bracken watch` (decision `adf43778c85d`):
1. The evaluation runner: the experiment crate repurposed with Claude Code
   and Pi backends, the Python execution profile, and the grader, with their
   tests and security review.
2. One rung 1 project, with its spec, seed, hidden tests and golden solution.
3. A pilot run of one configuration, with the owner's go, then a look at the
   trace together.
4. One rung 2 project, written for the suite.
5. Decide the first metrics to optimise from what the pilot showed; then
   compare configurations, with replicates.

## Open questions

- **Budget:** per run and per month; decides how many replicates and
  projects.
- **The unit of work in Weave for Python:** file-level units are what Weave
  has today; whether that's fine-grained enough is something the pilot shows.
- **Human time:** how much of the owner's attention each look at a trace
  takes, and whether a summary view should come with the runner.
