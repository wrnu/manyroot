//! A fake `claude` for the agent driver's tests (build step 6): it takes
//! Claude Code's arguments, starts the MCP server its `--mcp-config` names
//! the way Claude Code does, and plays the script in its system prompt file,
//! one directive per line:
//!
//! - `init <key> <json>`: the `init` event shows `<json>` for `<key>` (`null` drops it)
//! - `usage <json>`: the usage later assistant messages carry
//! - `call <tool> <json>`: an assistant message asking for `<tool>`, then the call through MCP
//! - `say [blocks]`: an assistant message with no tool, sent as `blocks` events (default 1)
//! - `result <output>`: a result event adding up the messages, with `<output>` output tokens
//! - `result-json <json>`: a result event as given
//! - `line <text>`: a raw line
//! - `env <NAME>`: an event quoting environment variable `<NAME>`
//! - `stderr-env <NAME>`: the same, on standard error
//! - `probe <json>`: tries each access in `<json>` and reports which were allowed, never what it read
//! - `connect-loop <host:port> <file>`: forever, a `CONNECT` through `HTTPS_PROXY` every 100 ms,
//!   appending `ok` or `no` to `<file>` (after writing its pid to `<file>.pid`)
//! - `sleep <ms>`, `exit <code>`
//!
//! Run as `experiment-fake-claude probe <json>`, it only probes, printing the report: tests run it
//! under the tools and hub profiles in place of the worker binary. Run as `experiment-fake-claude
//! spawn <program> <args>…`, it runs that and exits as it did: a parent for a probe to read whose
//! environment shows, which a system program's doesn't.
//!
//! The `init` event by default shows what Claude Code 2.1.311 shows when started as the driver starts it.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::Duration;

use serde_json::{Map, Value, json};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("spawn") {
        let status = Command::new(&args[2]).args(&args[3..]).status().expect("the program");
        std::process::exit(status.code().unwrap_or(1));
    }
    if args.get(1).map(String::as_str) == Some("probe") {
        let spec: Value = serde_json::from_str(args.get(2).map_or("{}", String::as_str)).expect("a probe spec");
        println!("{}", json!({"type": "probe", "results": probe(&spec)}));
        return;
    }
    let opt = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let script = std::fs::read_to_string(opt("--system-prompt-file").expect("--system-prompt-file")).expect("script");
    let mut fake = Fake {
        out: std::io::stdout(),
        model: opt("--model").unwrap_or_default(),
        usage: json!({"input_tokens": 1000, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0, "output_tokens": 1}),
        messages: BTreeMap::new(),
        next: 0,
        mcp: None,
    };
    let mut init = Map::new();
    let mut lines = script.lines().peekable();
    while let Some(line) = lines.next_if(|l| l.starts_with("init ")) {
        let (key, value) = line["init ".len()..].split_once(' ').expect("init <key> <json>");
        init.insert(key.into(), serde_json::from_str(value).expect("init json"));
    }
    let tools = match opt("--mcp-config") {
        Some(path) => fake.connect(&path),
        None => Vec::new(),
    };
    fake.init(tools, &args, init);
    for line in lines {
        let (verb, rest) = line.split_once(' ').unwrap_or((line, ""));
        match verb {
            "usage" => fake.usage = serde_json::from_str(rest).expect("usage json"),
            "call" => {
                let (tool, input) = rest.split_once(' ').unwrap_or((rest, "{}"));
                fake.call(tool, serde_json::from_str(input).expect("call json"));
            }
            "say" => fake.say(rest.parse().unwrap_or(1)),
            "result" => fake.result(rest.parse().expect("result <output>")),
            "result-json" => fake.emit(&serde_json::from_str(rest).expect("result json")),
            "line" => fake.raw(rest),
            "env" => fake.emit(&json!({"type": "system", "subtype": "env", "value": std::env::var(rest).ok()})),
            "stderr-env" => eprintln!("{}", std::env::var(rest).unwrap_or_default()),
            "probe" => {
                fake.emit(&json!({"type": "probe", "results": probe(&serde_json::from_str(rest).expect("spec"))}))
            }
            "connect-loop" => {
                let (target, file) = rest.split_once(' ').expect("connect-loop <target> <file>");
                connect_loop(target, Path::new(file));
            }
            "sleep" => std::thread::sleep(Duration::from_millis(rest.parse().expect("ms"))),
            "exit" => std::process::exit(rest.parse().expect("code")),
            "" => {}
            other => panic!("unknown directive {other}"),
        }
    }
}

struct Mcp {
    _child: Child,
    to: ChildStdin,
    from: BufReader<ChildStdout>,
    next: u64,
}

impl Mcp {
    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let msg = json!({"jsonrpc": "2.0", "id": self.next, "method": method, "params": params});
        writeln!(self.to, "{msg}").and_then(|()| self.to.flush()).expect("writing to the MCP server");
        let mut line = String::new();
        self.from.read_line(&mut line).expect("reading from the MCP server");
        serde_json::from_str::<Value>(&line).map(|r| r["result"].clone()).unwrap_or(Value::Null)
    }
}

struct Fake {
    out: std::io::Stdout,
    model: String,
    usage: Value,
    /// Per message id, its usage.
    messages: BTreeMap<String, Value>,
    next: u64,
    mcp: Option<Mcp>,
}

impl Fake {
    fn raw(&mut self, line: &str) {
        let _ = writeln!(self.out, "{line}").and_then(|()| self.out.flush());
    }

    fn emit(&mut self, event: &Value) {
        self.raw(&event.to_string());
    }

    /// Starts the one MCP server `path` names, as Claude Code does, and lists its tools.
    fn connect(&mut self, path: &str) -> Vec<String> {
        let config: Value = serde_json::from_str(&std::fs::read_to_string(path).expect("mcp config")).expect("json");
        let Some((name, server)) = config["mcpServers"].as_object().and_then(|m| m.iter().next()) else {
            return Vec::new();
        };
        let args: Vec<&str> = server["args"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        let Ok(mut child) = Command::new(server["command"].as_str().unwrap_or_default())
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        else {
            return Vec::new();
        };
        let to = child.stdin.take().expect("stdin");
        let from = BufReader::new(child.stdout.take().expect("stdout"));
        let mut mcp = Mcp { _child: child, to, from, next: 0 };
        let init = json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "fake-claude"}});
        if mcp.request("initialize", init).is_null() {
            return Vec::new();
        }
        let _ = writeln!(mcp.to, "{}", json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        let tools = mcp.request("tools/list", json!({}));
        let names = tools["tools"].as_array().into_iter().flatten().filter_map(|t| t["name"].as_str());
        let names = names.map(|t| format!("mcp__{name}__{t}")).collect();
        self.mcp = Some(mcp);
        names
    }

    fn init(&mut self, tools: Vec<String>, args: &[String], overrides: Map<String, Value>) {
        let opt = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
        let expect = bracken_experiment::claude::Expect::pinned_2_1_311("");
        let config = std::env::var("CLAUDE_CONFIG_DIR").unwrap_or_default();
        let servers =
            if self.mcp.is_some() { json!([{"name": "experiment", "status": "connected"}]) } else { json!([]) };
        let mut init = json!({
            "type": "system", "subtype": "init",
            "cwd": std::env::current_dir().map(|d| d.display().to_string()).unwrap_or_default(),
            "tools": tools,
            "mcp_servers": servers,
            "model": self.model,
            "permissionMode": opt("--permission-mode"),
            "apiKeySource": "none",
            "claude_code_version": "2.1.311",
            "skills": expect.skills,
            "plugins": expect.plugins.iter().map(|p| json!({"name": p, "path": "builtin"})).collect::<Vec<_>>(),
            "memory_paths": {"auto": format!("{config}/projects/fake/memory/")},
        });
        for (k, v) in overrides {
            match v {
                Value::Null => init.as_object_mut().expect("an object").remove(&k),
                v => init.as_object_mut().expect("an object").insert(k, v),
            };
        }
        self.emit(&init);
    }

    fn message(&mut self, content: Vec<Value>, blocks: usize) -> String {
        self.next += 1;
        let id = format!("msg_{}", self.next);
        self.messages.insert(id.clone(), self.usage.clone());
        for _ in 0..blocks.max(1) {
            let m = json!({"id": id, "model": self.model, "usage": self.usage, "content": content});
            self.emit(&json!({"type": "assistant", "message": m}));
        }
        id
    }

    fn say(&mut self, blocks: usize) {
        self.message(vec![json!({"type": "text", "text": "thinking"})], blocks);
    }

    fn call(&mut self, tool: &str, input: Value) {
        let use_id = format!("toolu_{}", self.next + 1);
        let block =
            json!({"type": "tool_use", "id": use_id, "name": format!("mcp__experiment__{tool}"), "input": input});
        self.message(vec![block], 1);
        let reply = match self.mcp.as_mut() {
            Some(m) => m.request("tools/call", json!({"name": tool, "arguments": input})),
            None => Value::Null,
        };
        let content = json!([{"type": "tool_result", "tool_use_id": use_id, "content": reply["content"]}]);
        self.emit(&json!({"type": "user", "message": {"role": "user", "content": content}}));
    }

    fn result(&mut self, output: u64) {
        let sum = |k: &str| self.messages.values().map(|u| u[k].as_u64().unwrap_or(0)).sum::<u64>();
        let (input, read, write) =
            (sum("input_tokens"), sum("cache_read_input_tokens"), sum("cache_creation_input_tokens"));
        let usage = json!({"input_tokens": input, "cache_read_input_tokens": read,
            "cache_creation_input_tokens": write, "output_tokens": output});
        let models = json!({self.model.clone(): {"inputTokens": input, "cacheReadInputTokens": read,
            "cacheCreationInputTokens": write, "outputTokens": output, "costUSD": 0.01}});
        let result = json!({"type": "result", "subtype": "success", "usage": usage, "modelUsage": models,
            "total_cost_usd": 0.01, "duration_ms": 5});
        self.emit(&result);
    }
}

/// Tries each access in `spec` (lists under `read`, `write`, `exec`, `tcp`, `proxy`, `unix`,
/// `mach`, `process`) and reports `allowed` or `denied` for each, as `<kind>:<target>`.
///
/// For `process`, `parent` or a pid, each way of reading another process is `<way>:<target>`
/// ([`PROCESS_WAYS`]). A read of its arguments and environment is `allowed` only if the variable
/// `spec["plant"]` names is among them, and `read` if it isn't, so a test can tell the read is real.
fn probe(spec: &Value) -> BTreeMap<String, &'static str> {
    let mut results = BTreeMap::new();
    let list = |k: &str| -> Vec<String> {
        spec[k].as_array().into_iter().flatten().filter_map(Value::as_str).map(String::from).collect()
    };
    for target in list("read") {
        let p = Path::new(&target);
        let ok = if p.is_dir() {
            std::fs::read_dir(p).is_ok()
        } else {
            std::fs::File::open(p).and_then(|mut f| f.read(&mut [0u8; 1])).is_ok()
        };
        results.insert(format!("read:{target}"), verdict(ok));
    }
    for target in list("write") {
        // A new file only, so a wrongly allowed write never changes anything that was there.
        let made = std::fs::OpenOptions::new().write(true).create_new(true).open(&target);
        let ok = made.and_then(|mut f| f.write_all(b"probe")).is_ok();
        if ok {
            let _ = std::fs::remove_file(&target);
        }
        results.insert(format!("write:{target}"), verdict(ok));
    }
    for target in list("exec") {
        let started = Command::new(&target)
            .args(["-c", "exit 0"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let ok = started.and_then(|mut c| c.wait()).is_ok();
        results.insert(format!("exec:{target}"), verdict(ok));
    }
    for target in list("tcp") {
        let ok =
            target.parse::<SocketAddr>().is_ok_and(|a| TcpStream::connect_timeout(&a, Duration::from_secs(2)).is_ok());
        results.insert(format!("tcp:{target}"), verdict(ok));
    }
    for target in list("proxy") {
        results.insert(format!("proxy:{target}"), verdict(through_proxy(&target)));
    }
    for target in list("unix") {
        results.insert(format!("unix:{target}"), verdict(UnixStream::connect(&target).is_ok()));
    }
    for target in list("mach") {
        results.insert(format!("mach:{target}"), verdict(mach_lookup(&target)));
    }
    let plant = format!("{}=", spec["plant"].as_str().unwrap_or("BRACKEN_PROBE_PLANTED_NAME"));
    for target in list("process") {
        let pid = if target == "parent" { std::os::unix::process::parent_id() } else { target.parse().unwrap_or(0) };
        for (way, v) in process_reads(pid as i32, plant.as_bytes()) {
            results.insert(format!("{way}:{target}"), v);
        }
    }
    results
}

/// The ways [`process_reads`] tries: the arguments and environment by number and by name, the
/// process table, one process's entry, the list of pids, a process's path and its BSD info, and its
/// process group and session.
pub const PROCESS_WAYS: [&str; 11] = [
    "procargs2-mib",
    "procargs-mib",
    "procargs2-name",
    "procargs-name",
    "kern.proc.all",
    "kern.proc.pid",
    "listallpids",
    "pidpath",
    "pidinfo",
    "getpgid",
    "getsid",
];

/// Elsewhere this probe has nothing to try.
#[cfg(not(target_os = "macos"))]
fn process_reads(_pid: i32, _plant: &[u8]) -> Vec<(&'static str, &'static str)> {
    PROCESS_WAYS.iter().map(|w| (*w, "denied")).collect()
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn sysctl(
        name: *mut i32,
        namelen: u32,
        oldp: *mut std::ffi::c_void,
        oldlenp: *mut usize,
        newp: *mut std::ffi::c_void,
        newlen: usize,
    ) -> i32;
    fn sysctlnametomib(name: *const std::ffi::c_char, mibp: *mut i32, sizep: *mut usize) -> i32;
    fn proc_listallpids(buffer: *mut std::ffi::c_void, buffersize: i32) -> i32;
    fn proc_pidpath(pid: i32, buffer: *mut std::ffi::c_void, buffersize: u32) -> i32;
    fn proc_pidinfo(pid: i32, flavor: i32, arg: u64, buffer: *mut std::ffi::c_void, buffersize: i32) -> i32;
}

/// `pid`'s arguments and environment, its entries in the process table, its path and its info, each
/// tried as macOS offers it (note 116ae2144d9d).
#[cfg(target_os = "macos")]
fn process_reads(pid: i32, plant: &[u8]) -> Vec<(&'static str, &'static str)> {
    const CTL_KERN: i32 = 1;
    const KERN_ARGMAX: i32 = 8;
    const KERN_PROC: i32 = 14;
    const KERN_PROC_ALL: i32 = 0;
    const KERN_PROC_PID: i32 = 1;
    const KERN_PROCARGS: i32 = 38;
    const KERN_PROCARGS2: i32 = 49;
    const KINFO_PROC_SIZE: usize = 648;
    const PROC_PIDTBSDINFO: i32 = 3;
    const PROC_BSDINFO_SIZE: usize = 136;
    // SAFETY, for each call below: every pointer is to a live buffer of the length passed with it.
    let read = |mib: &mut [i32], buf: &mut [u8]| -> Option<usize> {
        let mut len = buf.len();
        let ptr = if buf.is_empty() { std::ptr::null_mut() } else { buf.as_mut_ptr().cast() };
        let rc = unsafe { sysctl(mib.as_mut_ptr(), mib.len() as u32, ptr, &mut len, std::ptr::null_mut(), 0) };
        (rc == 0).then_some(len)
    };
    let mut argmax = [0u8; 4];
    let argmax = match read(&mut [CTL_KERN, KERN_ARGMAX], &mut argmax) {
        Some(4) => (i32::from_ne_bytes(argmax) as usize).clamp(4096, 4 << 20),
        _ => 1 << 20,
    };
    // A larger buffer than KERN_ARGMAX is refused (EINVAL), so it is exactly that.
    let mut buf = vec![0u8; argmax];
    let mut args = |mib: &mut [i32]| -> &'static str {
        match read(mib, &mut buf) {
            None => "denied",
            Some(n) if buf[..n].windows(plant.len()).any(|w| w == plant) => "allowed",
            Some(_) => "read",
        }
    };
    let by_name = |name: &str| -> Option<Vec<i32>> {
        let name = std::ffi::CString::new(name).expect("a name without NUL");
        let mut mib = [0i32; 12];
        let mut n = mib.len() - 1;
        (unsafe { sysctlnametomib(name.as_ptr(), mib.as_mut_ptr(), &mut n) } == 0).then(|| {
            let mut v = mib[..n].to_vec();
            v.push(pid);
            v
        })
    };
    let procargs2 = args(&mut [CTL_KERN, KERN_PROCARGS2, pid]);
    let procargs = args(&mut [CTL_KERN, KERN_PROCARGS, pid]);
    let procargs2_name = by_name("kern.procargs2").map_or("denied", |mut m| args(&mut m));
    let procargs_name = by_name("kern.procargs").map_or("denied", |mut m| args(&mut m));
    // A table's size alone: open if it holds more than one entry, or for one process any.
    let size = |mib: &mut [i32]| read(mib, &mut []);
    let all = verdict(size(&mut [CTL_KERN, KERN_PROC, KERN_PROC_ALL]).is_some_and(|n| n > KINFO_PROC_SIZE));
    let one = verdict(size(&mut [CTL_KERN, KERN_PROC, KERN_PROC_PID, pid]).is_some_and(|n| n > 0));
    let pids = verdict(unsafe { proc_listallpids(std::ptr::null_mut(), 0) } > 1);
    let mut path = [0u8; 4096];
    let path = verdict(unsafe { proc_pidpath(pid, path.as_mut_ptr().cast(), path.len() as u32) } > 0);
    let mut info = [0u8; PROC_BSDINFO_SIZE];
    let info =
        verdict(unsafe { proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, info.as_mut_ptr().cast(), info.len() as i32) } > 0);
    let pgid = verdict(rustix::process::getpgid(rustix::process::Pid::from_raw(pid)).is_ok());
    let sid = verdict(rustix::process::getsid(rustix::process::Pid::from_raw(pid)).is_ok());
    let got = [procargs2, procargs, procargs2_name, procargs_name, all, one, pids, path, info, pgid, sid];
    PROCESS_WAYS.iter().copied().zip(got).collect()
}

fn verdict(ok: bool) -> &'static str {
    if ok { "allowed" } else { "denied" }
}

/// Whether the forwarding proxy in `HTTPS_PROXY` opens a tunnel to `target`.
fn through_proxy(target: &str) -> bool {
    let Some(addr) = std::env::var("HTTPS_PROXY").ok().and_then(|p| p.strip_prefix("http://").map(String::from)) else {
        return false;
    };
    let Ok(mut s) = TcpStream::connect(addr) else { return false };
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    if write!(s, "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n").is_err() {
        return false;
    }
    let mut buf = [0u8; 64];
    let n = s.read(&mut buf).unwrap_or(0);
    buf[..n].starts_with(b"HTTP/1.1 200")
}

fn connect_loop(target: &str, file: &Path) -> ! {
    std::fs::write(file.with_extension("pid"), std::process::id().to_string()).expect("writing the pid");
    loop {
        let ok = through_proxy(target);
        let mut f = std::fs::OpenOptions::new().append(true).create(true).open(file).expect("the log");
        let _ = writeln!(f, "{}", if ok { "ok" } else { "no" });
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    static bootstrap_port: u32;
    fn bootstrap_look_up(bp: u32, name: *const std::ffi::c_char, sp: *mut u32) -> i32;
}

/// Elsewhere there are no mach services, so none can be looked up.
#[cfg(not(target_os = "macos"))]
fn mach_lookup(_name: &str) -> bool {
    false
}

/// Whether the mach service `name` can be looked up: the keychain is behind such services.
#[cfg(target_os = "macos")]
fn mach_lookup(name: &str) -> bool {
    let Ok(name) = std::ffi::CString::new(name) else { return false };
    let mut port = 0u32;
    // SAFETY: bootstrap_port is the process's bootstrap port, set by the runtime before main;
    // name is a valid C string and port a valid out pointer.
    unsafe { bootstrap_look_up(bootstrap_port, name.as_ptr(), &mut port) == 0 }
}
