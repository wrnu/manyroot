//! What agents are told (design section 3, Prompts; decision `4cf00e1d701d`):
//! one base prompt for every mode, a short "Where things are" section per
//! mode, of the same shape in each, and the mode's tool descriptions. The
//! files are in `crates/experiment/prompts/`, built into the binary, and
//! hashed for the run's plan, so a run's prompts can't drift from what was
//! reviewed.

/// The prompt every mode shares.
pub const BASE: &str = include_str!("../prompts/base.md");

/// A mode's "Where things are" section.
pub fn section(mode: &str) -> Option<&'static str> {
    match mode {
        "bracken" => Some(include_str!("../prompts/bracken.md")),
        "git" => Some(include_str!("../prompts/git.md")),
        "crdt" => Some(include_str!("../prompts/crdt.md")),
        _ => None,
    }
}

/// The system prompt for `mode`: the base prompt, then the mode's section.
pub fn system_prompt(mode: &str) -> Result<String, String> {
    let section = section(mode).ok_or_else(|| format!("no prompt for mode {mode}"))?;
    Ok(format!("{BASE}\n{section}"))
}

/// The first message of each turn: which turn of how many, and who the agent is.
pub fn turn_message(turn: u32, turns: u32, name: &str) -> String {
    crate::claude::TURN_MESSAGE
        .replace("{turns}", &turns.to_string())
        .replace("{turn}", &turn.to_string())
        .replace("{name}", name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mode::mode_named;

    #[test]
    fn every_mode_has_its_own_prompt() {
        let mut seen = std::collections::BTreeSet::new();
        for name in ["bracken", "git", "crdt"] {
            let prompt = system_prompt(name).expect("a prompt");
            assert!(prompt.starts_with(BASE) && prompt.ends_with(section(name).expect("a section")));
            assert!(seen.insert(prompt), "{name}'s prompt is its own");
        }
        assert!(system_prompt("other").is_err());
        assert_eq!(
            turn_message(3, 8, "agent-1"),
            "This is your turn 3 of 8. You are agent-1. Start by reading your notes."
        );
    }

    /// The sections have one shape: the same heading and four sentences in the same order, of
    /// similar length (decision `4cf00e1d701d`).
    #[test]
    fn the_mode_sections_have_one_shape() {
        let sentences = |name: &str| -> Vec<String> {
            let text = section(name).expect("a section");
            let body = text.strip_prefix("## Where things are\n\n").expect("the heading");
            body.trim().split(". ").map(str::to_string).collect()
        };
        let words = |name: &str| -> usize { sentences(name).iter().map(|s| s.split_whitespace().count()).sum() };
        let (low, high) =
            ["bracken", "git", "crdt"].iter().map(|m| words(m)).fold((usize::MAX, 0), |(l, h), w| (l.min(w), h.max(w)));
        assert!(high * 10 <= low * 12, "within 20% of each other: {low} to {high} words");
        for name in ["bracken", "git", "crdt"] {
            let s = sentences(name);
            assert_eq!(s.len(), 4, "{name}: four sentences");
            assert!(s[0].starts_with("The task is"), "{name}: where the task is");
            assert!(s[1].starts_with("A change to the task arrives as"), "{name}: where changes appear");
            assert!(s[2].starts_with("The library counts once it is"), "{name}: what integrated means");
            assert_eq!(s[3], "Others' work reaches you, and yours reaches them, only when you sync.", "{name}");
        }
    }

    /// Each mode names its shared channel once, in one tool description, in the same sentence form
    /// (finding `6436d068437e`), and the prompts name none.
    #[test]
    fn each_mode_names_its_shared_channel_once_alike() {
        let phrase = " where agents talk to each other: everyone can read ";
        for name in ["bracken", "git", "crdt"] {
            let tools = mode_named(name).expect("a mode").tools();
            let naming: Vec<_> = tools.iter().filter(|t| t.description.contains(phrase)).collect();
            assert_eq!(naming.len(), 1, "{name}: one tool names the channel");
            let reader = naming[0].description.rsplit("with ").next().expect("a tool");
            assert!(tools.iter().any(|t| t.name == reader), "{name}: read with one of its tools, not {reader}");
            assert!(!section(name).expect("a section").contains(phrase), "{name}'s section names none");
        }
        assert!(!BASE.contains(phrase));
    }

    /// Nothing tells an agent it is measured, or that other modes exist (design section 4).
    #[test]
    fn the_prompts_never_mention_the_experiment() {
        let mut texts = vec![BASE.to_string(), turn_message(1, 8, "agent-0")];
        for name in ["bracken", "git", "crdt"] {
            texts.push(section(name).expect("a section").to_string());
            let mode = mode_named(name).expect("a mode");
            texts.push(serde_json::to_string(&mode.tools()).expect("JSON"));
        }
        let all = texts.join("\n").to_lowercase();
        for word in ["experiment", "metric", "measure", "token", "score", "grader", "hidden", "mode", "crdt", "bracken"]
        {
            assert!(!all.contains(word), "a prompt mentions {word:?}");
        }
    }
}
