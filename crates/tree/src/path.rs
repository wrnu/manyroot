//! Paths, folding, collisions and symlink targets (docs/design/weave.md, *Unit:
//! a file, by path* and *Manifests*).
//!
//! A path names a file relative to a tree's root. These rules make every valid
//! path safe to write on macOS's APFS and HFS+ and on Windows' NTFS, as git's
//! `core.protectHFS` and `core.protectNTFS` do, and keep `.git` and every
//! `.bracken*` name out of a tree.

use icu_casemap::CaseMapper;
use unicode_normalization::UnicodeNormalization;

/// The longest path, in bytes, before folding.
pub const MAX_PATH_BYTES: usize = 4096;

/// The longest component, in bytes, before folding: what APFS, HFS+ and NTFS hold.
pub const MAX_COMPONENT_BYTES: usize = 255;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathError {
    #[error("the path is empty")]
    Empty,
    #[error("the path is {0} bytes, over the {MAX_PATH_BYTES} byte limit")]
    TooLong(usize),
    #[error("a component is {0} bytes, over the {MAX_COMPONENT_BYTES} byte limit")]
    ComponentTooLong(usize),
    #[error("the path starts with '/'")]
    Absolute,
    #[error("the path ends with '/'")]
    TrailingSlash,
    #[error("the path holds a NUL")]
    Nul,
    #[error("the path holds a blank component")]
    BlankComponent,
    #[error("the path holds a '.' or '..' component, or one that NTFS reads as one: {0:?}")]
    DotComponent(String),
    #[error("the path holds a backslash, which NTFS reads as a separator")]
    Backslash,
    #[error("the path holds a ':', which NTFS reads as a drive or a stream")]
    Colon,
    #[error("the component {0:?} names .git or a .bracken entry, after folding")]
    Reserved(String),
    #[error("a symlink target is not UTF-8")]
    NotUtf8,
}

/// Checks a path against every rule: UTF-8 (by type), `/` as the separator, not
/// empty, at most [`MAX_PATH_BYTES`] bytes, no leading or trailing `/`, no NUL,
/// and every component valid. The length limits are checked
/// before anything is folded, so checking a path costs time and memory linear in
/// its length.
pub fn validate_path(path: &str) -> Result<(), PathError> {
    if path.is_empty() {
        return Err(PathError::Empty);
    }
    if path.len() > MAX_PATH_BYTES {
        return Err(PathError::TooLong(path.len()));
    }
    if let Some(long) = path.split('/').find(|c| c.len() > MAX_COMPONENT_BYTES) {
        return Err(PathError::ComponentTooLong(long.len()));
    }
    if path.starts_with('/') {
        return Err(PathError::Absolute);
    }
    if path.ends_with('/') {
        return Err(PathError::TrailingSlash);
    }
    if path.contains('\0') {
        return Err(PathError::Nul);
    }
    path.split('/').try_for_each(validate_component)
}

/// Checks one component:
/// - not empty, and not made only of `White_Space` characters (protocol.md, rule 8),
///   before or after folding;
/// - not `.` or `..`, nor a name NTFS or folding reads as one (only dots and spaces);
/// - no backslash, and no `:`. Windows reads a name before `:` as a drive (`C:x`,
///   `1:x`, or any letter at all, git's CVE-2019-1351) and a name after it as a
///   stream (`.git:x`), and NTFS can't hold one in a name;
/// - after folding, with NTFS's trailing dots and spaces dropped, not `.git`, not starting with `.bracken`, and not an 8.3 short name
///   of either (`git~N`, `bracke~N`).
pub fn validate_component(component: &str) -> Result<(), PathError> {
    if component.trim().is_empty() {
        return Err(PathError::BlankComponent);
    }
    if component.contains('\0') {
        return Err(PathError::Nul);
    }
    if component.contains('\\') {
        return Err(PathError::Backslash);
    }
    if component.contains(':') {
        return Err(PathError::Colon);
    }
    let folded = fold(component);
    // A component made only of ignorable code points folds to nothing.
    if folded.trim().is_empty() {
        return Err(PathError::BlankComponent);
    }
    if folded.chars().all(|c| c == '.' || c == ' ') {
        return Err(PathError::DotComponent(component.to_owned()));
    }
    let ntfs = folded.trim_end_matches(['.', ' ']);
    let reserved =
        ntfs == ".git" || folded.starts_with(".bracken") || is_short_name(ntfs, "git") || is_short_name(ntfs, "bracke");
    if reserved {
        return Err(PathError::Reserved(component.to_owned()));
    }
    Ok(())
}

/// `<stem>~<digits>`: an 8.3 short name NTFS may give a long name starting with `stem`.
fn is_short_name(name: &str, stem: &str) -> bool {
    name.strip_prefix(stem)
        .and_then(|rest| rest.strip_prefix('~'))
        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
}

/// Folds a path or component the way case-insensitive, normalization-insensitive
/// file systems compare names. It drops default-ignorable code points (as git's
/// `protectHFS` ignores them), then takes Unicode's canonical caseless match form
/// (definition D145): `NFD(casefold(NFD(s)))`, with full case folding. Case folding
/// can leave marks out of canonical order, or split a composed letter, so the
/// second NFD is what makes canonically equivalent names fold alike. Two paths
/// collide when their folds are equal.
pub fn fold(s: &str) -> String {
    let nfd: String = s.chars().filter(|&c| !is_default_ignorable(c)).nfd().collect();
    CaseMapper::new().fold_string(&nfd).nfd().collect()
}

/// Unicode's `Default_Ignorable_Code_Point` property (DerivedCoreProperties.txt,
/// unchanged since Unicode 6.0 apart from U+180F, added in 14.0).
pub fn is_default_ignorable(c: char) -> bool {
    matches!(c as u32,
        0x00AD | 0x034F | 0x061C | 0x115F..=0x1160 | 0x17B4..=0x17B5 | 0x180B..=0x180F
        | 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x206F | 0x3164 | 0xFE00..=0xFE0F
        | 0xFEFF | 0xFFA0 | 0xFFF0..=0xFFF8 | 0x1BCA0..=0x1BCA3 | 0x1D173..=0x1D17A
        | 0xE0000..=0xE0FFF)
}

/// Checks a symlink's target, which is its file's bytes: UTF-8, relative, not
/// empty, no NUL, and every component valid. So a target never has `.` or `..`,
/// and only ever points down from the symlink's own directory.
pub fn validate_symlink_target(target: &[u8]) -> Result<(), PathError> {
    let target = std::str::from_utf8(target).map_err(|_| PathError::NotUtf8)?;
    validate_path(target)
}

/// Two paths of one tree that a case-insensitive file system can't hold together.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Collision {
    #[error("{0:?} and {1:?} are the same path after folding")]
    Equal(String, String),
    #[error("{0:?} is a file, and a directory of {1:?} after folding")]
    Prefix(String, String),
}

/// Finds a collision among `paths`: two paths equal after folding (named in the
/// order given), or a path that is, after folding, a directory of another. Each
/// path is folded once and nothing else is copied: the folds are sorted, so equal
/// ones are neighbours, and the paths under a directory `d` are the contiguous run
/// that starts with `d/`. Validate the paths first, so their length is bounded.
pub fn find_collision<'a>(paths: impl IntoIterator<Item = &'a str>) -> Option<Collision> {
    let mut folded: Vec<(String, usize, &str)> =
        paths.into_iter().enumerate().map(|(i, path)| (fold(path), i, path)).collect();
    folded.sort();
    if let Some(pair) = folded.windows(2).find(|pair| pair[0].0 == pair[1].0) {
        return Some(Collision::Equal(pair[0].2.to_owned(), pair[1].2.to_owned()));
    }
    for (fold, _, path) in &folded {
        let dir = format!("{fold}/");
        let first_under = folded.partition_point(|(other, _, _)| other.as_str() < dir.as_str());
        if let Some((other, _, under)) = folded.get(first_under)
            && other.starts_with(&dir)
        {
            return Some(Collision::Prefix((*path).to_owned(), (*under).to_owned()));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_names() {
        assert!(is_short_name("git~1", "git"));
        assert!(is_short_name("bracke~12", "bracke"));
        assert!(!is_short_name("git~", "git"));
        assert!(!is_short_name("git~a", "git"));
        assert!(!is_short_name("gitx~1", "git"));
    }
}
