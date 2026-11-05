//! The team server signing for the identities whose keys it holds (docs/design/agent-keys.md, 4.1 and 4.2,
//! build step 2): `bracken serve --keys`, `POST /v1/<ground>/sign`, `scopes.toml` and the signing log,
//! and `bracken`'s writes signing through it, end to end against throwaway Grounds and servers.

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::Duration;

use manyroot::{AgentId, Body, Contribution, ContributionContent, GroundId, RelationKind};
use serde_json::Value;

const BIN: &str = env!("CARGO_BIN_EXE_bracken");
const SRV: &str = "token-for-srv";
const UNSCOPED: &str = "token-for-unscoped";
const KEYLESS: &str = "token-for-keyless";

fn cmd(dir: &Path) -> Command {
    let mut c = Command::new(BIN);
    c.env("BRACKEN_DIR", dir)
        .env_remove("BRACKEN_AGENT")
        .env_remove("BRACKEN_TOKEN")
        .env_remove("BRACKEN_SIGN_URL")
        .env_remove("BRACKEN_SIGN_TOKEN_FILE");
    c
}

fn ok(dir: &Path, args: &[&str]) -> String {
    let out = cmd(dir).args(args).output().unwrap();
    assert!(out.status.success(), "bracken {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
    common::bare_ids(String::from_utf8(out.stdout).unwrap().trim().to_owned())
}

const SCOPES: &str = r#"
# What the server signs for each identity it holds.
[scope."srv*"]
kinds = ["claim", "relation", "evidence", "retract"]
relations = ["implements", "refines"]
forbidden_tags = ["decision", "approval"]
"#;

/// A client replica `client` with a local identity `local` and two server-held ones, `srv` and
/// `unscoped` (keys in `keys/`, public entries in the client's directory), and a server replica of the
/// same Ground; `scopes.toml` gives srv a scope and `unscoped` none, and the tokens file has a third
/// credential, `keyless`, with no key.
struct Team {
    tmp: PathBuf,
    client: PathBuf,
    server: PathBuf,
}

fn team() -> Team {
    let tmp = tempfile::tempdir().unwrap().keep();
    let (client, server, keys) = (tmp.join("client"), tmp.join("server"), tmp.join("keys"));
    ok(&client, &["init", "--name", "g"]);
    ok(&client, &["id", "new", "local"]);
    std::fs::create_dir(&keys).unwrap();
    set_mode(&keys, 0o700);
    for name in ["srv", "unscoped"] {
        ok(&client, &["id", "new", name, "--key-dir", keys.to_str().unwrap()]);
    }
    let bundle = tmp.join("seed.mrb");
    ok(&client, &["export", bundle.to_str().unwrap()]);
    ok(&server, &["join", bundle.to_str().unwrap()]);
    let allow: String = ["local", "srv", "unscoped"].iter().map(|n| key(&client, n).to_hex() + "\n").collect();
    std::fs::write(tmp.join("allow"), allow).unwrap();
    let hash = |t: &str| blake3::hash(t.as_bytes()).to_hex().to_string();
    let tokens = format!("srv {}\nunscoped {}\nkeyless {}\n", hash(SRV), hash(UNSCOPED), hash(KEYLESS));
    std::fs::write(tmp.join("tokens"), tokens).unwrap();
    std::fs::write(tmp.join("scopes.toml"), SCOPES).unwrap();
    for (name, token) in [("srv", SRV), ("unscoped", UNSCOPED), ("keyless", KEYLESS)] {
        std::fs::write(tmp.join(format!("{name}.token")), token).unwrap();
    }
    Team { tmp, client, server }
}

fn set_mode(path: &Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }
    let _ = (path, mode);
}

fn key(dir: &Path, name: &str) -> AgentId {
    let list: Value = serde_json::from_str(&ok(dir, &["id", "list", "--json"])).unwrap();
    let entry = list.as_array().unwrap().iter().find(|e| e["name"] == name).unwrap();
    entry["agent"].as_str().unwrap().parse().unwrap()
}

fn ground_id(dir: &Path) -> GroundId {
    let info: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("ground.json")).unwrap()).unwrap();
    info["ground"].as_str().unwrap().parse().unwrap()
}

struct Server {
    child: Child,
    url: String,
    port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Team {
    fn path(&self, name: &str) -> String {
        self.tmp.join(name).to_str().unwrap().to_owned()
    }

    fn serve(&self) -> Server {
        let args = [
            "serve",
            "--tokens",
            &self.path("tokens"),
            "--allow",
            &self.path("allow"),
            "--first-seen",
            &self.path("first-seen.jsonl"),
            "--keys",
            &self.path("keys"),
            "--scopes",
            &self.path("scopes.toml"),
            "--signed",
            &self.path("signed.jsonl"),
        ];
        let mut child = cmd(&self.server).args(args).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
        let url = line.trim().strip_prefix("listening on ").unwrap_or_else(|| panic!("no URL in {line:?}")).to_owned();
        let port = url.rsplit(':').next().unwrap().parse().unwrap();
        Server { child, url, port }
    }

    /// `bracken <args> --as <who>` in the client, signing through the server with `who`'s token.
    fn write(&self, server: &Server, who: &str, args: &[&str]) -> Output {
        cmd(&self.client)
            .args(args)
            .args(["--as", who])
            .env("BRACKEN_SIGN_URL", &server.url)
            .env("BRACKEN_SIGN_TOKEN_FILE", self.tmp.join(format!("{who}.token")))
            .output()
            .unwrap()
    }

    fn signed(&self) -> Vec<Value> {
        let text = std::fs::read_to_string(self.tmp.join("signed.jsonl")).unwrap_or_default();
        text.lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }

    /// Content by `author`, for a raw sign request.
    fn content(&self, author: AgentId, body: Body) -> ContributionContent {
        ContributionContent {
            protocol: 1,
            ground: ground_id(&self.client),
            author,
            parents: vec![],
            timestamp_ms: 1,
            body,
        }
    }
}

fn claim(text: &str, tags: &[&str]) -> Body {
    Body::Claim { statement: text.into(), tags: tags.iter().map(|t| (*t).to_owned()).collect() }
}

fn sign(port: u16, ground: GroundId, token: &str, body: &[u8]) -> (u16, String) {
    let head = format!(
        "POST /v1/{ground}/sign HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(40))).unwrap();
    s.write_all(&[head.as_bytes(), body].concat()).unwrap();
    let _ = s.shutdown(std::net::Shutdown::Write);
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    let split = out.windows(4).position(|w| w == b"\r\n\r\n").map_or(out.len(), |i| i + 4);
    let status = String::from_utf8_lossy(&out[..split]).split_whitespace().nth(1).and_then(|s| s.parse().ok());
    (status.unwrap_or(0), String::from_utf8_lossy(&out[split..]).into_owned())
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// A server-held identity's writes are signed by the server, verify under its key, are logged with its
/// credential, and push like any other; an identity with a local key file still signs from it, and the
/// server never sees its content.
#[test]
fn the_server_signs_for_the_identities_it_holds_and_logs_each_signature() {
    let t = team();
    let server = t.serve();
    let out = t.write(&server, "srv", &["claim", "signed by the server", "--tag", "note"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let claim_id = common::bare_ids(String::from_utf8(out.stdout).unwrap().trim().to_owned());
    let first = ok(&t.client, &["claim", "an earlier claim", "--as", "local"]);
    let out = t.write(&server, "srv", &["relate", &claim_id, "refines", &first]);
    assert!(out.status.success(), "{}", stderr(&out));
    let relation_id = common::bare_ids(String::from_utf8(out.stdout).unwrap().trim().to_owned());

    let shown: Value = serde_json::from_str(&ok(&t.client, &["show", &claim_id, "--json"])).unwrap();
    assert_eq!(shown["author"].as_str().unwrap(), key(&t.client, "srv").to_hex(), "{shown}");
    ok(&t.client, &["verify"]);
    let logged: Vec<(String, String)> = t
        .signed()
        .iter()
        .map(|e| (e["id"].as_str().unwrap().to_owned(), e["credential"].as_str().unwrap().to_owned()))
        .collect();
    assert_eq!(logged, [(claim_id.clone(), "srv".to_owned()), (relation_id, "srv".to_owned())]);

    // The local identity signs from its file, with or without a signing server, and nothing is logged.
    let out = t.write(&server, "local", &["claim", "signed locally"]);
    assert!(out.status.success(), "{}", stderr(&out));
    ok(&t.client, &["claim", "signed locally, no server", "--as", "local"]);
    assert_eq!(t.signed().len(), 2);

    // Everything pushes, the server-signed contributions too.
    let push =
        cmd(&t.client).args(["sync", &server.url, "--token-file", &t.path("srv.token"), "--confirm"]).output().unwrap();
    assert!(push.status.success(), "{}", stderr(&push));

    // Without a signing server, a server-held identity can't sign at all.
    let out = cmd(&t.client).args(["claim", "x", "--as", "srv"]).output().unwrap();
    assert!(!out.status.success() && stderr(&out).contains("no signing server"), "{}", stderr(&out));
}

/// What the server refuses to sign, each with nothing logged: a body outside the identity's scope, an
/// identity with no scope, a credential with no key, another identity's key as author (so a token can't
/// sign as another key), another Ground, another protocol, content that isn't canonical, and content
/// holding a key the server holds.
#[test]
fn the_server_signs_only_its_own_canonical_content_within_scope() {
    let t = team();
    let server = t.serve();
    let ground = ground_id(&t.client);
    let (srv, unscoped) = (key(&t.client, "srv"), key(&t.client, "unscoped"));
    let refused = |token: &str, content: &ContributionContent, why: &str| {
        let (status, body) = sign(server.port, ground, token, &content.encode());
        assert!(status == 403 && body.contains(why), "{why}: {status} {body}");
    };
    let id = manyroot::ContributionId([3; 32]);
    let supersedes = Body::Relation { from: id, kind: RelationKind::Supersedes, to: id, note: String::new() };
    let artifact = Body::Artifact { name: "a".into(), media_type: "text/plain".into(), content: b"a".to_vec() };
    refused(SRV, &t.content(srv, claim("x", &["note", "decision"])), "the tag \"decision\"");
    refused(SRV, &t.content(srv, supersedes), "supersedes relations");
    refused(SRV, &t.content(srv, artifact), "doesn't include artifacts");
    refused(UNSCOPED, &t.content(unscoped, claim("x", &[])), "unscoped has no scope");
    refused(KEYLESS, &t.content(srv, claim("x", &[])), "holds no key for keyless");
    refused(SRV, &t.content(unscoped, claim("x", &[])), "isn't srv's key");
    let mut other = t.content(srv, claim("x", &[]));
    other.ground = GroundId([9; 32]);
    refused(SRV, &other, "for Ground");
    let mut later = t.content(srv, claim("x", &[]));
    later.protocol = 2;
    refused(SRV, &later, "protocol 2");
    let keyfile = std::fs::read_to_string(t.tmp.join("keys/unscoped.json")).unwrap();
    refused(SRV, &t.content(srv, claim(&keyfile, &[])), "private key of unscoped");

    let good = t.content(srv, claim("x", &["note"]));
    let mut trailing = good.encode();
    trailing.push(0);
    let (status, body) = sign(server.port, ground, SRV, &trailing);
    assert!(status == 400 && body.contains("canonical"), "{status} {body}");
    let (status, _) = sign(server.port, ground, "not-a-token", &good.encode());
    assert_eq!(status, 401);
    assert!(t.signed().is_empty(), "{:?}", t.signed());

    // The one it signs is logged.
    assert_eq!(sign(server.port, ground, SRV, &good.encode()).0, 200);
    assert_eq!(t.signed().len(), 1);
    assert_eq!(t.signed()[0]["id"].as_str().unwrap(), good.id().to_hex());

    // scopes.toml is read on every request: an edit takes effect at once, and one that doesn't parse
    // signs nothing.
    std::fs::write(t.tmp.join("scopes.toml"), SCOPES.replace("forbidden_tags = [\"decision\", \"approval\"]", ""))
        .unwrap();
    let (status, body) = sign(server.port, ground, SRV, &t.content(srv, claim("y", &["decision"])).encode());
    assert_eq!(status, 200, "{body}");
    std::fs::write(t.tmp.join("scopes.toml"), "[scope.srv\n").unwrap();
    refused(SRV, &t.content(srv, claim("z", &[])), "don't parse");
    assert_eq!(t.signed().len(), 2);
}

/// A key moved out of the key directory (retired) is no longer signed with, and a key directory open to
/// other users, or a key file under another name, stops the server starting.
#[test]
fn a_retired_key_is_refused_and_the_key_directory_is_checked() {
    let t = team();
    std::fs::create_dir(t.tmp.join("keys/retired")).unwrap();
    set_mode(&t.tmp.join("keys/retired"), 0o700);
    std::fs::rename(t.tmp.join("keys/srv.json"), t.tmp.join("keys/retired/srv.json")).unwrap();
    let server = t.serve();
    let out = t.write(&server, "srv", &["claim", "after retiring"]);
    assert!(!out.status.success() && stderr(&out).contains("holds no key for srv"), "{}", stderr(&out));
    assert!(t.signed().is_empty());
    // A retired key is still the server's: content holding it is refused (finding 6f54cb439f9b).
    let retired = std::fs::read_to_string(t.tmp.join("keys/retired/srv.json")).unwrap();
    let secret: Value = serde_json::from_str(&retired).unwrap();
    let pasted = format!("pasted: {}", secret["secret_key"].as_str().unwrap());
    std::fs::write(t.tmp.join("scopes.toml"), SCOPES.replace("\"srv*\"", "\"*\"")).unwrap();
    let out = t.write(&server, "unscoped", &["claim", &pasted]);
    assert!(!out.status.success() && stderr(&out).contains("private key of srv"), "{}", stderr(&out));
    assert!(t.signed().is_empty());
    let out = t.write(&server, "unscoped", &["claim", "nothing pasted"]);
    assert!(out.status.success(), "{}", stderr(&out));
    drop(server);
    let signed = t.signed().len();

    // A retired key put back in keys/ isn't signed with: copied back while retired/ still has it, or moved
    // back with retired.jsonl naming it (finding ab071f5d3de6).
    std::fs::copy(t.tmp.join("keys/retired/srv.json"), t.tmp.join("keys/srv.json")).unwrap();
    let server = t.serve();
    let out = t.write(&server, "srv", &["claim", "copied back"]);
    assert!(!out.status.success() && stderr(&out).contains("holds no key for srv"), "{}", stderr(&out));
    drop(server);
    let srv = key(&t.client, "srv").to_hex();
    std::fs::write(t.tmp.join("keys/retired.jsonl"), format!("{{\"name\":\"srv\",\"agent\":\"{srv}\"}}\n")).unwrap();
    std::fs::remove_file(t.tmp.join("keys/retired/srv.json")).unwrap();
    let server = t.serve();
    let out = t.write(&server, "srv", &["claim", "moved back"]);
    assert!(!out.status.success() && stderr(&out).contains("holds no key for srv"), "{}", stderr(&out));
    let out = t.write(&server, "unscoped", &["claim", "unscoped still signs"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(t.signed().len(), signed + 1);
    drop(server);
    std::fs::rename(t.tmp.join("keys/srv.json"), t.tmp.join("keys/retired/srv.json")).unwrap();

    let start = |t: &Team| {
        cmd(&t.server)
            .args(["serve", "--tokens", &t.path("tokens"), "--allow", &t.path("allow")])
            .args(["--first-seen", &t.path("first-seen.jsonl"), "--keys", &t.path("keys")])
            .args(["--scopes", &t.path("scopes.toml"), "--signed", &t.path("signed.jsonl")])
            .stdout(Stdio::null())
            .output()
            .unwrap()
    };
    set_mode(&t.tmp.join("keys"), 0o755);
    let out = start(&t);
    assert!(!out.status.success() && stderr(&out).contains("open to other users"), "{}", stderr(&out));
    set_mode(&t.tmp.join("keys"), 0o700);
    std::fs::copy(t.tmp.join("keys/unscoped.json"), t.tmp.join("keys/renamed.json")).unwrap();
    let out = start(&t);
    assert!(!out.status.success() && stderr(&out).contains("not \"renamed\""), "{}", stderr(&out));
}

/// A contribution made with a server-held key outside the server verifies like any other: the signing
/// log is what tells them apart, which scripts/ground-guards reports on. Here, only that the bytes are
/// the same either way.
#[test]
fn a_server_signature_is_the_signature_the_key_file_makes() {
    let t = team();
    let server = t.serve();
    let ground = ground_id(&t.client);
    let srv = key(&t.client, "srv");
    let content = t.content(srv, claim("same bytes", &["note"]));
    let head = format!(
        "POST /v1/{ground}/sign HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {SRV}\r\nContent-Length: {}\r\n\r\n",
        content.encode().len()
    );
    let mut s = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
    s.write_all(&[head.as_bytes(), &content.encode()].concat()).unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).unwrap();
    let split = out.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
    let signature = &out[split..];
    let identity = manyroot::Identity::load(&t.tmp.join("keys/srv.json")).unwrap();
    let local = Contribution::create(ground, &identity, [], 1, claim("same bytes", &["note"]));
    assert_eq!(signature, local.signature().0);
}
