//! `bracken watch`: the new contributions since a cursor, filtered, once or as they arrive
//! (docs/design/agents.md, `bracken watch`).
//!
//! The cursor is `log --since`'s: causal heads, and what it has seen is those heads and all their
//! ancestors. A replica only ever holds a contribution together with its ancestors, so one that arrives
//! later through sync was never an ancestor of a cursor printed before it: it is past every such cursor,
//! however old its timestamp or wherever it sits in causal order, and is reported once. It names
//! contributions by full id, so it means the same on any replica that holds them; callers treat it as
//! opaque and pass back what was printed.
//!
//! Where new contributions come from (a [`Source`]) is kept apart from filtering and printing, so a
//! source other than the local replica can feed the same output.
//!
//! The output is other agents' text. Lines are escaped as `log`'s are, and a harness should treat
//! them as untrusted and check a contribution with `show` before acting on it.

use std::collections::{BTreeSet, HashSet};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use clap::Args;
use manyroot::{AgentId, Body, Contribution, ContributionId, Graph, Policy, RelationKind, Store};
use serde_json::json;

use super::{CurrentView, Names, Output, compact_line, contribution_json, listing, now_ms, open_finding, queue};
use super::{remote, status_rows, wire, write_stdout};

/// The shortest `--interval`: how often `--follow` may re-read the replica, and pull with `--sync`.
const MIN_INTERVAL: Duration = Duration::from_secs(2);

/// The longest `--interval`, a day: far beyond any useful wait, and nowhere near overflowing a clock.
const MAX_INTERVAL: Duration = Duration::from_secs(86_400);

/// How often a `--follow` that is waiting checks whether its reader has gone.
const READER_CHECK: Duration = Duration::from_millis(250);

#[derive(Args)]
pub(crate) struct WatchArgs {
    /// Only claims with this tag.
    #[arg(long)]
    tag: Option<String>,
    /// Only findings as they become open, as `log --open` decides it: each finding is printed when what
    /// arrives makes it open (the finding, its contradicts relation, or a triage change), even if the
    /// finding itself arrived earlier. Reopened after it closed, it is printed again. The findings one read
    /// opens come after everything it read, all with the cursor past it.
    #[arg(long)]
    open: bool,
    /// With --open: the identity whose evidence triages findings.
    #[arg(long, value_name = "NAME", default_value = "manager", requires = "open")]
    triager: String,
    /// Only what concerns the --as identity, written by someone else: a contradicts, refines or
    /// depends_on relation to one of its contributions, and work waiting for its pass, as `status --as`
    /// orders it (the work, or an implements relation from it).
    #[arg(long)]
    for_me: bool,
    /// Only what arrived after this cursor: pass back, unchanged, a cursor watch or `log --since` printed
    /// ("" for everything). Without it, watch starts at the replica's heads, so only what arrives from now on.
    /// A cursor means the same only under the same filters: new work the filters hid is past it all the same.
    #[arg(long, value_name = "CURSOR")]
    since: Option<String>,
    /// Keep watching: re-read the replica every --interval and print what's new, until stopped or until
    /// stdout's reader goes away (then exit 0).
    #[arg(long)]
    follow: bool,
    /// With --follow: how long to wait between reads, from 2s to 1 day.
    #[arg(
        long,
        value_name = "DURATION",
        default_value = "5s",
        value_parser = humantime::parse_duration,
        requires = "follow"
    )]
    interval: Duration,
    /// With --follow: before each read, sync with this server (http://127.0.0.1:<port>), as `sync` does. A
    /// failed sync is said on stderr, and that read is of the local replica.
    #[arg(long, value_name = "URL", requires = "follow", requires = "token_file")]
    sync: Option<String>,
    /// With --sync: read the bearer token from this file.
    #[arg(long, value_name = "PATH", requires = "sync")]
    token_file: Option<PathBuf>,
    #[command(flatten)]
    out: Output,
}

/// Which new contributions are printed.
struct Filter {
    tag: Option<String>,
    /// The triager, with --open.
    open: Option<manyroot::AgentId>,
    /// The caller, with --for-me.
    me: Option<manyroot::AgentId>,
}

/// Where the watch is: everything the cursor has seen, and its heads.
struct Cursor {
    seen: HashSet<ContributionId>,
    heads: BTreeSet<ContributionId>,
}

impl Cursor {
    fn new(g: &Graph, since: Option<&str>) -> Result<Cursor> {
        let seen = match since {
            Some(c) => listing::seen(g, c)?,
            None => g.iter().map(|c| c.id()).collect(),
        };
        let parents: HashSet<ContributionId> =
            seen.iter().flat_map(|id| g.get(id).expect("seen ids exist").parents().iter().copied()).collect();
        let heads = seen.iter().filter(|id| !parents.contains(id)).copied().collect();
        Ok(Cursor { seen, heads })
    }

    /// Moves past `c`, whose parents it has seen already.
    fn advance(&mut self, c: &Contribution) {
        self.seen.insert(c.id());
        for p in c.parents() {
            self.heads.remove(p);
        }
        self.heads.insert(c.id());
    }

    /// As `log --since` prints it: sorted, comma-separated full ids.
    fn text(&self) -> String {
        let mut heads: Vec<String> = self.heads.iter().map(ContributionId::to_hex).collect();
        heads.sort();
        heads.join(",")
    }
}

pub(crate) fn run(s: Store, agent: Option<&str>, a: WatchArgs) -> Result<ExitCode> {
    ensure!(
        a.interval >= MIN_INTERVAL,
        "--interval {} is under the minimum, {}",
        humantime::format_duration(a.interval),
        humantime::format_duration(MIN_INTERVAL)
    );
    ensure!(
        a.interval <= MAX_INTERVAL,
        "--interval {} is over the maximum, {}",
        humantime::format_duration(a.interval),
        humantime::format_duration(MAX_INTERVAL)
    );
    let sync = match (&a.sync, &a.token_file) {
        (Some(url), Some(path)) => {
            let token = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
            Some((url.clone(), token.trim().to_owned()))
        }
        _ => None,
    };
    let names = Names::load(&s)?;
    let me = if a.for_me {
        let who = agent.context("--for-me: who is me? pass --as <NAME> or set BRACKEN_AGENT")?;
        Some(names.resolve(who).with_context(|| format!("--for-me: no identity {who}"))?)
    } else {
        None
    };
    let open = if a.open {
        Some(names.resolve(&a.triager).with_context(|| format!("--open: who triages? no identity {}", a.triager))?)
    } else {
        None
    };
    let filter = Filter { tag: a.tag.clone(), open, me };
    let mut cursor = Cursor::new(s.graph(), a.since.as_deref())?;
    let mut source = Local { store: s, sync, first: true };
    // The cursor last printed: each round ends with one if it moved, and the first always does.
    let mut printed: Option<String> = None;
    loop {
        let batch = source.next(&cursor)?;
        report(&batch, &filter, &mut cursor, a.out.json, &mut printed)?;
        if !a.follow {
            return Ok(ExitCode::SUCCESS);
        }
        if !wait(a.interval) {
            return Ok(ExitCode::SUCCESS);
        }
        if cfg!(not(unix)) {
            printed = None; // the heartbeat: see reader_gone
        }
    }
}

/// What a [`Source`] hands over each round: a replica holding everything new, and what is new since the
/// cursor, in causal order. Each new contribution's parents are either seen by the cursor or earlier in
/// `new`, so the cursor can move past them one at a time.
struct Batch<'a> {
    store: &'a Store,
    new: Vec<ContributionId>,
}

/// Where new contributions come from, kept apart from filtering and printing ([`report`]). A source only
/// says what is new since a cursor; the cursor is heads by full id, the same on every replica, so a
/// source elsewhere (a team server's change feed) can answer for it too. It must bring each new
/// contribution into the replica it hands over, since filters read the graph around it.
trait Source {
    fn next(&mut self, cursor: &Cursor) -> Result<Batch<'_>>;
}

/// The local replica: as opened for the first round, then re-read for each one after, synced first with
/// the server in `sync` (URL and token) if given.
struct Local {
    store: Store,
    sync: Option<(String, String)>,
    first: bool,
}

impl Source for Local {
    fn next(&mut self, cursor: &Cursor) -> Result<Batch<'_>> {
        if !std::mem::take(&mut self.first) {
            self.store = Store::open(self.store.dir())?;
            if let Some((url, token)) = &self.sync {
                let bounds = wire::Bounds { deadline: Duration::from_secs(30), ..wire::Bounds::default() };
                if let Err(e) = remote::sync_http(&mut self.store, url, token, bounds) {
                    eprintln!("bracken: watch: not synced with {url} ({e:#}); reading the local replica");
                }
            }
        }
        let new = self.store.graph().topo_order().into_iter().filter(|id| !cursor.seen.contains(id)).collect();
        Ok(Batch { store: &self.store, new })
    }
}

/// Prints what the batch brings that the filter admits, each with the cursor past it, then the cursor past
/// everything if it moved since the last one printed (or none was). Without --open that is each new
/// contribution, in its place. With --open it is each finding the batch made open, old or new, at the
/// batch's end ([`newly_open`]).
fn report(batch: &Batch, filter: &Filter, cursor: &mut Cursor, json: bool, printed: &mut Option<String>) -> Result<()> {
    let g = batch.store.graph();
    if !batch.new.is_empty() {
        for id in &batch.new {
            ensure!(g.contains(id), "a source handed over {id}, which its replica doesn't hold");
        }
        let names = Names::load(batch.store)?;
        let waiting = filter.me.map(|me| waiting_for(g, me));
        let prefixes = listing::unique_prefixes(g.iter().map(|c| c.id()), listing::COMPACT_ID_DIGITS);
        // What to print after moving past each new contribution.
        let mut due: Vec<Vec<ContributionId>> = vec![Vec::new(); batch.new.len()];
        match filter.open {
            // At the end: what makes a finding open can be anywhere in the batch (its contradicts, a triage,
            // a retraction that makes the contradicted contribution current again), so only the cursor past
            // all of it is one from which resuming doesn't report the finding again (finding 408881286395).
            // Findings opened by one batch share that cursor.
            Some(triager) => {
                *due.last_mut().expect("the batch isn't empty") = newly_open(g, &cursor.seen, triager);
            }
            None => due.iter_mut().zip(&batch.new).for_each(|(d, id)| d.push(*id)),
        }
        for (id, due) in batch.new.iter().zip(due) {
            cursor.advance(g.get(id).expect("checked above"));
            for c in due.iter().map(|d| g.get(d).expect("graph ids exist")) {
                let tagged = filter
                    .tag
                    .as_ref()
                    .is_none_or(|t| matches!(c.body(), Body::Claim { tags, .. } if tags.contains(t)));
                let mine = filter.me.is_none_or(|me| for_me(g, me, waiting.as_ref().expect("--for-me lists it"), c));
                if !(tagged && mine) {
                    continue;
                }
                // Only a JSON line carries its cursor; the text form has it on the round's last line.
                if json {
                    let at = cursor.text();
                    let line = json!({"contribution": contribution_json(g, &names, c), "cursor": at});
                    write_stdout(format!("{line}\n").as_bytes());
                    *printed = Some(at);
                } else {
                    write_stdout(format!("{}\n", compact_line(g, &names, &prefixes, c)).as_bytes());
                    *printed = None;
                }
            }
        }
    }
    let at = cursor.text();
    if printed.as_deref() != Some(at.as_str()) {
        let line = if json { json!({"cursor": at}).to_string() } else { format!("cursor: {at}") };
        write_stdout(format!("{line}\n").as_bytes());
        *printed = Some(at);
    }
    Ok(())
}

/// The findings a batch made open, in causal order, as `log --open` decides it: open in `g` and not in the
/// replica the cursor had seen (`seen`, which holds every ancestor of what it holds). A finding is open only
/// once its contradicts relation exists, often written in a later round, and stays open through a
/// triager's confirming pass, so its own arrival is neither the only moment it opens nor a second one.
/// Reopened after it closed (a triager's fail, then a pass), it is reported again.
fn newly_open(g: &Graph, seen: &HashSet<ContributionId>, triager: AgentId) -> Vec<ContributionId> {
    let order = g.topo_order();
    let view = CurrentView::new(g, &order);
    let after: Vec<ContributionId> = order
        .iter()
        .filter(|id| open_finding(g, &view, triager, g.get(id).expect("ordered ids exist")))
        .copied()
        .collect();
    if after.is_empty() {
        return Vec::new();
    }
    let mut before = Graph::new(g.ground());
    before.import(order.iter().filter(|id| seen.contains(id)).map(|id| g.get(id).expect("ordered ids exist").clone()));
    let before_view = CurrentView::new(&before, &before.topo_order());
    after
        .into_iter()
        .filter(|f| !before.get(f).is_some_and(|b| open_finding(&before, &before_view, triager, b)))
        .collect()
}

/// The work waiting for `me`'s pass, as `status --as` lists it under the default policy.
fn waiting_for(g: &Graph, me: manyroot::AgentId) -> HashSet<ContributionId> {
    let (rows, _) = status_rows(g, &Policy::default(), false, now_ms());
    queue(g, &rows, Some(me)).for_your_pass.iter().map(|w| w.work.id()).collect()
}

/// Whether `c`, someone else's, concerns `me`: it contradicts, refines or depends on `me`'s
/// contribution, or it is work waiting for `me`'s pass, or the implements relation from such work.
fn for_me(g: &Graph, me: manyroot::AgentId, waiting: &HashSet<ContributionId>, c: &Contribution) -> bool {
    if c.author() == me {
        return false;
    }
    match c.body() {
        Body::Relation {
            kind: RelationKind::Contradicts | RelationKind::Refines | RelationKind::DependsOn,
            to,
            ..
        } => g.get(to).is_some_and(|t| t.author() == me),
        Body::Relation { from, kind: RelationKind::Implements, .. } => waiting.contains(from),
        Body::Claim { .. } | Body::Artifact { .. } => waiting.contains(&c.id()),
        _ => false,
    }
}

/// Waits `interval`; false if stdout's reader went away meanwhile.
fn wait(interval: Duration) -> bool {
    let end = Instant::now() + interval;
    loop {
        if reader_gone() {
            return false;
        }
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return true;
        }
        std::thread::sleep(left.min(READER_CHECK));
    }
}

/// Whether stdout is a pipe or socket whose reader has gone. Asks for writability too: macOS reports the
/// hang-up only then. Linux says ERR for a pipe, macOS HUP; NVAL means stdout was closed outright.
#[cfg(unix)]
fn reader_gone() -> bool {
    use rustix::event::{PollFd, PollFlags, Timespec, poll};
    let stdout = std::io::stdout();
    let mut fds = [PollFd::new(&stdout, PollFlags::OUT)];
    let now = Timespec { tv_sec: 0, tv_nsec: 0 };
    let gone = PollFlags::ERR | PollFlags::HUP | PollFlags::NVAL;
    poll(&mut fds, Some(&now)).is_ok() && fds[0].revents().intersects(gone)
}

/// Elsewhere it can't tell without writing, so each `--follow` round prints its cursor line (a heartbeat),
/// and the write that fails ends it.
#[cfg(not(unix))]
fn reader_gone() -> bool {
    false
}
