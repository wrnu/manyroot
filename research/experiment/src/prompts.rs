//! What agents are told (design section 3, Prompts; decision `4cf00e1d701d`):
//! one base prompt for every mode, a short "Where things are" section per
//! mode, of the same shape in each, and the mode's tool descriptions. The
//! files are in `research/experiment/prompts/`, built into the binary, and
//! hashed for the run's plan, so a run's prompts can't drift from what was
//! reviewed.

/// The prompt every mode shares.
pub const BASE: &str = include_str!("../prompts/base.md");

/// The prompt of a project run (Weave mode), in place of [`BASE`]: a project, not a library of functions.
pub const PROJECT_BASE: &str = include_str!("../prompts/project.md");

/// A mode's "Where things are" section.
pub fn section(mode: &str) -> Option<&'static str> {
    match mode {
        "bracken" => Some(include_str!("../prompts/bracken.md")),
        "git" => Some(include_str!("../prompts/git.md")),
        "crdt" => Some(include_str!("../prompts/crdt.md")),
        "weave" => Some(include_str!("../prompts/weave.md")),
        crate::git_project::NAME => Some(include_str!("../prompts/git-project.md")),
        _ => None,
    }
}

/// The words of [`PROJECT_BASE`] that are Weave's, and git project mode's for them (task `6bd13d994a03`): its
/// base is Weave's in every other word.
pub const GIT_WORDS: [(&str, &str); 2] = [
    ("on the line everyone builds on", "on the main branch everyone builds on"),
    (
        "Before you integrate someone's proposal, look at what it changes.",
        "Before you merge someone's branch, look at what it changes.",
    ),
];

/// [`PROJECT_BASE`] in git project mode's words ([`GIT_WORDS`]).
pub fn git_project_base() -> String {
    GIT_WORDS.iter().fold(PROJECT_BASE.to_string(), |text, (weave, git)| text.replace(weave, git))
}

/// The system prompt for `mode`: the base prompt, then the mode's section. Weave mode's runs are of a
/// project, so its base is [`PROJECT_BASE`], and git project mode's is the same in git's words.
pub fn system_prompt(mode: &str) -> Result<String, String> {
    let section = section(mode).ok_or_else(|| format!("no prompt for mode {mode}"))?;
    let base = match mode {
        "weave" => PROJECT_BASE.to_string(),
        crate::git_project::NAME => git_project_base(),
        _ => BASE.to_string(),
    };
    Ok(format!("{base}\n{section}"))
}

/// The first message of each turn: which turn of how many, and who the agent is.
pub fn turn_message(turn: u32, turns: u32, name: &str) -> String {
    crate::backend::TURN_MESSAGE
        .replace("{turns}", &turns.to_string())
        .replace("{turn}", &turn.to_string())
        .replace("{name}", name)
}

/// The role sentence of the one integrator, after its turn message, when the run's integrator setting names one
/// agent (task `5b2f7f4a6b7f`); the plan hashes it.
pub const INTEGRATOR_ROLE: &str = "You are the run's one integrator: the other agents take the items and propose their \
     work, and only you integrate onto main, so review their proposals, integrate what's sound and settle any \
     conflict. You take no items yourself. You may write code, but only to reconcile a conflict or to repair \
     main. After each integration, its result shows main's public tests: while a test that passed on main before fails, \
     main is broken, and you repair it before you integrate anything else. Each of your turns starts once every \
     other agent has ended that turn, so in each integrate every proposal open then, and end the turn when none is \
     left. Your last turn starts only once every other agent's last turn has ended, so in it integrate every \
     proposal still open.";

/// The role sentence of every other agent then: `{integrator}` is the integrator's name. The plan hashes it.
pub const BUILDER_ROLE: &str = "{integrator} is the run's one integrator: propose your work, and {integrator} \
     integrates it onto main; integrate is refused to you. {integrator} integrates after each of your turns, so \
     propose each item's work as soon as it is done. {integrator}'s last turn comes after your last, so propose all \
     your work by the end of your last turn.";

/// Git project mode's [`INTEGRATOR_ROLE`], in git's words (task `6bd13d994a03`). The plan hashes it.
pub const GIT_INTEGRATOR_ROLE: &str = "You are the run's one integrator: the other agents take the items, push their \
     work on branches and ask for its integration, and only you update main, so review their requests, merge what's \
     sound into your main, settle any conflict, and sync. You take no items yourself. You may write code, but \
     only to reconcile a conflict or to repair main. After each integration, its result shows main's public tests: while a \
     test that passed on main before fails, main is broken, and you repair it before you integrate anything else. \
     Each of your turns starts once every other agent has ended that turn, so in each merge every request \
     open then, sync, and end the turn when none is left. Your last turn starts only once every other agent's last \
     turn has ended, so in it merge every request still open.";

/// Git project mode's [`BUILDER_ROLE`], in git's words. The plan hashes it.
pub const GIT_BUILDER_ROLE: &str = "{integrator} is the run's one integrator: push your work on a branch of your own \
     and ask for its integration, and {integrator} merges it into main; updating main is refused to you. \
     {integrator} merges after each of your turns, so ask for each item's integration as soon as its work is done. \
     {integrator}'s last turn comes after your last, so ask for the integration of all your work by the end of your \
     last turn.";

/// The integrator's and the builders' role sentences in `mode`.
pub fn roles(mode: &str) -> (&'static str, &'static str) {
    if mode == crate::git_project::NAME {
        (GIT_INTEGRATOR_ROLE, GIT_BUILDER_ROLE)
    } else {
        (INTEGRATOR_ROLE, BUILDER_ROLE)
    }
}

/// The sentence telling agent `name` of `agents` in `mode` its role under the integrator setting `setting`: none
/// for [`crate::weave_mode::ANY_AGENT`], where every agent integrates.
pub fn role_message(mode: &str, setting: &str, name: &str, agents: usize) -> Result<Option<String>, String> {
    let (integrator_role, builder_role) = roles(mode);
    Ok(crate::weave_mode::integrator_agent(setting, agents)?.map(|n| {
        let integrator = format!("agent-{n}");
        if name == integrator { integrator_role.to_string() } else { builder_role.replace("{integrator}", &integrator) }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mode::mode_named;
    use crate::task::Task;

    #[test]
    fn one_integrator_and_its_builders_are_each_told_their_role() {
        for (mode, own) in [("weave", INTEGRATOR_ROLE), (crate::git_project::NAME, GIT_INTEGRATOR_ROLE)] {
            assert_eq!(role_message(mode, "any-agent", "agent-0", 4), Ok(None));
            let integrator = role_message(mode, "agent-3", "agent-3", 4).expect("a setting").expect("a role");
            assert_eq!(integrator, own);
            let builder = role_message(mode, "agent-3", "agent-0", 4).expect("a setting").expect("a role");
            assert!(builder.starts_with("agent-3 is the run's one integrator") && !builder.contains('{'), "{builder}");
            assert!(role_message(mode, "agent-4", "agent-0", 4).is_err(), "no agent-4 in a run of 4");
            assert!(role_message(mode, "someone", "agent-0", 4).is_err());
        }
        let git = role_message(crate::git_project::NAME, "agent-3", "agent-0", 4).expect("a setting").expect("a role");
        assert!(git.contains("branch") && !git.contains("propose"), "git's words: {git}");
    }

    #[test]
    fn every_mode_has_its_own_prompt() {
        let mut seen = std::collections::BTreeSet::new();
        for name in ["bracken", "git", "crdt"] {
            let prompt = system_prompt(name).expect("a prompt");
            assert!(prompt.starts_with(BASE) && prompt.ends_with(section(name).expect("a section")));
            assert!(seen.insert(prompt), "{name}'s prompt is its own");
        }
        let weave = system_prompt("weave").expect("a prompt");
        assert!(weave.starts_with(PROJECT_BASE) && weave.ends_with(section("weave").expect("a section")));
        assert!(seen.insert(weave), "weave's prompt is its own");
        let git = system_prompt(crate::git_project::NAME).expect("a prompt");
        assert!(
            git.starts_with(&git_project_base())
                && git.ends_with(section(crate::git_project::NAME).expect("a section"))
        );
        assert!(seen.insert(git), "git project mode's prompt is its own");
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
            let tools = mode_named(name).expect("a mode").tools(Task::Phase1);
            let naming: Vec<_> = tools.iter().filter(|t| t.description.contains(phrase)).collect();
            assert_eq!(naming.len(), 1, "{name}: one tool names the channel");
            let reader = naming[0].description.rsplit("with ").next().expect("a tool");
            assert!(tools.iter().any(|t| t.name == reader), "{name}: read with one of its tools, not {reader}");
            assert!(!section(name).expect("a section").contains(phrase), "{name}'s section names none");
        }
        assert!(!BASE.contains(phrase));
    }

    /// Every mode's prompt says, in the shared words, that an author's own check never counts, so
    /// others' current work that waits for a verdict is checked first and the verdict recorded, and
    /// that inputs are a, b and c whatever the specification calls them (task `6b3f9afbdcf2`,
    /// approval `e0ff76633d31`, analysis `61e53d61b843`). The verdict is named in each mode's form,
    /// a pass or an approval, so no mode's word stands for the others' (finding `26aeb8d5d0e1`).
    #[test]
    fn every_prompt_says_whose_check_counts_and_what_inputs_are_named() {
        let rules = [
            "A function's inputs are always named a, b and c, in the order its specification lists them, whatever \
             names the specification gives them.",
            "Your own check of your own work never counts as anyone else's: where the library needs another \
             agent's verdict before work counts (a pass, or an approval), only theirs does. So when others' current \
             work waits for such a verdict, check it first and record yours, before you write more of your own.",
        ];
        for name in ["bracken", "git", "crdt"] {
            let prompt = system_prompt(name).expect("a prompt");
            for rule in rules {
                assert_eq!(prompt.matches(rule).count(), 1, "{name}: {rule}");
            }
        }
        for name in ["bracken", "git", "crdt"] {
            assert!(!section(name).expect("a section").contains("a, b and c"), "{name}: the base says it, once");
        }
    }

    /// Each mode shows the definition form once, in the same words, in the tool that writes a
    /// definition: the artifact in Bracken, an edit in Git and CRDT (task `eca3770b7ae4`; the second
    /// Pi run's outcome `e7034ccbf48f` had 8 writes without it).
    #[test]
    fn each_mode_shows_the_definition_form_once_in_its_writing_tool() {
        let form = "Each definition is one line: `def <name> <arity> : <program>`";
        for (name, writer) in [("bracken", "artifact"), ("git", "edit"), ("crdt", "edit")] {
            for task in [Task::Phase1, Task::Phase2] {
                let tools = mode_named(name).expect("a mode").tools(task);
                let showing: Vec<_> = tools.iter().filter(|t| t.description.contains(form)).map(|t| t.name).collect();
                assert_eq!(showing, [writer], "{name}, {task:?}");
            }
        }
        assert!(!BASE.contains(form));
    }

    /// Git project mode's base is Weave's in every word but those [`GIT_WORDS`] gives git's for, and none of
    /// Weave's own words is left in it or its section (task `6bd13d994a03`).
    #[test]
    fn git_project_mode_reads_as_weave_mode_in_gits_words() {
        let git = git_project_base();
        for (weave, theirs) in GIT_WORDS {
            assert_eq!(PROJECT_BASE.matches(weave).count(), 1, "{weave}");
            assert!(git.contains(theirs) && !git.contains(weave), "{theirs}");
        }
        let section = section(crate::git_project::NAME).expect("a section");
        let take = serde_json::to_string(&crate::taken::take_tool()).expect("JSON");
        for word in ["propose", "proposal", "decision", "claim", "line", "Weave", "manifest"] {
            assert!(!git.contains(word) && !section.contains(word), "{word}");
            assert!(!take.contains(word), "the take tool is in neither mode's words: {word}");
            assert!(!GIT_INTEGRATOR_ROLE.contains(word) && !GIT_BUILDER_ROLE.contains(word), "{word}");
        }
        let tools = crate::mode::mode_named(crate::git_project::NAME).expect("the mode").tools(Task::Phase1);
        let weave = crate::mode::mode_named("weave").expect("the mode").tools(Task::Phase1);
        for t in tools.iter().filter(|t| crate::git_project::SHARED.contains(&t.name)) {
            let w = weave.iter().find(|w| w.name == t.name).expect("the same tool");
            assert_eq!((t.description, &t.input_schema), (w.description, &w.input_schema), "{} is Weave's", t.name);
        }
    }

    /// Both project modes' prompts say, in the same words, to take an item before working on it and never to work
    /// on another's; and in each mode the integrator is told it takes none and plays last, and builders that it
    /// plays after them (task `cb6a40559593`).
    #[test]
    fn both_project_modes_divide_the_work_alike() {
        let rule = "Before you work on an item, take it with take_item: the task's index shows who has taken each \
                    item, and you never work on an item another agent has taken.";
        for mode in ["weave", crate::git_project::NAME] {
            assert_eq!(system_prompt(mode).expect("a prompt").matches(rule).count(), 1, "{mode}");
            let (integrator, builder) = roles(mode);
            assert!(integrator.contains("You take no items yourself."), "{mode}");
            assert!(integrator.contains("Your last turn starts only once every other agent's last turn has ended"));
            assert!(builder.contains("{integrator}'s last turn comes after your last"), "{mode}");
            // Integration as work arrives, not only in the last turn (task 61911c91247f).
            assert!(integrator.contains("Each of your turns starts once every other agent has ended that turn"));
            // The integrator may write code to reconcile or repair, and repairs a broken main first, in the same
            // words in both modes (task b0a9769a48da).
            for rule in [
                "You may write code, but only to reconcile a conflict or to repair main.",
                "After each integration, its result shows main's public tests: while a test that passed on main \
                 before fails, main is broken, and you repair it before you integrate anything else.",
            ] {
                assert_eq!(integrator.matches(rule).count(), 1, "{mode}: {rule}");
            }
            assert!(!integrator.contains("write any item's code"), "{mode}");
            assert!(builder.contains("after each of your turns") && builder.contains("as soon as"), "{mode}");
        }
    }

    /// Nothing tells an agent it is measured, or that other modes exist (design section 4).
    #[test]
    fn the_prompts_never_mention_the_experiment() {
        let mut texts = vec![BASE.to_string(), PROJECT_BASE.to_string(), turn_message(1, 8, "agent-0")];
        texts.extend([git_project_base(), GIT_INTEGRATOR_ROLE.into(), GIT_BUILDER_ROLE.into()]);
        texts.extend([INTEGRATOR_ROLE.into(), BUILDER_ROLE.into()]);
        for name in ["bracken", "git", "crdt", "weave", crate::git_project::NAME] {
            texts.push(section(name).expect("a section").to_string());
            let mode = mode_named(name).expect("a mode");
            texts.push(serde_json::to_string(&mode.tools(Task::Phase1)).expect("JSON"));
        }
        texts.push(serde_json::to_string(&crate::taken::take_tool()).expect("JSON"));
        let all = texts.join("\n").to_lowercase();
        for word in ["experiment", "metric", "measure", "token", "score", "grader", "hidden", "mode", "crdt", "bracken"]
        {
            assert!(!all.contains(word), "a prompt mentions {word:?}");
        }
    }
}
