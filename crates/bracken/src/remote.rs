//! `bracken sync <url>` and `bracken sync --exec`: this replica's side of a session with a
//! server, over HTTP on the loopback interface or over a program's stdin and stdout.
//!
//! The client holds the server to the same [`Bounds`] the server holds it to.

use std::io::{self, Cursor, Read, Write};
use std::net::TcpStream;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use manyroot::sync::Rejected;
use manyroot::{Contribution, Graph, ImportReport, Store};

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
        let (status, id, length, early) = match read_response_head(&mut stream) {
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

/// A response's status, session id header, length, and the body bytes read with its head.
fn read_response_head(stream: &mut impl Read) -> Result<(u16, Option<String>, u64, Vec<u8>)> {
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
    Ok((status, id, length.context("the server's response has no Content-Length")?, early))
}

/// Syncs with a server reached through a program's stdin and stdout (`ssh host bracken serve
/// --stdio`), run without a shell and without this side's token or replica in its environment.
/// The program is killed at the deadline, or soon after the session ends if it doesn't exit.
pub fn sync_exec(store: &mut Store, program: &[String], bounds: Bounds) -> Result<Side> {
    let (name, args) = program.split_first().context("--exec needs a program")?;
    let mut child = Command::new(name)
        .args(args)
        .env_remove("BRACKEN_TOKEN")
        .env_remove("BRACKEN_DIR")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("running {name}"))?;
    let mut input = child.stdin.take().expect("stdin is piped");
    let mut output = io::BufReader::new(child.stdout.take().expect("stdout is piped"));
    let child = Arc::new(Mutex::new(child));
    let mut side = side(store, bounds);
    let deadline = side.deadline();
    // Kills the program at the deadline, which unblocks a read or write stuck on it.
    let (done, finished) = mpsc::channel::<()>();
    let watchdog = {
        let child = Arc::clone(&child);
        std::thread::spawn(move || {
            let timed_out = finished.recv_timeout(deadline.saturating_duration_since(Instant::now())).is_err();
            if timed_out {
                let _ = child.lock().map(|mut c| c.kill());
            }
            timed_out
        })
    };
    let opening = side.session.start(store.graph());
    let result =
        run_stream(&mut side, opening, &mut output, &mut input, |side, frame| side.handle(&mut Local(store), frame));
    drop(input);
    // The session is over either way: give the program a moment to exit, within the deadline.
    let until = deadline.min(Instant::now() + EXIT_GRACE);
    let status = loop {
        let mut c = child.lock().map_err(|_| anyhow::anyhow!("the child's lock is poisoned"))?;
        if let Some(status) = c.try_wait()? {
            break Some(status);
        }
        if Instant::now() >= until {
            let _ = c.kill();
            c.wait()?;
            break None;
        }
        drop(c);
        std::thread::sleep(Duration::from_millis(20));
    };
    let _ = done.send(());
    let timed_out = watchdog.join().unwrap_or(false) || Instant::now() >= deadline;
    match (result, status) {
        (Err(_), _) | (Ok(()), None) if timed_out => Err(Refused::deadline().into()),
        (Err(e), _) => Err(e),
        (Ok(()), None) => bail!("{name} didn't exit after the session ended; it was killed"),
        (Ok(()), Some(status)) if !status.success() => bail!("{name} exited with {status}"),
        (Ok(()), Some(_)) => Ok(side),
    }
}

/// How long `--exec`'s program may take to exit once the session is over.
const EXIT_GRACE: Duration = Duration::from_secs(2);
