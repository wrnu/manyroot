//! The MCP shim Claude Code starts (design section 3): it relays bytes
//! between its standard input and output and the driver's socket for one
//! agent, and does nothing else. It refuses to start with any environment
//! variable set, so it can never hold the proxy's auth value.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

pub fn run(socket: &Path) -> Result<(), String> {
    if std::env::vars_os().next().is_some() {
        return Err("the shim must start with an empty environment".into());
    }
    let stream = UnixStream::connect(socket).map_err(|e| format!("connecting to {}: {e}", socket.display()))?;
    let mut to_driver = stream.try_clone().map_err(|e| e.to_string())?;
    let up = std::thread::spawn(move || {
        let _ = relay(&mut std::io::stdin().lock(), &mut to_driver);
        let _ = to_driver.shutdown(std::net::Shutdown::Write);
    });
    let mut from_driver = stream;
    relay(&mut from_driver, &mut std::io::stdout().lock())?;
    let _ = up.join();
    Ok(())
}

/// A socket read with one deadline for all its reads: each read waits at most until it, and
/// after it every read fails, so nothing reading through this can wait for ever.
pub struct Timed {
    pub stream: std::os::unix::net::UnixStream,
    pub deadline: std::time::Instant,
}

impl Read for Timed {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let left = self.deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "the deadline passed"));
        }
        // macOS refuses a timeout (EINVAL) once the peer has gone; the read then ends at once anyway.
        let _ = self.stream.set_read_timeout(Some(left));
        match self.stream.read(buf) {
            // A read timeout on macOS is EAGAIN: the deadline passed.
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "the deadline passed"))
            }
            other => other,
        }
    }
}

/// Copies `from` to `to` until `from` ends or `stop` is set, reading in short waits; an error
/// once `deadline` passes. The bytes copied.
pub fn relay_until(
    from: &std::os::unix::net::UnixStream,
    to: &mut impl Write,
    stop: &std::sync::atomic::AtomicBool,
    deadline: std::time::Instant,
) -> Result<u64, String> {
    // macOS refuses a timeout (EINVAL) once the peer has gone; reads then end at once anyway.
    let _ = from.set_read_timeout(Some(std::time::Duration::from_millis(50)));
    let mut buf = [0u8; 8192];
    let mut total = 0;
    loop {
        match (&mut &*from).read(&mut buf) {
            Ok(0) => return Ok(total),
            Ok(n) => {
                to.write_all(&buf[..n]).and_then(|()| to.flush()).map_err(|e| e.to_string())?;
                total += n as u64;
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                if stop.load(std::sync::atomic::Ordering::SeqCst) {
                    return Ok(total);
                }
                if std::time::Instant::now() > deadline {
                    return Err("a relay's deadline passed".into());
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// Copies `from` to `to` until `from` ends, flushing as it goes; the bytes copied.
pub fn relay(from: &mut impl Read, to: &mut impl Write) -> Result<u64, String> {
    let mut buf = [0u8; 8192];
    let mut total = 0;
    loop {
        let n = from.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            return Ok(total);
        }
        to.write_all(&buf[..n]).and_then(|()| to.flush()).map_err(|e| e.to_string())?;
        total += n as u64;
    }
}
