//! Dividing a project run's work (task `cb6a40559593`, stage 3): an agent takes a work item before working on
//! it, and the task tool shows every item's taker to every agent, so builders work on disjoint items.
//!
//! The runner holds who took what, the same in every mode, so the coordination substrate (Weave, or git and
//! branches) stays the only difference between modes. The tool is `take_item`, in words neither mode owns.
//!
//! The rules:
//! - Only an item already released can be taken, and by one agent at a time; taking one's own again is a no-op.
//! - A run with one integrator: the integrator takes no items. Its work is integrating the builders' items.
//! - An agent holds at most its fair share of the released items: their count over the agents that may take
//!   items, rounded up. So no agent can take every item at the start; giving one up frees a place.
//! - An item stays taken until its taker gives it up (`give_up`), even once its work is done: the index then
//!   says whose work it is.
//! - When the change goes out, a change item whose revised items were all taken by one agent is that agent's,
//!   taken for it by the runner (it holds the code the change revises); otherwise it is free to take, as any
//!   other item. Such a taking counts towards the agent's share like any other.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::args::short;
use crate::mode::ToolSpec;
use crate::project::{Item, Kind, Release};

/// The tool, the same in every project run.
pub fn take_tool() -> ToolSpec {
    ToolSpec {
        name: "take_item",
        description: "Take a released work item as yours before you work on it, so others leave it to you; refused \
                      if another agent has taken it or you hold your share. The task tool's index shows who has \
                      taken each item. With give_up set, give up an item you took, so another agent can take it",
        input_schema: json!({
            "type": "object",
            "properties": {
                "item": {"type": "string", "description": "An item's id, as the task's index lists it"},
                "give_up": {"type": "boolean"},
            },
            "required": ["item"],
            "additionalProperties": false,
        }),
    }
}

/// The field of an index entry, or of an item's view, that names its taker.
pub const TAKEN_BY: &str = "taken_by";

/// Room an entry needs for `,"taken_by":"agent-NNN"`: what the task tool's pages keep free per item.
pub const TAKEN_ROOM: usize = 32;

/// Who has taken which item, for one run.
#[derive(Debug, Default)]
pub struct Taken {
    /// Item id to the agent (index) that took it.
    by: BTreeMap<String, usize>,
    /// The run's one integrator, which takes no items.
    integrator: Option<usize>,
    agents: usize,
    /// The ids of the items released so far.
    released: Vec<String>,
    /// Each change item, and the items it revises.
    changes: Vec<(String, Vec<String>)>,
    /// The ids of the items the change releases, until it goes out.
    later: Vec<String>,
}

impl Taken {
    /// Nothing taken yet: the items released at the start are open. A run holding the change back never
    /// releases the later ones.
    pub fn new(items: &[Item], agents: usize, integrator: Option<usize>) -> Taken {
        let ids = |start: bool| -> Vec<String> {
            items.iter().filter(|i| (i.release == Release::Start) == start).map(|i| i.id.clone()).collect()
        };
        let changes =
            items.iter().filter(|i| i.kind == Kind::Change).map(|i| (i.id.clone(), i.revises.clone())).collect();
        Taken { by: BTreeMap::new(), integrator, agents, released: ids(true), changes, later: ids(false) }
    }

    /// How many agents may take items.
    fn takers(&self) -> usize {
        self.agents - usize::from(self.integrator.is_some())
    }

    /// The most items one agent may hold now: the released items over the agents that may take them, rounded
    /// up.
    pub fn share(&self) -> usize {
        self.released.len().div_ceil(self.takers().max(1))
    }

    /// The ids `agent` holds, in order.
    pub fn held(&self, agent: usize) -> Vec<&str> {
        self.by.iter().filter(|(_, a)| **a == agent).map(|(id, _)| id.as_str()).collect()
    }

    /// `take_item` for `agent`: takes `item`, or with `give_up`, gives it up. The result says what holds now, and
    /// whether anything changed (taking one's own item again changes nothing).
    pub fn take(&mut self, agent: usize, item: &str, give_up: bool) -> Result<(Value, bool), String> {
        if !self.released.iter().any(|i| i == item) {
            return Err(format!("no released item {}: the task's index lists those released", short(item)));
        }
        let holder = self.by.get(item).copied();
        if give_up {
            if holder != Some(agent) {
                return Err(format!("you haven't taken {item}, so there is nothing to give up"));
            }
            self.by.remove(item);
            return Ok((json!({"item": item, "given_up": true, "yours": self.held(agent)}), true));
        }
        if self.integrator == Some(agent) {
            return Err("you are the run's one integrator: the other agents take the items, and you integrate their \
                        work, so you take none"
                .into());
        }
        let changed = match holder {
            Some(a) if a == agent => false,
            Some(a) => return Err(format!("{item} is taken by agent-{a}: leave it to them, and take another")),
            None => {
                // A change handed on at its release can put an agent past its share: say what it holds.
                let (share, held) = (self.share(), self.held(agent).len());
                if held >= share {
                    return Err(format!(
                        "you hold {held} items, and one agent may hold {share} now: finish them, or give one up to \
                         take another"
                    ));
                }
                self.by.insert(item.to_string(), agent);
                true
            }
        };
        Ok((json!({"item": item, TAKEN_BY: format!("agent-{agent}"), "yours": self.held(agent)}), changed))
    }

    /// The change goes out: its items are released, and a change whose revised items one agent holds, all of
    /// them, is taken for that agent. Each such taking, as `(item, agent)`.
    pub fn change_out(&mut self) -> Vec<(String, usize)> {
        self.released.append(&mut self.later);
        let mut taken = Vec::new();
        for (change, revises) in &self.changes {
            if self.by.contains_key(change) || !self.released.contains(change) {
                continue;
            }
            let holders: Vec<Option<usize>> = revises.iter().map(|r| self.by.get(r).copied()).collect();
            if let Some(Some(a)) = holders.first().copied()
                && holders.iter().all(|h| *h == Some(a))
            {
                taken.push((change.clone(), a));
            }
        }
        for (item, a) in &taken {
            self.by.insert(item.clone(), *a);
        }
        taken
    }

    /// Every taking, by item id, as agents' names.
    pub fn names(&self) -> BTreeMap<String, String> {
        self.by.iter().map(|(id, a)| (id.clone(), format!("agent-{a}"))).collect()
    }
}

/// The task tool's `result` with each item's taker: on every index entry an item of `taken` names, and on the
/// view of one item. The task tool keeps [`TAKEN_ROOM`] bytes free for each, so a page stays under the cap.
pub fn show_taken(result: &mut Value, taken: &BTreeMap<String, String>) {
    if let Some(index) = result.get_mut("items").and_then(Value::as_array_mut) {
        for entry in index {
            let by = entry["id"].as_str().and_then(|id| taken.get(id));
            if let (Some(by), Some(map)) = (by, entry.as_object_mut()) {
                map.insert(TAKEN_BY.into(), json!(by));
            }
        }
    }
    let by = result.get("item").and_then(Value::as_str).and_then(|id| taken.get(id)).cloned();
    if let (Some(by), Some(map)) = (by, result.as_object_mut()) {
        map.insert(TAKEN_BY.into(), json!(by));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn item(id: &str, kind: Kind, release: Release, revises: &[&str]) -> Item {
        Item {
            id: id.into(),
            kind,
            title: id.into(),
            text: PathBuf::new(),
            release,
            revises: revises.iter().map(|r| r.to_string()).collect(),
        }
    }

    fn five() -> Vec<Item> {
        let mut items: Vec<Item> =
            ["a", "b", "c", "d", "e"].iter().map(|id| item(id, Kind::Feature, Release::Start, &[])).collect();
        items.push(item("ch", Kind::Change, Release::AfterShare(0.5), &["a", "b"]));
        items.push(item("ch2", Kind::Change, Release::AfterShare(0.5), &["c", "d"]));
        items
    }

    #[test]
    fn builders_take_disjoint_items_up_to_their_share_and_the_integrator_takes_none() {
        let mut t = Taken::new(&five(), 4, Some(3));
        assert_eq!(t.share(), 2, "5 items over 3 builders");
        assert!(t.take(0, "a", false).expect("free").1, "a taking");
        assert!(!t.take(0, "a", false).expect("its own again").1, "nothing changes");
        let err = t.take(1, "a", false).expect_err("taken");
        assert!(err.contains("taken by agent-0"), "{err}");
        assert!(t.take(3, "b", false).expect_err("the integrator").contains("integrator"));
        assert!(t.take(0, "ch", false).expect_err("not released").contains("no released item"));
        t.take(0, "b", false).expect("its second");
        let err = t.take(0, "c", false).expect_err("over its share");
        assert!(err.contains("you hold 2 items, and one agent may hold 2"), "{err}");
        assert!(t.take(1, "b", true).expect_err("not its own").contains("haven't taken"));
        let (given, changed) = t.take(0, "b", true).expect("its own");
        assert_eq!((given["yours"].clone(), changed), (json!(["a"]), true));
        t.take(1, "b", false).expect("free again");
        assert_eq!(t.names(), BTreeMap::from([("a".into(), "agent-0".into()), ("b".into(), "agent-1".into())]));
    }

    #[test]
    fn a_change_goes_to_the_one_agent_holding_what_it_revises() {
        let mut t = Taken::new(&five(), 3, None);
        for (agent, id) in [(0, "a"), (0, "b"), (1, "c"), (2, "d")] {
            t.take(agent, id, false).expect("free");
        }
        assert_eq!(t.change_out(), [("ch".to_string(), 0)], "ch2's items are held by two agents");
        assert_eq!(t.share(), 3, "7 items over 3 agents");
        let err = t.take(0, "e", false).expect_err("a, b and the change handed on");
        assert!(err.contains("you hold 3 items, and one agent may hold 3"), "{err}");
        t.take(2, "ch2", false).expect("released, and free");
        assert!(t.take(1, "ch", false).expect_err("agent-0's").contains("agent-0"));
    }

    #[test]
    fn the_index_and_an_items_view_show_its_taker() {
        let taken = BTreeMap::from([("a".to_string(), "agent-1".to_string())]);
        let mut page = json!({"spec": "s", "items": [{"id": "a"}, {"id": "b"}], "texts": []});
        show_taken(&mut page, &taken);
        assert_eq!(page["items"], json!([{"id": "a", "taken_by": "agent-1"}, {"id": "b"}]));
        let mut one = json!({"item": "a", "text": "Item a"});
        show_taken(&mut one, &taken);
        assert_eq!(one["taken_by"], "agent-1");
        let entry = json!({"taken_by": "agent-999"}).to_string();
        assert!(entry.len() < TAKEN_ROOM, "{entry}");
    }
}
