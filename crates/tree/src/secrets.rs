//! What import refuses because it looks like a secret (docs/design/weave.md,
//! *What import takes*): paths on the deny-list, and content holding key
//! material. The content scan matches key material, not words: a file that only
//! mentions `secret_key` passes.
//!
//! Keys read here are kept in memory for the comparison and never printed: a
//! [`Refusal`] names the path and the rule, never the bytes that matched.

use std::fmt;
use std::fs;
use std::io;
use std::path::Path;

/// Why a path or its content is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretRule {
    /// A component under an `identities/` directory.
    IdentitiesDir,
    /// A basename `.env`, or starting with `.env.`.
    EnvFile,
    /// A basename ending in `.pem`, `.key`, `.p12` or `.pfx`.
    KeyFileExtension,
    /// A basename `id_rsa`, `id_dsa`, `id_ecdsa` or `id_ed25519`, with or without a suffix.
    SshKeyName,
    /// Content holding a PEM private-key header.
    PemPrivateKey,
    /// Content holding an armored PGP private key block.
    PgpPrivateKey,
    /// Content holding an RFC 4716 (SSH2) private key header.
    Ssh2PrivateKey,
    /// Content holding a PuTTY private key file header.
    PuttyPrivateKey,
    /// Content holding a `secret_key` field with a 64-hex-digit value: JSON,
    /// YAML or TOML, quoted or not.
    SecretKeyField,
    /// Content holding the key of a known identity, in hex or raw.
    IdentityKey { identity: String },
}

impl fmt::Display for SecretRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SecretRule::IdentitiesDir => f.write_str("it is under an identities/ directory"),
            SecretRule::EnvFile => f.write_str("its name is .env or starts with .env."),
            SecretRule::KeyFileExtension => f.write_str("its name ends in .pem, .key, .p12 or .pfx"),
            SecretRule::SshKeyName => f.write_str("its name is an SSH private key's"),
            SecretRule::PemPrivateKey => f.write_str("it holds a PEM private-key header"),
            SecretRule::PgpPrivateKey => f.write_str("it holds an armored PGP private key block"),
            SecretRule::Ssh2PrivateKey => f.write_str("it holds an SSH2 private-key header"),
            SecretRule::PuttyPrivateKey => f.write_str("it holds a PuTTY private-key header"),
            SecretRule::SecretKeyField => f.write_str("it holds a secret_key field with a 64-hex-digit value"),
            SecretRule::IdentityKey { identity } => write!(f, "it holds the private key of identity {identity:?}"),
        }
    }
}

/// Checks a path against the deny-list. Names are compared without regard to
/// ASCII case, since macOS and Windows don't regard it either.
pub fn deny_listed(path: &str) -> Option<SecretRule> {
    let lower = path.to_ascii_lowercase();
    let components: Vec<&str> = lower.split('/').collect();
    let (basename, dirs) = components.split_last().expect("split always yields one part");
    if dirs.contains(&"identities") {
        return Some(SecretRule::IdentitiesDir);
    }
    if *basename == ".env" || basename.starts_with(".env.") {
        return Some(SecretRule::EnvFile);
    }
    if [".pem", ".key", ".p12", ".pfx"].iter().any(|ext| basename.ends_with(ext)) {
        return Some(SecretRule::KeyFileExtension);
    }
    if ["id_rsa", "id_dsa", "id_ecdsa", "id_ed25519"].iter().any(|key| basename.starts_with(key)) {
        return Some(SecretRule::SshKeyName);
    }
    None
}

/// The private keys of known identities, to refuse content holding one.
#[derive(Default, Clone)]
pub struct KnownKeys {
    /// (identity name, key as lowercase hex, key as bytes)
    keys: Vec<(String, String, Vec<u8>)>,
}

impl fmt::Debug for KnownKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<&str> = self.keys.iter().map(|(name, _, _)| name.as_str()).collect();
        f.debug_struct("KnownKeys").field("identities", &names).finish_non_exhaustive()
    }
}

impl KnownKeys {
    pub fn new() -> KnownKeys {
        KnownKeys::default()
    }

    /// Adds every identity file (`*.json`) in a Ground's `identities/` directory.
    /// A directory that doesn't exist adds nothing.
    pub fn add_identities_dir(&mut self, dir: &Path) -> io::Result<()> {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        for entry in entries {
            let path = entry?.path();
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            let text = fs::read(&path)?;
            if let Some(hex) = secret_key_field(&text) {
                let name = path.file_stem().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                self.add(name, &hex);
            }
        }
        Ok(())
    }

    /// Adds one identity's key, given as hex.
    pub fn add(&mut self, identity: String, hex: &str) {
        let hex = hex.to_ascii_lowercase();
        let raw = (0..hex.len() / 2).filter_map(|i| u8::from_str_radix(hex.get(2 * i..2 * i + 2)?, 16).ok()).collect();
        self.keys.push((identity, hex, raw));
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

/// Scans content for key material.
pub fn scan(content: &[u8], known: &KnownKeys) -> Option<SecretRule> {
    if let Some(rule) = armored_private_key(content) {
        return Some(rule);
    }
    if has_putty_header(content) {
        return Some(SecretRule::PuttyPrivateKey);
    }
    if secret_key_field(content).is_some() {
        return Some(SecretRule::SecretKeyField);
    }
    if !known.keys.is_empty() {
        let lower = content.to_ascii_lowercase();
        for (identity, hex, raw) in &known.keys {
            if contains(&lower, hex.as_bytes()) || contains(content, raw) {
                return Some(SecretRule::IdentityKey { identity: identity.clone() });
            }
        }
    }
    None
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

/// The label after `begin`, as real armor has it: capital letters, digits and
/// spaces. Prose that quotes a header with an ellipsis, as weave.md does, has
/// no such label.
fn labels<'a>(content: &'a [u8], begin: &'a [u8]) -> impl Iterator<Item = (&'a [u8], &'a [u8])> + 'a {
    let mut rest = content;
    std::iter::from_fn(move || {
        let at = find(rest, begin)?;
        let after = &rest[at + begin.len()..];
        let len = after.iter().take_while(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || **b == b' ').count();
        rest = after;
        Some((&after[..len], &after[len..]))
    })
}

/// An armored private key:
/// - PEM, `-----BEGIN <label>-----` with the label ending in `PRIVATE KEY`
///   (PKCS#8, RSA, EC, DSA, OpenSSH, encrypted);
/// - PGP, the same with the label ending in `PRIVATE KEY BLOCK`;
/// - RFC 4716 (SSH2), `---- BEGIN <label> ----` with the label ending in
///   `PRIVATE KEY`.
fn armored_private_key(content: &[u8]) -> Option<SecretRule> {
    for (label, after) in labels(content, b"-----BEGIN ") {
        if after.starts_with(b"-----") {
            if label.ends_with(b"PRIVATE KEY BLOCK") {
                return Some(SecretRule::PgpPrivateKey);
            }
            if label.ends_with(b"PRIVATE KEY") {
                return Some(SecretRule::PemPrivateKey);
            }
        }
    }
    for (label, after) in labels(content, b"---- BEGIN ") {
        if label.ends_with(b"PRIVATE KEY ") && after.starts_with(b"----") {
            return Some(SecretRule::Ssh2PrivateKey);
        }
    }
    None
}

/// A PuTTY key file's first line: `PuTTY-User-Key-File-<digits>:`, at the start
/// of a line.
fn has_putty_header(content: &[u8]) -> bool {
    const HEADER: &[u8] = b"PuTTY-User-Key-File-";
    content.split(|&b| b == b'\n').any(|line| {
        line.strip_prefix(HEADER).is_some_and(|rest| {
            let digits = rest.iter().take_while(|b| b.is_ascii_digit()).count();
            digits > 0 && rest[digits..].starts_with(b":")
        })
    })
}

/// The value of a `secret_key` field holding exactly 64 hex digits:
/// - the key, alone as a word, quoted or not: `"secret_key"` (JSON, a Bracken
///   identity file), `secret_key` (YAML, TOML);
/// - optional whitespace, then `:` or `=`, then optional whitespace;
/// - the value, in double or single quotes or bare, and nothing hex-like right
///   after it.
fn secret_key_field(content: &[u8]) -> Option<String> {
    const KEY: &[u8] = b"secret_key";
    let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut from = 0;
    while let Some(at) = find(&content[from..], KEY) {
        let start = from + at;
        let end = start + KEY.len();
        from = end;
        if start > 0 && word(content[start - 1]) {
            continue;
        }
        let mut s = content[end..].strip_prefix(b"\"").unwrap_or(&content[end..]);
        if s.first().copied().is_some_and(word) {
            continue;
        }
        let skip_space = |s: &[u8]| -> usize { s.iter().take_while(|b| b.is_ascii_whitespace()).count() };
        s = &s[skip_space(s)..];
        let Some(rest) = s.strip_prefix(b":").or_else(|| s.strip_prefix(b"=")) else { continue };
        s = &rest[skip_space(rest)..];
        let quote = s.first().copied().filter(|q| *q == b'"' || *q == b'\'');
        if quote.is_some() {
            s = &s[1..];
        }
        let Some(value) = s.get(..64) else { continue };
        let next = s.get(64).copied();
        let closed = match quote {
            Some(q) => next == Some(q),
            None => !next.is_some_and(word),
        };
        if value.iter().all(u8::is_ascii_hexdigit) && closed {
            return Some(String::from_utf8(value.to_vec()).expect("hex digits are ASCII"));
        }
    }
    None
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_deny_list_entry_is_refused() {
        let cases = [
            (".bracken-x/identities/manager.json", SecretRule::IdentitiesDir),
            ("identities/a", SecretRule::IdentitiesDir),
            ("a/Identities/b/c", SecretRule::IdentitiesDir),
            (".env", SecretRule::EnvFile),
            ("app/.env.local", SecretRule::EnvFile),
            (".ENV", SecretRule::EnvFile),
            ("certs/server.pem", SecretRule::KeyFileExtension),
            ("tls.key", SecretRule::KeyFileExtension),
            ("a/store.p12", SecretRule::KeyFileExtension),
            ("a/store.PFX", SecretRule::KeyFileExtension),
            ("id_rsa", SecretRule::SshKeyName),
            ("home/.ssh/id_dsa", SecretRule::SshKeyName),
            ("id_ecdsa_old", SecretRule::SshKeyName),
            ("id_ed25519.pub", SecretRule::SshKeyName),
        ];
        for (path, rule) in cases {
            assert_eq!(deny_listed(path), Some(rule), "{path:?}");
        }
        for path in
            ["identities", "src/identities.rs", "docs/identities", ".envrc", "env", "keys.md", "pem/x", "my_id_rsa"]
        {
            assert_eq!(deny_listed(path), None, "{path:?}");
        }
    }

    #[test]
    fn key_material_is_found_and_words_are_not() {
        let none = KnownKeys::new();
        // Built at runtime, so this file holds no header and the repository
        // still imports cleanly.
        let header = |label: &str| format!("-----BEGIN {label}PRIVATE KEY-----");
        for pem in [
            format!("{}\nMII", header("")),
            format!("x\n{}", header("RSA ")),
            header("OPENSSH "),
            header("ENCRYPTED "),
            header("EC "),
        ] {
            assert_eq!(scan(pem.as_bytes(), &none), Some(SecretRule::PemPrivateKey), "{pem:?}");
        }
        let hex = "0123456789abcdef".repeat(4);
        for field in
            [format!("{{\"secret_key\":\"{hex}\"}}"), format!("\"secret_key\" :\n  \"{}\"", hex.to_uppercase())]
        {
            assert_eq!(scan(field.as_bytes(), &none), Some(SecretRule::SecretKeyField), "{field:?}");
        }
        for prose in [
            "the secret_key field".to_owned(),
            "-----BEGIN ... PRIVATE KEY-----".to_owned(),
            "-----BEGIN PUBLIC KEY-----".to_owned(),
            "-----BEGIN private key-----".to_owned(),
            format!("\"secret_key\": \"{}\"", &hex[..63]),
            format!("\"secret_key\": \"{hex}0\""),
            "file[\"secret_key\"].as_str()".to_owned(),
        ] {
            assert_eq!(scan(prose.as_bytes(), &none), None, "{prose:?}");
        }
    }

    // Finding ecfbb7d52203. Every vector is built at runtime, so this file holds
    // none and the repository still imports cleanly (gotcha 5a3dda0b1f93).
    #[test]
    fn pgp_ssh2_putty_and_yaml_toml_keys_are_found() {
        let none = KnownKeys::new();
        let hex = "0123456789abcdef".repeat(4);
        let cases = [
            (format!("-----BEGIN PGP {}-----\nVersion: x", "PRIVATE KEY BLOCK"), SecretRule::PgpPrivateKey),
            (format!("x\n---- BEGIN SSH2 {} ----\n", "ENCRYPTED PRIVATE KEY"), SecretRule::Ssh2PrivateKey),
            (format!("---- BEGIN SSH2 {} ----", "PRIVATE KEY"), SecretRule::Ssh2PrivateKey),
            (format!("{}-{}: ssh-ed25519\nEncryption: none", "PuTTY-User-Key-File", 3), SecretRule::PuttyPrivateKey),
            (format!("x\n{}-{}: ssh-rsa", "PuTTY-User-Key-File", 2), SecretRule::PuttyPrivateKey),
            (format!("identity:\n  {}: {hex}\n", "secret_key"), SecretRule::SecretKeyField),
            (format!("{} = \"{hex}\"\n", "secret_key"), SecretRule::SecretKeyField),
            (format!("{}='{}'", "secret_key", hex.to_uppercase()), SecretRule::SecretKeyField),
            (format!("{}:{hex}", "secret_key"), SecretRule::SecretKeyField),
        ];
        for (content, rule) in cases {
            assert_eq!(scan(content.as_bytes(), &none), Some(rule), "{content:?}");
        }
        for prose in [
            format!("-----BEGIN PGP {}-----", "PUBLIC KEY BLOCK"),
            format!("-----BEGIN PGP ... {}-----", "PRIVATE KEY BLOCK"),
            format!("---- BEGIN SSH2 {} ----", "PUBLIC KEY"),
            format!("---- BEGIN ... {} ----", "PRIVATE KEY"),
            format!("see {}-<n>: in the spec", "PuTTY-User-Key-File"),
            format!("{}-: x", "PuTTY-User-Key-File"),
            format!("my_{}: {hex}", "secret_key"),
            format!("{}_old: {hex}", "secret_key"),
            format!("{}: {hex}0", "secret_key"),
            format!("{}: \"{hex}", "secret_key"),
            format!("{}: String", "secret_key"),
            format!("{}: hex::encode(key)", "secret_key"),
        ] {
            assert_eq!(scan(prose.as_bytes(), &none), None, "{prose:?}");
        }
    }

    #[test]
    fn a_known_identity_key_is_found_in_hex_or_raw() {
        let hex = "a1b2c3d4".repeat(8);
        let mut known = KnownKeys::new();
        known.add("weave-integrator".to_owned(), &hex);
        let found = SecretRule::IdentityKey { identity: "weave-integrator".to_owned() };
        let rule = Some(found.clone());
        assert_eq!(scan(format!("key = {hex}\n").as_bytes(), &known), rule);
        assert_eq!(scan(format!("KEY={}", hex.to_uppercase()).as_bytes(), &known), rule);
        let raw: Vec<u8> = (0..32).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).expect("hex")).collect();
        let mut binary = b"\x00\x01".to_vec();
        binary.extend(&raw);
        assert_eq!(scan(&binary, &known), rule);
        assert_eq!(scan(b"nothing here", &known), None);
        // The refusal names the identity, never the key.
        let shown = format!("{found} {known:?}");
        assert!(!shown.contains(&hex), "{shown}");
    }
}
