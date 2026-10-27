//! The rules every agent-chosen value is checked against before anything runs
//! (design section 3, Arguments). A value that breaks a rule is refused with an
//! error result, and no process starts.

use serde_json::{Map, Value};

use crate::task::spec_named;

/// The most bytes of free text in one value: statements, notes, details, file contents.
pub const MAX_TEXT: usize = 16 * 1024;
/// The most characters of an agent's private notes.
pub const MAX_NOTES: usize = 4000;
/// The most tags on one claim.
pub const MAX_TAGS: usize = 8;

/// The arguments of one tool call: a JSON object.
pub struct Args<'a> {
    map: &'a Map<String, Value>,
    /// Off only in tests that show the argument placement alone keeps values from being options.
    checked: bool,
}

impl<'a> Args<'a> {
    /// The call's arguments, which must be an object holding only `allowed` keys.
    pub fn new(value: &'a Value, allowed: &[&str]) -> Result<Args<'a>, String> {
        let map = value.as_object().ok_or("arguments must be a JSON object")?;
        if let Some(k) = map.keys().find(|k| !allowed.contains(&k.as_str())) {
            return Err(format!("unknown argument {}", short(k)));
        }
        Ok(Args { map, checked: true })
    }

    /// The same, with every rule check off: strings pass through as they are.
    #[cfg(test)]
    pub(crate) fn unchecked(value: &'a Value) -> Result<Args<'a>, String> {
        Ok(Args { map: value.as_object().ok_or("arguments must be a JSON object")?, checked: false })
    }

    fn get(&self, key: &str) -> Option<&'a Value> {
        self.map.get(key)
    }

    /// `v` if `check` allows it, or if checks are off.
    fn ruled(&self, key: &str, v: &'a str, check: Result<(), String>) -> Result<&'a str, String> {
        match check {
            Err(e) if self.checked => Err(format!("`{key}`: {e}")),
            _ => Ok(v),
        }
    }

    fn str_opt(&self, key: &str) -> Result<Option<&'a str>, String> {
        match self.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) => Ok(Some(s)),
            Some(_) => Err(format!("`{key}` must be a string")),
        }
    }

    fn str(&self, key: &str) -> Result<&'a str, String> {
        self.str_opt(key)?.ok_or_else(|| format!("`{key}` is required"))
    }

    /// A contribution id: 12 to 64 lowercase hex digits.
    pub fn id(&self, key: &str) -> Result<&'a str, String> {
        let v = self.str(key)?;
        self.ruled(key, v, check_id(v))
    }

    /// A list of ids.
    pub fn ids(&self, key: &str) -> Result<Vec<&'a str>, String> {
        self.list(key, MAX_TAGS)?.into_iter().map(|v| self.ruled(key, v, check_id(v))).collect()
    }

    /// Free text: at most [`MAX_TEXT`] bytes, no NUL.
    pub fn text(&self, key: &str) -> Result<&'a str, String> {
        let v = self.str(key)?;
        self.ruled(key, v, check_text(v, MAX_TEXT))
    }

    pub fn text_opt(&self, key: &str) -> Result<&'a str, String> {
        match self.str_opt(key)? {
            None => Ok(""),
            Some(v) => self.ruled(key, v, check_text(v, MAX_TEXT)),
        }
    }

    /// One of `choices`.
    pub fn choice(&self, key: &str, choices: &[&'static str]) -> Result<&'a str, String> {
        let v = self.str(key)?;
        let check = if choices.contains(&v) { Ok(()) } else { Err(format!("must be one of {}", choices.join(", "))) };
        self.ruled(key, v, check)
    }

    pub fn choice_opt(&self, key: &str, choices: &[&'static str]) -> Result<Option<&'a str>, String> {
        match self.str_opt(key)? {
            None => Ok(None),
            Some(_) => self.choice(key, choices).map(Some),
        }
    }

    /// The name of one of the task's functions.
    pub fn spec_name(&self, key: &str) -> Result<&'a str, String> {
        let v = self.str(key)?;
        let check = spec_named(v).map(drop).ok_or_else(|| format!("{} is not a function of the task", short(v)));
        self.ruled(key, v, check)
    }

    /// Tags: at most [`MAX_TAGS`], each `^[a-z][a-z0-9-]{0,31}$`.
    pub fn tags(&self, key: &str) -> Result<Vec<&'a str>, String> {
        self.list(key, MAX_TAGS)?
            .into_iter()
            .map(|t| {
                let check = if is_tag(t) { Ok(()) } else { Err(format!("{} is not a tag (a-z, 0-9 and -)", short(t))) };
                self.ruled(key, t, check)
            })
            .collect()
    }

    /// One tag, if given.
    pub fn tag_opt(&self, key: &str) -> Result<Option<&'a str>, String> {
        match self.str_opt(key)? {
            None => Ok(None),
            Some(t) => {
                let check = if is_tag(t) { Ok(()) } else { Err(format!("{} is not a tag (a-z, 0-9 and -)", short(t))) };
                self.ruled(key, t, check).map(Some)
            }
        }
    }

    /// An integer in `range`.
    pub fn int_opt(&self, key: &str, range: std::ops::RangeInclusive<i64>) -> Result<Option<i64>, String> {
        match self.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => match v.as_i64() {
                Some(n) if range.contains(&n) => Ok(Some(n)),
                _ => Err(format!("`{key}` must be an integer from {} to {}", range.start(), range.end())),
            },
        }
    }

    pub fn bool_opt(&self, key: &str) -> Result<bool, String> {
        match self.get(key) {
            None | Some(Value::Null) => Ok(false),
            Some(Value::Bool(b)) => Ok(*b),
            Some(_) => Err(format!("`{key}` must be true or false")),
        }
    }

    fn list(&self, key: &str, max: usize) -> Result<Vec<&'a str>, String> {
        match self.get(key) {
            None | Some(Value::Null) => Ok(Vec::new()),
            Some(Value::Array(items)) if items.len() <= max => {
                items.iter().map(|v| v.as_str().ok_or_else(|| format!("`{key}` must be a list of strings"))).collect()
            }
            Some(Value::Array(_)) => Err(format!("`{key}` holds more than {max} items")),
            Some(_) => Err(format!("`{key}` must be a list")),
        }
    }
}

pub fn check_id(v: &str) -> Result<(), String> {
    if (12..=64).contains(&v.len()) && v.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        Ok(())
    } else {
        Err(format!("{} is not an id (12 to 64 lowercase hex digits)", short(v)))
    }
}

pub fn check_text(v: &str, max: usize) -> Result<(), String> {
    if v.len() > max {
        Err(format!("{} bytes, over the limit of {max}", v.len()))
    } else if v.contains('\0') {
        Err("contains a NUL byte".into())
    } else {
        Ok(())
    }
}

pub fn is_tag(t: &str) -> bool {
    let mut chars = t.chars();
    t.len() <= 32
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// At most 40 characters of `v`, for an error message.
pub fn short(v: &str) -> String {
    match v.char_indices().nth(40) {
        Some((cut, _)) => format!("`{}...`", &v[..cut]),
        None => format!("`{v}`"),
    }
}
