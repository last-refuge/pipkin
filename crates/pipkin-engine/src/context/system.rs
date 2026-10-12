//! Positional system sections and tool declarations (pinned durable harness prompt contract).
use pipkin_engine_protocol::{
    Entry, EntryKind, Message, SystemContent, SystemMessage, ToolDeclaration, ToolReference,
};
use serde_json::{Map, Value};

use super::derive;

pub fn is_system_entry(entry: &Entry) -> bool {
    matches!(
        &entry.kind,
        EntryKind::Message {
            message: Message::System(_),
            ..
        }
    ) || matches!(&entry.kind, EntryKind::Custom { custom_type, .. } if custom_type == "pi.system")
}

pub fn sections(messages: &[Message]) -> Map<String, Value> {
    let mut shown = Map::new();
    for message in messages {
        if let Message::System(system) = message {
            for (key, value) in &system.sections {
                if value.is_null() {
                    shown.shift_remove(key);
                } else {
                    shown.insert(key.clone(), value.clone());
                }
            }
        }
    }
    shown
}

pub fn tools(messages: &[Message]) -> Vec<ToolDeclaration> {
    let mut tools: Vec<ToolDeclaration> = Vec::new();
    for message in messages {
        if let Message::System(system) = message {
            for removed in &system.tools_removed {
                tools.retain(|t| t.name != removed.name);
            }
            for added in &system.tools_added {
                if let Some(old) = tools.iter_mut().find(|t| t.name == added.name) {
                    *old = added.clone();
                } else {
                    tools.push(added.clone());
                }
            }
        }
    }
    tools
}

pub fn text(system: &SystemMessage) -> String {
    std::iter::once(system.content.text())
        .chain(
            system
                .sections
                .values()
                .filter_map(Value::as_str)
                .map(str::to_owned),
        )
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

pub fn update_text(system: &SystemMessage) -> String {
    let mut parts = Vec::new();
    let content = system.content.text();
    if !content.is_empty() {
        parts.push(content);
    }
    for (key, value) in &system.sections {
        parts.push(match value.as_str() {
            Some(value) => format!("Updated system prompt section \"{key}\":\n\n{value}"),
            None => format!("Removed system prompt section \"{key}\"."),
        });
    }
    parts.join("\n\n")
}

/// APIs without mid-conversation system support receive the fully replayed prompt.
pub fn collapse(messages: &[Message]) -> Vec<Message> {
    collapse_owned(messages.to_vec())
}

pub fn collapse_owned(messages: Vec<Message>) -> Vec<Message> {
    let systems: Vec<_> = messages
        .iter()
        .filter_map(|m| match m {
            Message::System(s) => Some(s),
            _ => None,
        })
        .collect();
    let Some(first) = systems.first() else {
        return messages;
    };
    let combined = SystemMessage {
        content: SystemContent::Text(
            systems
                .iter()
                .map(|s| s.content.text())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n"),
        ),
        sections: sections(&messages),
        tools_added: tools(&messages),
        timestamp: first.timestamp,
        ..SystemMessage::default()
    };
    std::iter::once(Message::System(combined))
        .chain(
            messages
                .into_iter()
                .filter(|m| !matches!(m, Message::System(_))),
        )
        .collect()
}

#[derive(Debug, Default)]
pub struct Plan {
    pub messages: Vec<SystemMessage>,
    /// Old system entries omitted when a compaction/reset starts a new baseline.
    pub retire: Vec<u64>,
}

pub fn plan(
    entries: &[Entry],
    desired: &Map<String, Value>,
    wanted: &[ToolDeclaration],
    timestamp: i64,
) -> Plan {
    if let Some(head) = entries.first().filter(|e| derive::is_head_marker(e))
        && !entries.iter().any(|e| is_system_entry(e) && e.id > head.id)
    {
        return Plan {
            messages: vec![SystemMessage {
                sections: desired.clone(),
                tools_added: wanted.to_vec(),
                timestamp,
                ..Default::default()
            }],
            retire: entries
                .iter()
                .filter(|e| is_system_entry(e))
                .map(|e| e.id)
                .collect(),
        };
    }
    let context: Vec<_> = entries
        .iter()
        .filter(|e| is_system_entry(e))
        .flat_map(|e| derive::contribution(e, false))
        .collect();
    let shown = sections(&context);
    let patched_order: Vec<_> = shown
        .keys()
        .filter(|k| desired.contains_key(*k))
        .chain(desired.keys().filter(|k| !shown.contains_key(*k)))
        .collect();
    let mut patches = Vec::new();
    if patched_order != desired.keys().collect::<Vec<_>>() {
        patches.push(shown.keys().map(|k| (k.clone(), Value::Null)).collect());
        patches.push(desired.clone());
    } else {
        let mut patch = Map::new();
        for (key, value) in &shown {
            if desired.get(key) != Some(value) {
                patch.insert(
                    key.clone(),
                    desired.get(key).cloned().unwrap_or(Value::Null),
                );
            }
        }
        for (key, value) in desired {
            if !shown.contains_key(key) {
                patch.insert(key.clone(), value.clone());
            }
        }
        if !patch.is_empty() {
            patches.push(patch);
        }
    }
    let offered = tools(&context);
    // Parameter key order matters in the reference's declaration comparison.
    let kept: Vec<_> = offered
        .iter()
        .filter(|old| {
            wanted.iter().any(|new| {
                old.name == new.name
                    && serde_json::to_string(old).ok() == serde_json::to_string(new).ok()
            })
        })
        .collect();
    let added: Vec<_> = wanted
        .iter()
        .filter(|t| !kept.iter().any(|k| k.name == t.name))
        .cloned()
        .collect();
    let replayed: Vec<_> = kept
        .iter()
        .map(|t| &t.name)
        .chain(added.iter().map(|t| &t.name))
        .collect();
    let (removed, added) = if replayed != wanted.iter().map(|t| &t.name).collect::<Vec<_>>() {
        (
            offered
                .iter()
                .map(|t| ToolReference {
                    name: t.name.clone(),
                })
                .collect(),
            wanted.to_vec(),
        )
    } else {
        (
            offered
                .iter()
                .filter(|t| !kept.iter().any(|k| k.name == t.name))
                .map(|t| ToolReference {
                    name: t.name.clone(),
                })
                .collect::<Vec<_>>(),
            added,
        )
    };
    let mut messages: Vec<_> = patches
        .into_iter()
        .map(|sections| SystemMessage {
            sections,
            timestamp,
            ..Default::default()
        })
        .collect();
    if !removed.is_empty() || !added.is_empty() {
        if messages.is_empty() {
            messages.push(SystemMessage {
                timestamp,
                ..Default::default()
            });
        }
        let last = messages.last_mut().unwrap();
        last.tools_added = added;
        last.tools_removed = removed;
    }
    Plan {
        messages,
        retire: Vec::new(),
    }
}

/// Additions can be anchored only if the transcript never removes or redeclares a tool.
pub fn additive_tools(messages: &[Message]) -> bool {
    let mut names = std::collections::HashSet::new();
    for message in messages {
        if let Message::System(system) = message {
            if !system.tools_removed.is_empty() {
                return false;
            }
            for tool in &system.tools_added {
                if !names.insert(&tool.name) {
                    return false;
                }
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use pipkin_engine_protocol::InputMode;
    use serde_json::json;

    fn patch(sections: Value, added: &[&str], removed: &[&str]) -> SystemMessage {
        SystemMessage {
            sections: sections.as_object().unwrap().clone(),
            tools_added: added
                .iter()
                .map(|name| ToolDeclaration {
                    name: (*name).into(),
                    description: (*name).into(),
                    parameters: json!({"type":"object"}),
                    constrained_sampling: None,
                })
                .collect(),
            tools_removed: removed
                .iter()
                .map(|name| ToolReference {
                    name: (*name).into(),
                })
                .collect(),
            timestamp: 1,
            ..Default::default()
        }
    }
    fn entry(id: u64, system: SystemMessage) -> Entry {
        Entry {
            id,
            at_ms: 1,
            run: None,
            retired: false,
            kind: EntryKind::Message {
                message: Message::System(system),
                mode: InputMode::Prompt,
                request: None,
                partial: false,
            },
        }
    }
    #[test]
    fn planner_and_collapse_match_executed_pinned_reference_cases() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../../fixtures/native-system/prompt-reference.json"
        ))
        .unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            let mut entries = Vec::new();
            if let Some(id) = case["head"].as_u64() {
                entries.push(Entry {
                    id,
                    at_ms: id as i64,
                    run: None,
                    retired: false,
                    kind: EntryKind::Compaction {
                        summary: "summary".into(),
                        first_kept: 1,
                        tokens_before: 0,
                    },
                });
            }
            for raw in case["entries"].as_array().unwrap() {
                let Message::System(system) =
                    serde_json::from_value(raw["model"][0].clone()).unwrap()
                else {
                    panic!()
                };
                entries.push(entry(raw["id"].as_u64().unwrap(), system));
            }
            let desired = case["desired"].as_object().unwrap();
            let wanted: Vec<ToolDeclaration> =
                serde_json::from_value(case["tools"].clone()).unwrap();
            let planned = plan(&entries, desired, &wanted, 100);
            let raw: Vec<_> = planned
                .messages
                .iter()
                .enumerate()
                .map(|(i, system)| {
                    let mut draft = json!({"model":[Message::System(system.clone())]});
                    if i == 0 && !planned.retire.is_empty() {
                        draft["edits"] = json!(
                            planned
                                .retire
                                .iter()
                                .map(|id| json!({"target":id,"action":"omit"}))
                                .collect::<Vec<_>>()
                        );
                    }
                    draft
                })
                .collect();
            assert_eq!(
                json!(raw),
                case["planned"],
                "{}: planned entries",
                case["name"]
            );
            // Compare serialized JSON too: object key order is part of section replay.
            assert_eq!(
                serde_json::to_string(&raw).unwrap(),
                serde_json::to_string(&case["planned"]).unwrap(),
                "{}: ordered patches",
                case["name"]
            );
            let mut messages = entries
                .iter()
                .filter(|e| !planned.retire.contains(&e.id))
                .flat_map(|e| derive::contribution(e, false))
                .collect::<Vec<_>>();
            messages.extend(planned.messages.into_iter().map(Message::System));
            assert_eq!(
                serde_json::to_value(collapse(&messages)).unwrap(),
                case["collapsed"],
                "{}: collapsed",
                case["name"]
            );
        }
    }

    #[test]
    fn minimal_patches_preserve_order_and_reorder_removes_then_readds() {
        let a = patch(json!({"z":"Z","a":"A"}), &["read", "write"], &[]);
        let entries = vec![entry(1, a.clone())];
        assert!(
            plan(&entries, &a.sections, &a.tools_added, 2)
                .messages
                .is_empty()
        );
        let desired = json!({"z":"Z2","a":"A","b":"B"})
            .as_object()
            .unwrap()
            .clone();
        let p = plan(&entries, &desired, &a.tools_added, 2);
        assert_eq!(p.messages.len(), 1);
        assert_eq!(
            p.messages[0].sections,
            json!({"z":"Z2","b":"B"}).as_object().unwrap().clone()
        );
        let desired = json!({"a":"A","z":"Z"}).as_object().unwrap().clone();
        let p = plan(&entries, &desired, &a.tools_added, 3);
        assert_eq!(p.messages.len(), 2);
        assert!(p.messages[0].sections.values().all(Value::is_null));
        let mut replay = vec![Message::System(a)];
        replay.extend(p.messages.into_iter().map(Message::System));
        assert_eq!(
            sections(&replay)
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["a", "z"]
        );
    }
    #[test]
    fn tool_redefinitions_and_order_changes_preserve_the_desired_declarations() {
        let a = patch(json!({"p":"P"}), &["read", "write"], &[]);
        let mut desired = a.tools_added.clone();
        desired[0].description = "changed".into();
        let p = plan(&[entry(1, a.clone())], &a.sections, &desired, 2);
        assert_eq!(p.messages.len(), 1);
        assert_eq!(
            p.messages[0].tools_removed.len(),
            2,
            "changing the first definition also requires order repair"
        );
        let replay = [Message::System(a), Message::System(p.messages[0].clone())];
        assert_eq!(tools(&replay), desired);
        assert!(!additive_tools(&replay));
    }
    #[test]
    fn compaction_rebaselines_once_and_omits_retained_earlier_system_entries() {
        let a = patch(json!({"p":"P"}), &["read"], &[]);
        let head = Entry {
            id: 10,
            at_ms: 10,
            run: None,
            retired: false,
            kind: EntryKind::Compaction {
                summary: "summary".into(),
                first_kept: 1,
                tokens_before: 10,
            },
        };
        let mut entries = vec![head, entry(1, a.clone())];
        let p = plan(&entries, &a.sections, &a.tools_added, 11);
        assert_eq!(p.retire, [1]);
        assert_eq!(p.messages[0].sections, a.sections);
        entries[1].retired = true;
        entries.push(entry(11, p.messages[0].clone()));
        assert!(
            plan(&entries, &a.sections, &a.tools_added, 12)
                .messages
                .is_empty()
        );
    }
    #[test]
    fn legacy_imports_replay_with_current_messages_and_collapse_without_duplication() {
        let a = patch(json!({"z":"Z","a":"A"}), &["read"], &[]);
        let raw = serde_json::to_value(Message::System(a)).unwrap();
        let imported = Entry {
            id: 1,
            at_ms: 1,
            run: None,
            retired: false,
            kind: EntryKind::Custom {
                custom_type: "pi.system".into(),
                data: json!({"model":[raw]}),
            },
        };
        let update = patch(json!({"z":null,"b":"B"}), &["write"], &["read"]);
        let messages = derive::model_messages(&[imported, entry(2, update.clone())]);
        let collapsed = collapse(&messages);
        let Message::System(head) = &collapsed[0] else {
            panic!()
        };
        assert_eq!(text(head), "A\n\nB");
        assert_eq!(head.tools_added[0].name, "write");
        assert_eq!(
            update_text(&update),
            "Removed system prompt section \"z\".\n\nUpdated system prompt section \"b\":\n\nB"
        );
        assert_eq!(
            serde_json::from_value::<Message>(serde_json::to_value(&messages[0]).unwrap()).unwrap(),
            messages[0]
        );
    }
}
