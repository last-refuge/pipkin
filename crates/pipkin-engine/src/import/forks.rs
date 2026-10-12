//! Materialize the visible history of a Pi conversation before mapping native entries.
//! Fork ancestry controls inherited history; task ownership is a separate relation.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use super::legacy::{Legacy, LegacyEntry};

pub fn parent(legacy: &Legacy, conversation: i64) -> Result<Option<(i64, i64)>, String> {
    let Some(record) = legacy.conversations.get(&conversation) else {
        return Err(format!(
            "unknown conversation {conversation} in fork ancestry"
        ));
    };
    let Some(parent) = record.get("parent").filter(|p| !p.is_null()) else {
        return Ok(None);
    };
    let (Some(id), Some(at)) = (
        parent.get("conversationId").and_then(Value::as_i64),
        parent.get("at").and_then(Value::as_i64),
    ) else {
        return Err(format!(
            "conversation {conversation} has malformed fork ancestry"
        ));
    };
    if id <= 0 || at <= 0 {
        return Err(format!(
            "conversation {conversation} has invalid fork ancestry IDs"
        ));
    }
    Ok(Some((id, at)))
}

/// Pi's storage scans local entries, then each ancestor with an inclusive upper bound
/// that is the minimum of every intervening fork point. Validate every edge first, even
/// when the final context has a reset that hides its inherited messages.
pub fn visible_entries(legacy: &Legacy, conversation: i64) -> Result<Vec<&LegacyEntry>, String> {
    let mut chain = Vec::new();
    let mut seen = BTreeSet::new();
    let mut current = conversation;
    loop {
        if !seen.insert(current) {
            return Err(format!(
                "conversation {conversation} has cyclic fork ancestry"
            ));
        }
        let edge = parent(legacy, current)?;
        chain.push((current, edge));
        match edge {
            Some((ancestor, _)) => current = ancestor,
            None => break,
        }
    }
    for (i, (child, edge)) in chain.iter().enumerate() {
        let Some((_, at)) = edge else { continue };
        let entry = legacy
            .entry(*at)
            .ok_or_else(|| format!("conversation {child} forks at missing entry {at}"))?;
        let mut upper = i64::MAX;
        let mut visible = false;
        for (ancestor, edge) in chain.iter().skip(i + 1) {
            if entry.conversation == *ancestor {
                visible = entry.id <= upper;
                break;
            }
            if let Some((_, cut)) = edge {
                upper = upper.min(*cut);
            }
        }
        if !visible {
            return Err(format!(
                "conversation {child} forks at entry {at} that is not visible from its parent"
            ));
        }
    }
    let mut limits = BTreeMap::new();
    let mut upper = i64::MAX;
    for (id, edge) in chain {
        limits.insert(id, upper);
        if let Some((_, at)) = edge {
            upper = upper.min(at);
        }
    }
    Ok(legacy
        .entries
        .iter()
        .filter(|entry| {
            limits
                .get(&entry.conversation)
                .is_some_and(|upper| entry.id <= *upper)
        })
        .collect())
}
