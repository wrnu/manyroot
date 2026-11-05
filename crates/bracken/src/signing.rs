//! Signing on the team server (docs/design/agent-keys.md, 4.1 and 4.2): the server holds new identities'
//! keys and signs for them, so no new key sits under a repository's working tree.
//!
//! `bracken serve --keys <dir>` binds each credential to the key of the same name in `<dir>` (a private
//! directory of identity files). `POST /v1/<ground>/sign`, with that credential's bearer token and a
//! contribution's canonical content as the body, answers with the 64-byte signature, after checking:
//!
//! 1. the content decodes, and re-encodes to the same bytes; its `ground` is the server's, its `protocol`
//!    is 1, and its `author` is the key bound to the token;
//! 2. it holds none of the server's keys, as hex or raw bytes;
//! 3. its body is within the identity's scope in `--scopes` (a TOML file, read on every request, so an
//!    edit takes effect at once; a name with no entry gets no scope, and a file that doesn't parse signs
//!    nothing);
//!
//! and once `{id, credential, signed_ms}` is in the signing log, `--signed`, synced to disk. The server
//! signs only content it has read, never a bare id, and only with the existing signing context.
//!
//! [`RemoteSigner`] is the client's side: a [`Signer`] that sends the content and checks what comes back.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Cursor, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use manyroot::contribution::{MAX_CONTRIBUTION_BYTES, PROTOCOL_VERSION};
use manyroot::{
    AgentId, Body, BodyKind, ContributionContent, ContributionId, GroundId, Identity, RelationKind, SignError,
    Signature, Signer,
};
use serde::{Deserialize, Serialize};

use crate::wire::{Refused, Timed};

/// The keys a server signs with, by the credential each is bound to.
pub struct Keys {
    /// The keys it signs with, by credential.
    signing: HashMap<String, Identity>,
    /// Retired keys (`<dir>/retired/`), never signed with, read only so content holding one is refused
    /// too (finding 6f54cb439f9b).
    retired: Vec<Identity>,
}

impl Keys {
    /// Reads every `<name>.json` identity file directly in `dir`, binding it to credential `<name>`, and
    /// those in `dir/retired/`, if there is one, for the content check only. The directory must be private
    /// (no access for group or others), and each file's name its identity's.
    ///
    /// A key is retired if it's in `dir/retired/` or its public key is in `dir/retired.jsonl` (one
    /// `{"name", "agent"}` per line, which `team-server retire` appends): one found in `dir` as well, put
    /// back by hand or from a backup, is never signed with (finding ab071f5d3de6).
    pub fn read(dir: &Path) -> Result<Keys> {
        let mut signing = Self::read_dir(dir)?;
        let retired_dir = dir.join("retired");
        let mut retired: Vec<Identity> = match fs::symlink_metadata(&retired_dir) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            _ => Self::read_dir(&retired_dir)?.into_values().collect(),
        };
        let mut ids: HashSet<AgentId> = retired.iter().map(Identity::agent_id).collect();
        ids.extend(Self::read_retired_list(&dir.join("retired.jsonl"))?);
        // A retired key found in `dir` isn't signed with, but stays the server's for the content check, so
        // content holding it is still refused (finding 7794d137682f).
        let names: Vec<String> = signing.keys().cloned().collect();
        for name in names {
            if ids.contains(&signing[&name].agent_id()) {
                eprintln!("bracken serve: not signing for {name}: its key is retired");
                retired.extend(signing.remove(&name));
            }
        }
        Ok(Keys { signing, retired })
    }

    /// The public keys `retired.jsonl` lists, if it's there.
    fn read_retired_list(path: &Path) -> Result<Vec<AgentId>> {
        #[derive(Deserialize)]
        struct Retired {
            agent: AgentId,
        }
        let text = match fs::read_to_string(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            text => text.with_context(|| format!("reading {}", path.display()))?,
        };
        let mut ids = Vec::new();
        for (n, line) in text.lines().enumerate().filter(|(_, l)| !l.trim().is_empty()) {
            let r: Retired = serde_json::from_str(line)
                .with_context(|| format!("{}:{}: not a retired key's entry", path.display(), n + 1))?;
            ids.push(r.agent);
        }
        Ok(ids)
    }

    /// The name and key of a key this server holds, retired or not, whose private key is in `bytes`.
    fn held_key_in(&self, bytes: &[u8]) -> Option<&str> {
        self.signing.values().chain(&self.retired).find(|k| k.key_is_in(bytes)).map(Identity::name)
    }

    fn read_dir(dir: &Path) -> Result<HashMap<String, Identity>> {
        let m = fs::symlink_metadata(dir).with_context(|| format!("reading {}", dir.display()))?;
        if !m.is_dir() {
            bail!("{} isn't a directory", dir.display());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if m.permissions().mode() & 0o077 != 0 {
                bail!("{} is open to other users; make it 0700", dir.display());
            }
        }
        let mut keys = HashMap::new();
        for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
            let path = entry.with_context(|| format!("reading {}", dir.display()))?.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".json")) else {
                continue;
            };
            if !fs::symlink_metadata(&path).is_ok_and(|m| m.is_file()) {
                bail!("{} isn't a regular file", path.display());
            }
            let identity = Identity::load(&path)?;
            if identity.name() != name {
                bail!("{} holds identity {:?}, not {name:?}", path.display(), identity.name());
            }
            keys.insert(name.to_owned(), identity);
        }
        Ok(keys)
    }
}

/// `scopes.toml`: each server-held identity's name, or a pattern ending in `*` (`builder-*`), and what it
/// may sign. An exact name wins over a pattern, and a longer pattern over a shorter one.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScopeFile {
    #[serde(default)]
    scope: BTreeMap<String, Scope>,
}

/// What one identity may sign. Every list is what's allowed, and an absent one allows nothing, except the
/// tags: `tags`, if given, lists the only tags a claim may carry, and `forbidden_tags` those it may not.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    #[serde(default)]
    kinds: Vec<BodyKind>,
    #[serde(default)]
    relations: Vec<RelationKind>,
    tags: Option<Vec<String>>,
    #[serde(default)]
    forbidden_tags: Vec<String>,
}

impl Scope {
    /// Why `body` is outside this scope, if it is.
    fn refuses(&self, body: &Body) -> Option<String> {
        let kind = body.kind();
        if !self.kinds.contains(&kind) {
            return Some(format!("its scope doesn't include {kind}s"));
        }
        match body {
            Body::Relation { kind, .. } if !self.relations.contains(kind) => {
                Some(format!("its scope doesn't include {kind} relations"))
            }
            Body::Claim { tags, .. } => tags.iter().find_map(|t| {
                let allowed = self.tags.as_ref().is_none_or(|a| a.contains(t)) && !self.forbidden_tags.contains(t);
                (!allowed).then(|| format!("its scope doesn't allow the tag {t:?}"))
            }),
            _ => None,
        }
    }
}

/// The scope `name` has in the scopes file's text.
pub fn scope_of(text: &str, name: &str) -> Result<Option<Scope>> {
    let mut file: ScopeFile = toml::from_str(text).context("parsing the scopes")?;
    for key in file.scope.keys() {
        let pattern = key.strip_suffix('*').unwrap_or(key);
        if pattern.contains('*')
            || !pattern.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-".contains(&b))
        {
            bail!("scope {key:?}: give a name, or a name's start followed by one '*'");
        }
    }
    if let Some(exact) = file.scope.remove(name) {
        return Ok(Some(exact));
    }
    let best = file
        .scope
        .into_iter()
        .filter_map(|(key, scope)| Some((key.strip_suffix('*')?.len(), key, scope)))
        .filter(|(_, key, _)| name.starts_with(&key[..key.len() - 1]))
        .max_by_key(|(len, _, _)| *len);
    Ok(best.map(|(_, _, scope)| scope))
}

/// One line of the signing log.
#[derive(Debug, Serialize, Deserialize)]
pub struct Signed {
    pub id: ContributionId,
    pub credential: String,
    pub signed_ms: u64,
}

/// A server's signing: its keys, the scopes file it reads on each request, and the log it appends to.
pub struct Signing {
    keys: Keys,
    scopes: PathBuf,
    log: Mutex<File>,
}

impl Signing {
    pub fn open(keys: Keys, scopes: &Path, log: &Path) -> Result<Signing> {
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let file = options.open(log).with_context(|| format!("opening the signing log {}", log.display()))?;
        Ok(Signing { keys, scopes: scopes.to_owned(), log: Mutex::new(file) })
    }

    /// The signature for `bytes`, a contribution's canonical content, by the key bound to `credential`.
    pub fn sign(&self, ground: GroundId, credential: &str, bytes: &[u8]) -> Result<Vec<u8>, Refused> {
        let forbidden = |reason: String| Refused::new(403, reason);
        let key = self.keys.signing.get(credential).ok_or_else(|| {
            forbidden(format!("this server holds no key for {credential}, so it signs nothing for it"))
        })?;
        let content = ContributionContent::decode(bytes)
            .map_err(|e| Refused::new(400, format!("the body isn't a contribution's content: {e}")))?;
        if content.ground != ground {
            return Err(forbidden(format!("the content is for Ground {}, not this server's", content.ground)));
        }
        if content.protocol != PROTOCOL_VERSION {
            return Err(forbidden(format!("the content is protocol {}, not {PROTOCOL_VERSION}", content.protocol)));
        }
        if content.author != key.agent_id() {
            return Err(forbidden(format!(
                "the content's author isn't {credential}'s key, the only one it may sign as"
            )));
        }
        if let Some(name) = self.keys.held_key_in(bytes) {
            return Err(forbidden(format!("the content holds the private key of {name}; it can't go into the Ground")));
        }
        let text = fs::read_to_string(&self.scopes).map_err(|e| {
            eprintln!("bracken serve: reading {}: {e}", self.scopes.display());
            forbidden("the server can't read its scopes, so it signs nothing".into())
        })?;
        let scope = scope_of(&text, credential).map_err(|e| {
            eprintln!("bracken serve: {}: {e:#}", self.scopes.display());
            forbidden("the server's scopes don't parse, so it signs nothing".into())
        })?;
        let scope =
            scope.ok_or_else(|| forbidden(format!("{credential} has no scope, so the server signs nothing for it")))?;
        if let Some(why) = scope.refuses(&content.body) {
            return Err(forbidden(format!("not signed for {credential}: {why}")));
        }
        let id = content.id();
        let signed_ms = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        let line = serde_json::to_string(&Signed { id, credential: credential.to_owned(), signed_ms })
            .expect("a signing log line serializes")
            + "\n";
        let mut log = self.log.lock().unwrap_or_else(|p| p.into_inner());
        log.write_all(line.as_bytes()).and_then(|()| log.sync_data()).map_err(|e| {
            eprintln!("bracken serve: writing the signing log: {e}");
            Refused::new(500, "the server couldn't write its signing log, so it signed nothing")
        })?;
        drop(log);
        Ok(key.sign(&id).0.to_vec())
    }
}

/// The longest a sign request may take, end to end.
const SIGN_TIMEOUT: Duration = Duration::from_secs(60);

/// A [`Signer`] whose key the team server holds: it sends the content to the server's sign request and
/// returns the signature, which [`manyroot::Contribution::create_with`] then checks.
pub struct RemoteSigner {
    pub url: String,
    pub token: String,
    pub ground: GroundId,
    pub agent: AgentId,
}

impl RemoteSigner {
    /// The signer for `agent` on the server at `url`, with the token in `token_file`.
    pub fn new(url: &str, token_file: &Path, ground: GroundId, agent: AgentId) -> Result<RemoteSigner> {
        let token = fs::read_to_string(token_file)
            .with_context(|| format!("reading the token file {}", token_file.display()))?
            .trim()
            .to_owned();
        if token.is_empty() || token.bytes().any(|b| b.is_ascii_control()) {
            bail!("the token in {} is empty or holds control characters", token_file.display());
        }
        crate::remote::loopback(url)?;
        Ok(RemoteSigner { url: url.to_owned(), token, ground, agent })
    }

    fn request(&self, content: &[u8]) -> Result<Signature> {
        let (host, port) = crate::remote::loopback(&self.url)?;
        let tcp = TcpStream::connect((host.as_str(), port)).with_context(|| format!("connecting to {}", self.url))?;
        let mut stream = Timed::new(&tcp, Instant::now() + SIGN_TIMEOUT, None);
        let head = format!(
            "POST /v1/{}/sign HTTP/1.1\r\nHost: localhost:{port}\r\nAuthorization: Bearer {}\r\n\
             Content-Type: application/cbor\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            self.ground,
            self.token,
            content.len()
        );
        stream.write_all(head.as_bytes())?;
        stream.write_all(content)?;
        stream.flush()?;
        let (status, length, early) = crate::remote::response_head(&mut stream)?;
        let mut reader = Cursor::new(early).chain(&mut stream).take(length.min(4096));
        let mut body = Vec::new();
        reader.read_to_end(&mut body).context("reading the server's answer")?;
        if status != 200 {
            bail!("the team server didn't sign it ({status}): {}", String::from_utf8_lossy(&body).trim());
        }
        let bytes: [u8; 64] = body.try_into().map_err(|_| anyhow::anyhow!("the server's signature isn't 64 bytes"))?;
        Ok(Signature(bytes))
    }
}

impl Signer for RemoteSigner {
    fn agent_id(&self) -> AgentId {
        self.agent
    }

    fn sign_content(&self, content: &ContributionContent, _: &ContributionId) -> Result<Signature, SignError> {
        let bytes = content.encode();
        if bytes.len() > MAX_CONTRIBUTION_BYTES {
            return Err(SignError(format!("the content is over the {MAX_CONTRIBUTION_BYTES} byte limit")));
        }
        self.request(&bytes).map_err(|e| SignError(format!("{e:#}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim(tags: &[&str]) -> Body {
        Body::Claim { statement: "s".into(), tags: tags.iter().map(|t| (*t).to_owned()).collect() }
    }

    #[test]
    fn a_scope_is_found_by_name_then_by_the_longest_pattern() {
        let text = r#"
            [scope."builder-*"]
            kinds = ["claim"]
            [scope."builder-keys"]
            kinds = ["evidence"]
            [scope."b*"]
            kinds = ["retract"]
        "#;
        let kinds = |name: &str| scope_of(text, name).unwrap().map(|s| s.kinds);
        assert_eq!(kinds("builder-keys"), Some(vec![BodyKind::Evidence]));
        assert_eq!(kinds("builder-x"), Some(vec![BodyKind::Claim]));
        assert_eq!(kinds("bob"), Some(vec![BodyKind::Retract]));
        assert_eq!(kinds("manager"), None);
        assert!(scope_of("[scope.\"a*b\"]\nkinds = []\n", "ab").is_err());
        assert!(scope_of("[scope.x]\nkinds = [\"claim\"]\nmistyped = 1\n", "x").is_err());
        assert!(scope_of("[scope.x]\nkinds = [\"claims\"]\n", "x").is_err());
    }

    #[test]
    fn a_scope_limits_kinds_relations_and_tags() {
        let text = r#"
            [scope.b]
            kinds = ["claim", "relation"]
            relations = ["implements"]
            forbidden_tags = ["decision"]
            [scope.t]
            kinds = ["claim"]
            tags = ["note"]
        "#;
        let b = scope_of(text, "b").unwrap().unwrap();
        assert_eq!(b.refuses(&claim(&["note", "pr"])), None);
        assert!(b.refuses(&claim(&["note", "decision"])).unwrap().contains("\"decision\""));
        let id = ContributionId([1; 32]);
        let relation = |kind| Body::Relation { from: id, kind, to: id, note: String::new() };
        assert_eq!(b.refuses(&relation(RelationKind::Implements)), None);
        assert!(b.refuses(&relation(RelationKind::Supersedes)).unwrap().contains("supersedes relations"));
        let retract = Body::Retract { target: id, reason: String::new() };
        assert!(b.refuses(&retract).unwrap().contains("retracts"));
        let t = scope_of(text, "t").unwrap().unwrap();
        assert_eq!(t.refuses(&claim(&["note"])), None);
        assert!(t.refuses(&claim(&["pr"])).is_some());
    }
}
