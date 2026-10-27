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
