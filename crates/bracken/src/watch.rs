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
//! source other than the local replica can feed the same output. Two do: the local replica ([`Local`]),
//! and with `--feed` a team server's change feed ([`Feed`]), whose replies are verified as a local read
//! verifies the replica, held in memory and never written to the replica.
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
    /// With --sync: wait on the server's change feed instead of syncing before each read, and print what
    /// arrives as it arrives. Everything it sends is verified as a local read verifies the replica, and
    /// kept in memory: the replica isn't written. --interval is then the wait after a failed request, or
    /// one that brought only what watch held already. A server without the feed is synced with instead,
    /// as --sync alone does.
    #[arg(long, requires = "sync")]
    feed: bool,
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
    let mut source: Box<dyn Source> = match sync {
        Some((url, token)) if a.feed => Box::new(Feed::new(s, url, token)),
        sync => Box::new(Local { store: s, sync, first: true }),
    };
    // The cursor last printed: each round ends with one if it moved, and the first always does.
    let mut printed: Option<String> = None;
    loop {
        let batch = source.next(&cursor)?;
        report(&batch, &filter, &mut cursor, a.out.json, &mut printed)?;
        if !a.follow {
            return Ok(ExitCode::SUCCESS);
        }
        if if source.pause() { !wait(a.interval) } else { reader_gone() } {
            return Ok(ExitCode::SUCCESS);
        }
        if cfg!(not(unix)) {
            printed = None; // the heartbeat: see reader_gone
        }
    }
}

/// What a [`Source`] hands over each round: a graph holding everything new, the replica whose identities
/// name authors, and what is new since the cursor, in causal order. Each new contribution's parents are
/// either seen by the cursor or earlier in `new`, so the cursor can move past them one at a time.
struct Batch<'a> {
    graph: &'a Graph,
    store: &'a Store,
    new: Vec<ContributionId>,
}

/// Where new contributions come from, kept apart from filtering and printing ([`report`]). A source only
/// says what is new since a cursor; the cursor is heads by full id, the same on every replica, so a
/// source elsewhere (a team server's change feed) can answer for it too. It must bring each new
/// contribution into the graph it hands over, since filters read the graph around it.
trait Source {
    fn next(&mut self, cursor: &Cursor) -> Result<Batch<'_>>;

    /// Whether `--follow` waits its `--interval` before the next round. A source that waits on its own
    /// (a long poll) says no, except to back off after a failure.
    fn pause(&self) -> bool {
        true
    }
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
            self.reread()?;
        }
        Ok(Batch { graph: self.store.graph(), store: &self.store, new: unseen(self.store.graph(), cursor) })
    }
}

impl Local {
    fn reread(&mut self) -> Result<()> {
        self.store = Store::open(self.store.dir())?;
        if let Some((url, token)) = &self.sync {
            let bounds = wire::Bounds { deadline: Duration::from_secs(30), ..wire::Bounds::default() };
            if let Err(e) = remote::sync_http(&mut self.store, url, token, bounds) {
                eprintln!("bracken: watch: not synced with {url} ({e:#}); reading the local replica");
            }
        }
        Ok(())
    }
}

/// What `g` holds that the cursor hasn't seen, in causal order.
fn unseen(g: &Graph, cursor: &Cursor) -> Vec<ContributionId> {
    g.topo_order().into_iter().filter(|id| !cursor.seen.contains(id)).collect()
}

/// How long a change feed request waits for something to arrive: the server's longest.
const FEED_WAIT: Duration = crate::serve::FEED_MAX_WAIT;

/// Replies in a row that fail verification before watch gives up.
const FEED_BAD_REPLIES: u32 = 3;

/// An empty reply that comes back sooner than this didn't wait as a long poll does: the next request
/// waits --interval, as it does after any reply of only what this side holds already, so no mismatch
/// can become a hot loop (findings 6552f9b16c65, d4f10edc63fb).
const FEED_QUICK: Duration = Duration::from_secs(1);

/// A team server's change feed (`bracken serve`'s `POST /v1/<ground>/changes`), long-polled. Its graph
/// starts as the local replica's, read once, and grows by what the feed sends, in memory only. Each reply
/// is checked as [`Store::open`] checks the replica: [`manyroot::bundle::read`] decodes each contribution
/// in canonical form and recomputes its id, and [`Graph::import`] checks the rest ([`Graph::check`]:
/// protocol, ground, size, signature, parents, references, body rules). A reply with anything rejected or
/// unresolved is dropped whole. If the server has no feed, this syncs as [`Local`] does, from then on.
struct Feed {
    local: Local,
    url: String,
    token: String,
    /// While the feed serves: everything verified so far. `None` once fallen back to syncing.
    graph: Option<Graph>,
    /// What this graph holds that the server is known to hold, because a reply brought it, with all its
    /// ancestors. It grows with each reply (finding d4f10edc63fb), except the first of a run of replies of
    /// only what is here already: the server lacked every head sent, so it may hold less than this said
    /// (restored from a backup, or re-created), and that reply's own contributions replace it (finding
    /// 2e421e838287). The replies after it in the run are the next pages of the same catch-up, so they
    /// add to it: replacing it on each could drop an earlier page, and paging would cycle (finding
    /// ab41112b47f2).
    held: HashSet<ContributionId>,
    /// The last reply brought only what is here already.
    resending: bool,
    first: bool,
    /// The last request failed, or came back at once with nothing new, so the next waits --interval.
    failed: bool,
    bad_replies: u32,
}

impl Feed {
    fn new(store: Store, url: String, token: String) -> Feed {
        let graph = Some(store.graph().clone());
        let local = Local { store, sync: Some((url.clone(), token.clone())), first: false };
        let held = HashSet::new();
        Feed { local, url, token, graph, held, resending: false, first: true, failed: false, bad_replies: 0 }
    }

    /// Records that the server holds `id`, and so everything before it.
    fn hold(&mut self, g: &Graph, id: ContributionId) {
        let mut stack = vec![id];
        while let Some(id) = stack.pop() {
            if self.held.insert(id) {
                stack.extend(g.get(&id).expect("graph ids and their parents exist").parents().iter().copied());
            }
        }
    }

    /// The cursor to send: `g`'s heads, and the heads of what the server is known to hold. The server
    /// leaves out heads it lacks, so the second set keeps it from sending again what is here already,
    /// however long a chain of this side's work it lacks. The first lets it say, in each reply, which
    /// heads it still lacks: once it has them, they count as seen.
    fn since(&self, g: &Graph) -> String {
        let parents: HashSet<ContributionId> = self
            .held
            .iter()
            .flat_map(|id| g.get(id).expect("held ids are the graph's").parents().iter().copied())
            .collect();
        let heads: BTreeSet<String> = self
            .held
            .iter()
            .filter(|id| !parents.contains(id))
            .copied()
            .chain(g.heads())
            .map(|id| id.to_hex())
            .collect();
        heads.into_iter().collect::<Vec<_>>().join(",")
    }

    /// One request, verified into the graph. A failure is said on stderr, and only verification failing
    /// [`FEED_BAD_REPLIES`] times in a row is an error.
    fn poll(&mut self) -> Result<()> {
        let g = self.graph.as_ref().expect("polled only while the feed serves");
        let since = self.since(g);
        let asked = Instant::now();
        let bundle = match remote::changes(&self.url, &self.token, g.ground(), &since, FEED_WAIT) {
            Ok(remote::Changes::Reply { bundle }) => bundle,
            Ok(remote::Changes::Unsupported(status)) => {
                eprintln!("bracken: watch: {} has no change feed ({status}); syncing with it instead", self.url);
                self.graph = None;
                return Ok(());
            }
            Err(e) => {
                eprintln!("bracken: watch: the change feed of {} failed ({e:#}); trying again", self.url);
                self.failed = true;
                return Ok(());
            }
        };
        match verified(g, &bundle) {
            Ok((next, brought)) => {
                // Nothing new: either a server sending what is here already (a cursor whose every head it
                // lacked), or an empty reply that didn't wait.
                let resent = next.len() == g.len() && !brought.is_empty();
                let idle = resent || (next.len() == g.len() && asked.elapsed() < FEED_QUICK);
                if resent && !self.resending {
                    self.held.clear();
                }
                self.resending = resent;
                for id in brought {
                    self.hold(&next, id);
                }
                (self.graph, self.failed, self.bad_replies) = (Some(next), idle, 0);
                Ok(())
            }
            Err(e) => {
                self.bad_replies += 1;
                ensure!(
                    self.bad_replies < FEED_BAD_REPLIES,
                    "the change feed of {} sent {FEED_BAD_REPLIES} replies in a row that failed verification; the last: {e:#}",
                    self.url
                );
                eprintln!(
                    "bracken: watch: dropped a change feed reply from {} that failed verification: {e:#}",
                    self.url
                );
                self.failed = true;
                Ok(())
            }
        }
    }
}

/// `g` with a change feed reply's contributions added, and the ids of all it brought; or why the reply
/// fails verification, in which case none of it counts.
fn verified(g: &Graph, bundle: &[u8]) -> Result<(Graph, Vec<ContributionId>)> {
    let (ground, contributions) = manyroot::bundle::read(bundle).context("decoding the reply")?;
    ensure!(ground == g.ground(), "the reply is of another ground, {ground}");
    let brought = contributions.iter().map(Contribution::id).collect();
    let mut next = g.clone();
    let report = next.import(contributions);
    if let Some((id, why)) = report.rejected.first() {
        anyhow::bail!("{} of its contributions rejected, the first {id}: {why}", report.rejected.len());
    }
    if let Some(id) = report.unresolved.first() {
        anyhow::bail!("{} of its contributions lack a parent, the first {id}", report.unresolved.len());
    }
    Ok((next, brought))
}

impl Source for Feed {
    fn next(&mut self, cursor: &Cursor) -> Result<Batch<'_>> {
        // The first round reads the replica as it was opened, as Local's does.
        if !std::mem::take(&mut self.first) {
            if self.graph.is_some() {
                self.poll()?;
            }
            if self.graph.is_none() {
                self.local.reread()?;
            }
        }
        let g = self.graph.as_ref().unwrap_or(self.local.store.graph());
        Ok(Batch { graph: g, store: &self.local.store, new: unseen(g, cursor) })
    }

    fn pause(&self) -> bool {
        self.failed || self.graph.is_none()
    }
}

/// Prints what the batch brings that the filter admits, each with the cursor past it, then the cursor past
/// everything if it moved since the last one printed (or none was). Without --open that is each new
/// contribution, in its place. With --open it is each finding the batch made open, old or new, at the
/// batch's end ([`newly_open`]).
fn report(batch: &Batch, filter: &Filter, cursor: &mut Cursor, json: bool, printed: &mut Option<String>) -> Result<()> {
    let g = batch.graph;
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
