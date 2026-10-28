//! The forwarding proxy (design section 3, Isolation, layer 4): `sandbox-exec`
//! filters by port, not by host, so Claude Code gets `HTTPS_PROXY` pointing
//! here, the agent profile lets it reach only this port, and this accepts
//! `CONNECT` only to the host and port of the owner's `ANTHROPIC_BASE_URL`.
//!
//! It only relays bytes: TLS runs between Claude Code and the LLM proxy, so it
//! never sees the auth value. It records each attempt's target and byte
//! counts, never a header. When it is dropped, or the driver dies, every
//! tunnel closes with it.

use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// The longest request head read before refusing.
const MAX_HEAD: usize = 8 * 1024;
/// How long a client has to send its request head.
const HEAD_DEADLINE: Duration = Duration::from_secs(5);
/// How long connecting to the allowed host may take.
const CONNECT_DEADLINE: Duration = Duration::from_secs(10);
/// The most tunnels open at once; more are refused.
const MAX_TUNNELS: usize = 64;

/// Where tunnels may go.
#[derive(Debug, Clone)]
pub struct Route {
    /// The one host `CONNECT` may name (compared without case), and its port.
    pub host: String,
    pub port: u16,
    /// Tests only: connect here instead of resolving `host`.
    pub connect: Option<SocketAddr>,
}

/// One `CONNECT` (or anything else) the proxy received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    /// The target it named, `host:port`, at most 300 printable characters; empty if none could be read.
    pub target: String,
    pub allowed: bool,
    /// Bytes relayed to and from the allowed host.
    pub up: u64,
    pub down: u64,
}

struct Shared {
    route: Route,
    idle: Duration,
    stop: AtomicBool,
    attempts: Mutex<Vec<Attempt>>,
    open: Mutex<Vec<TcpStream>>,
}

pub struct Proxy {
    port: u16,
    shared: Arc<Shared>,
    accept: Option<JoinHandle<()>>,
}

impl Proxy {
    /// Listens on a free port on 127.0.0.1. A tunnel quiet for `idle` closes.
    pub fn start(route: Route, idle: Duration) -> Result<Proxy, String> {
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("binding the proxy: {e}"))?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let shared = Arc::new(Shared {
            route,
            idle,
            stop: AtomicBool::new(false),
            attempts: Mutex::new(Vec::new()),
            open: Mutex::new(Vec::new()),
        });
        let s = Arc::clone(&shared);
        let accept = std::thread::spawn(move || {
            while !s.stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let s = Arc::clone(&s);
                        std::thread::spawn(move || s.handle(stream));
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(5)),
                }
            }
        });
        Ok(Proxy { port, shared, accept: Some(accept) })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Every attempt so far, in the order they finished.
    pub fn attempts(&self) -> Vec<Attempt> {
        self.shared.attempts.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.accept.take() {
            let _ = t.join();
        }
        for s in self.shared.open.lock().unwrap_or_else(|e| e.into_inner()).drain(..) {
            let _ = s.shutdown(Shutdown::Both);
        }
    }
}

impl Shared {
    fn record(&self, attempt: Attempt) {
        self.attempts.lock().unwrap_or_else(|e| e.into_inner()).push(attempt);
    }

    /// Keeps a handle to `s` so dropping the proxy closes it; false if too many are open or it is stopping.
    fn track(&self, s: &TcpStream) -> bool {
        let mut open = self.open.lock().unwrap_or_else(|e| e.into_inner());
        open.retain(|o| o.peer_addr().is_ok());
        if open.len() >= MAX_TUNNELS || self.stop.load(Ordering::SeqCst) {
            return false;
        }
        s.try_clone().map(|c| open.push(c)).is_ok()
    }

    fn handle(&self, mut client: TcpStream) {
        let _ = client.set_nonblocking(false);
        let _ = client.set_read_timeout(Some(HEAD_DEADLINE));
        let target = read_target(&mut client);
        let allowed = target.as_deref().is_some_and(|t| self.allows(t));
        let mut attempt =
            Attempt { target: target.as_deref().map(printable).unwrap_or_default(), allowed, up: 0, down: 0 };
        let upstream = if allowed { self.connect() } else { None };
        let Some(upstream) = upstream.filter(|u| self.track(&client) && self.track(u)) else {
            attempt.allowed = false;
            let _ = client.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n");
            let _ = client.shutdown(Shutdown::Both);
            self.record(attempt);
            return;
        };
        if client.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n").is_err() {
            self.record(attempt);
            return;
        }
        for s in [&client, &upstream] {
            let _ = s.set_read_timeout(Some(self.idle));
        }
        let (up, down) = std::thread::scope(|scope| {
            let up = scope.spawn(|| pipe(&client, &upstream));
            let down = pipe(&upstream, &client);
            (up.join().unwrap_or(0), down)
        });
        (attempt.up, attempt.down) = (up, down);
        self.record(attempt);
    }

    fn allows(&self, target: &str) -> bool {
        let Some((host, port)) = target.rsplit_once(':') else { return false };
        host.eq_ignore_ascii_case(&self.route.host) && port.parse::<u16>().ok() == Some(self.route.port)
    }

    fn connect(&self) -> Option<TcpStream> {
        let addrs: Vec<SocketAddr> = match self.route.connect {
            Some(a) => vec![a],
            None => (self.route.host.as_str(), self.route.port).to_socket_addrs().ok()?.collect(),
        };
        addrs.iter().find_map(|a| TcpStream::connect_timeout(a, CONNECT_DEADLINE).ok())
    }
}

/// The target of a `CONNECT host:port HTTP/1.x` head, or None for anything else.
fn read_target(client: &mut TcpStream) -> Option<String> {
    let mut head = Vec::new();
    let mut buf = [0u8; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = client.read(&mut buf).ok().filter(|&n| n > 0)?;
        head.extend_from_slice(&buf[..n]);
        if head.len() > MAX_HEAD {
            return None;
        }
    }
    let line = head.split(|&b| b == b'\r').next()?;
    let line = std::str::from_utf8(line).ok()?;
    match line.split(' ').collect::<Vec<_>>().as_slice() {
        ["CONNECT", target, version] if version.starts_with("HTTP/1.") => Some(target.to_string()),
        _ => None,
    }
}

fn printable(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_graphic()).take(300).collect()
}

/// Copies until either side ends, is quiet too long, or fails, then closes both
/// (a TLS tunnel has no use for half a connection); the bytes copied.
fn pipe(from: &TcpStream, to: &TcpStream) -> u64 {
    let mut buf = [0u8; 16 * 1024];
    let mut total = 0;
    while let Ok(n) = (&mut &*from).read(&mut buf) {
        if n == 0 || (&mut &*to).write_all(&buf[..n]).is_err() {
            break;
        }
        total += n as u64;
    }
    let _ = to.shutdown(Shutdown::Both);
    let _ = from.shutdown(Shutdown::Both);
    total
}
