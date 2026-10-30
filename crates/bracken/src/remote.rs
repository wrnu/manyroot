//! `bracken sync <url>` and `bracken sync --exec`: this replica's side of a session with a
//! server, over HTTP on the loopback interface or over a program's stdin and stdout.
//!
//! The client holds the server to the same [`Bounds`] the server holds it to.

use std::io::{self, Cursor, Read, Write};
use std::net::TcpStream;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use manyroot::sync::Rejected;
use manyroot::{Contribution, Graph, GroundId, ImportReport, Store};

use crate::serve::{parse_length, read_head_bytes};
use crate::wire::{Bounds, Refused, Replica, Side, Timed, run_stream, write_frames};

/// This replica, importing everything a session brings.
struct Local<'a>(&'a mut Store);

impl Replica for Local<'_> {
    fn graph(&self) -> &Graph {
        self.0.graph()
    }

    fn import(&mut self, batch: Vec<Contribution>) -> Result<(ImportReport, Vec<Rejected>)> {
        Ok((self.0.import(batch)?, Vec::new()))
    }
}

fn side(store: &Store, bounds: Bounds) -> Side {
    Side::open(store.info().ground, bounds)
}

/// The host and port of an `http://` URL, which must name the loopback interface: there's no
/// TLS, so a token sent anywhere else would cross the network in the clear.
fn loopback(url: &str) -> Result<(String, u16)> {
    let rest = url.strip_prefix("http://").context("the URL must start with http://")?;
    let authority = rest.strip_suffix('/').unwrap_or(rest);
    if authority.contains('/') || authority.contains('@') {
        bail!("give the server's URL only, http://127.0.0.1:<port>");
    }
    let (host, port) = authority.rsplit_once(':').context("the URL needs a port")?;
    let port: u16 = port.parse().context("bad port in the URL")?;
    if !["127.0.0.1", "localhost", "[::1]"].contains(&host) {
        bail!("only loopback servers (127.0.0.1, localhost, [::1]) are supported: there is no TLS yet");
    }
    // Connect to the address itself, never to whatever a name resolves to.
    let address = if host == "[::1]" { "::1" } else { "127.0.0.1" };
    Ok((address.to_owned(), port))
}

/// Syncs with a server over HTTP. Returns the side, with what each end imported.
pub fn sync_http(store: &mut Store, url: &str, token: &str, bounds: Bounds) -> Result<Side> {
    let (host, port) = loopback(url)?;
    if token.is_empty() || token.bytes().any(|b| b.is_ascii_control()) {
        bail!("the token is empty or holds control characters");
    }
    let mut side = side(store, bounds);
    let base = format!("/v1/{}/sessions", store.info().ground);
    let mut session: Option<String> = None;
    let mut out = side.session.start(store.graph());
    while !out.is_empty() {
        let path = session.as_ref().map_or_else(|| base.clone(), |id| format!("{base}/{id}"));
        let mut body = Vec::new();
        write_frames(&mut body, &out)?;
        let tcp = TcpStream::connect((host.as_str(), port)).with_context(|| format!("connecting to {url}"))?;
        side.time_left()?;
        // Every read and write of the round ends at the session's deadline, however slowly the
        // server trickles.
        let mut stream = Timed::new(&tcp, side.deadline(), None);
        let head = format!(
            "POST {path} HTTP/1.1\r\nHost: localhost:{port}\r\nAuthorization: Bearer {token}\r\n\
             Content-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(head.as_bytes())?;
        stream.write_all(&body)?;
        stream.flush()?;
        let Response { status, session: id, length, early, .. } = match read_response_head(&mut stream) {
            Err(_) if side.time_left().is_err() => return Err(Refused::deadline().into()),
            other => other?,
        };
        let mut reader = Cursor::new(early).chain(&mut stream).take(length);
        if status != 200 {
            let mut text = Vec::new();
            reader.take(1024).read_to_end(&mut text).ok();
            bail!("the server refused the session ({status}): {}", String::from_utf8_lossy(&text).trim());
        }
        if session.is_none() {
            session = Some(id.context("the server opened no session")?);
        }
        if length > side.budget_left() {
            return Err(Refused::new(413, "the server's response is over what's left of the session's budget").into());
        }
        out = Vec::new();
        while let Some(frame) = side.read_frame(&mut reader)? {
            out.extend(side.handle(&mut Local(store), &frame)?);
        }
        if reader.limit() > 0 {
            bail!("the server's response is shorter than its Content-Length");
        }
    }
    if !side.session.is_done() {
        bail!("the server stopped before the session finished");
    }
    Ok(side)
}

/// A server's response head, and the body bytes read with it.
struct Response {
    status: u16,
    /// X-Bracken-Session.
    session: Option<String>,
    length: u64,
    early: Vec<u8>,
}

fn read_response_head(stream: &mut impl Read) -> Result<Response> {
    let (bytes, early) = read_head_bytes(stream).map_err(|r| anyhow::anyhow!("reading the server's response: {r}"))?;
    let mut headers = [httparse::EMPTY_HEADER; 32];
    let mut response = httparse::Response::new(&mut headers);
    match response.parse(&bytes) {
        Ok(httparse::Status::Complete(_)) => {}
        _ => bail!("the server's response isn't HTTP"),
    }
    let mut id = None;
    let mut length = None;
    for h in response.headers.iter() {
        if h.name.eq_ignore_ascii_case("content-length") {
            length = Some(parse_length(h.value).context("bad Content-Length from the server")?);
        }
        if h.name.eq_ignore_ascii_case("x-bracken-session") {
            let v =
                std::str::from_utf8(h.value).ok().filter(|v| v.len() == 32 && v.bytes().all(|b| b.is_ascii_hexdigit()));
            id = Some(v.context("bad session id from the server")?.to_owned());
        }
        if h.name.eq_ignore_ascii_case("transfer-encoding") {
            bail!("the server sent Transfer-Encoding, which isn't supported");
        }
    }
    let status = response.code.context("the server's response has no status")?;
    let length = length.context("the server's response has no Content-Length")?;
    Ok(Response { status, session: id, length, early })
}

/// The largest change feed reply this side reads: the server's 8 MiB, past which it still sends one
/// contribution, of at most 16 MiB.
const FEED_REPLY_LIMIT: u64 = 32 << 20;

/// What a change feed request brought.
pub enum Changes {
    /// The server has no change feed: it answered the request with this status.
    Unsupported(u16),
    Reply {
        /// A bundle, unverified. The caller asks again at once whether or not the server said it has more
        /// (X-Bracken-More), so a reply's size doesn't matter to it.
        bundle: Vec<u8>,
    },
}

/// Asks a server's change feed for what `since` hasn't seen, waiting up to `wait` for something to arrive
/// (task 09bf8248e69b). What comes back is unverified: the caller checks it as a local read does.
pub fn changes(url: &str, token: &str, ground: GroundId, since: &str, wait: Duration) -> Result<Changes> {
    let (host, port) = loopback(url)?;
    if token.is_empty() || token.bytes().any(|b| b.is_ascii_control()) {
        bail!("the token is empty or holds control characters");
    }
    let body = serde_json::json!({"since": since, "wait_ms": wait.as_millis() as u64}).to_string();
    let tcp = TcpStream::connect((host.as_str(), port)).with_context(|| format!("connecting to {url}"))?;
    // The server answers within the wait, and writes its reply within its write timeout.
    let mut stream = Timed::new(&tcp, Instant::now() + wait + Duration::from_secs(30), None);
    let head = format!(
        "POST /v1/{ground}/changes HTTP/1.1\r\nHost: localhost:{port}\r\nAuthorization: Bearer {token}\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    stream.flush()?;
    let Response { status, length, early, .. } = read_response_head(&mut stream)?;
    let mut reader = Cursor::new(early).chain(&mut stream).take(length);
    if matches!(status, 404 | 405 | 501) {
        return Ok(Changes::Unsupported(status));
    }
    if status != 200 {
        let mut text = Vec::new();
        reader.take(1024).read_to_end(&mut text).ok();
        bail!("the server refused the change feed request ({status}): {}", String::from_utf8_lossy(&text).trim());
    }
    if length > FEED_REPLY_LIMIT {
        bail!("the server's change feed reply is {length} bytes, over the {FEED_REPLY_LIMIT} this side reads");
    }
    let mut bundle = Vec::new();
    reader.read_to_end(&mut bundle).context("reading the change feed reply")?;
    if bundle.len() as u64 != length {
        bail!("the server's response is shorter than its Content-Length");
    }
    Ok(Changes::Reply { bundle })
}

/// Syncs with a server reached through a program's stdin and stdout (`ssh host bracken serve
/// --stdio`), run without a shell and without this side's token or replica in its environment.
/// The program stays in this process's group, so it can still prompt on the terminal (ssh's
/// password and host-key questions). Every read and write of the session ends at the deadline,
/// whatever holds the pipes: they run on threads this side stops waiting for, so a descendant
/// that keeps a pipe open can't keep the client waiting. At the deadline, or soon after the
/// session ends if the program doesn't exit, the program itself is killed; its descendants
/// aren't, and may outlive the client. The program's stderr is relayed through a pipe, so a
/// descendant left alive doesn't hold the caller's stderr either.
pub fn sync_exec(store: &mut Store, program: &[String], bounds: Bounds) -> Result<Side> {
    let (name, args) = program.split_first().context("--exec needs a program")?;
    let mut child = Command::new(name)
        .args(args)
        .env_remove("BRACKEN_TOKEN")
        .env_remove("BRACKEN_DIR")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("running {name}"))?;
    let mut side = side(store, bounds);
    let deadline = side.deadline();
    let relayed = relay_stderr(child.stderr.take().expect("stderr is piped"));
    let mut input = Piped::writer(child.stdin.take().expect("stdin is piped"));
    let mut output = Piped::reader(child.stdout.take().expect("stdout is piped"), deadline);
    let opening = side.session.start(store.graph());
    let result =
        run_stream(&mut side, opening, &mut output, &mut input, |side, frame| side.handle(&mut Local(store), frame));
    // The writer thread closes the program's stdin once the queued writes have gone.
    drop(input);
    // The session is over either way: give the program a moment to exit, within the deadline.
    let until = deadline.min(Instant::now() + EXIT_GRACE);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if Instant::now() >= until {
            let _ = child.kill();
            child.wait()?;
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    // Let the last of the program's messages through, but don't wait for a descendant that
    // still holds its stderr.
    let _ = relayed.recv_timeout(deadline.saturating_duration_since(Instant::now()).min(STDERR_GRACE));
    let timed_out = Instant::now() >= deadline;
    match (result, status) {
        (Err(_), _) | (Ok(()), None) if timed_out => Err(Refused::deadline().into()),
        (Err(e), _) => Err(e),
        (Ok(()), None) => bail!("{name} didn't exit after the session ended; it was killed"),
        (Ok(()), Some(status)) if !status.success() => bail!("{name} exited with {status}"),
        (Ok(()), Some(_)) => Ok(side),
    }
}

/// One end of a pipe to `--exec`'s program, through a thread, so this side never blocks on the
/// pipe itself: reads wait on a channel with the time left to the deadline, and writes are queued.
/// The thread may stay blocked on the pipe after this side gives up; it ends with the process.
enum Piped {
    Reader { chunks: mpsc::Receiver<io::Result<Vec<u8>>>, buffer: Cursor<Vec<u8>>, deadline: Instant },
    Writer { chunks: mpsc::Sender<Vec<u8>> },
}

impl Piped {
    fn reader(mut pipe: impl Read + Send + 'static, deadline: Instant) -> Piped {
        let (send, chunks) = mpsc::sync_channel(4);
        std::thread::spawn(move || {
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let chunk = match pipe.read(&mut buf) {
                    Ok(n) => Ok(buf[..n].to_vec()),
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => Err(e),
                };
                let end = !matches!(&chunk, Ok(c) if !c.is_empty());
                if send.send(chunk).is_err() || end {
                    return;
                }
            }
        });
        Piped::Reader { chunks, buffer: Cursor::new(Vec::new()), deadline }
    }

    fn writer(mut pipe: impl Write + Send + 'static) -> Piped {
        let (chunks, receive) = mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            for chunk in receive {
                if pipe.write_all(&chunk).and_then(|()| pipe.flush()).is_err() {
                    return;
                }
            }
        });
        Piped::Writer { chunks }
    }
}

impl Read for Piped {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let Piped::Reader { chunks, buffer, deadline } = self else {
            return Err(io::Error::other("not a reader"));
        };
        if buffer.position() as usize == buffer.get_ref().len() {
            let left = deadline.saturating_duration_since(Instant::now());
            match chunks.recv_timeout(left) {
                Ok(chunk) => *buffer = Cursor::new(chunk?),
                Err(mpsc::RecvTimeoutError::Timeout) => return Err(io::ErrorKind::TimedOut.into()),
                Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(0),
            }
        }
        buffer.read(out)
    }
}

impl Write for Piped {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let Piped::Writer { chunks } = self else {
            return Err(io::Error::other("not a writer"));
        };
        chunks.send(data.to_vec()).map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?;
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Copies the program's stderr to this process's on a thread, so the program and its descendants
/// hold a pipe of this process's rather than its caller's stderr: a descendant left alive can't
/// keep a caller that reads this process's stderr waiting. The channel says when the copy is done.
fn relay_stderr(mut pipe: impl Read + Send + 'static) -> mpsc::Receiver<()> {
    let (done, finished) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = io::copy(&mut pipe, &mut io::stderr());
        let _ = done.send(());
    });
    finished
}

/// How long to wait for the program's last messages on stderr once it has exited.
const STDERR_GRACE: Duration = Duration::from_millis(200);

/// How long `--exec`'s program may take to exit once the session is over.
const EXIT_GRACE: Duration = Duration::from_secs(2);
