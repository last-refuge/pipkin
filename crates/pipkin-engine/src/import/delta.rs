//! Chord delta operations as the reference engine STORES them in `document_revisions.content`
//! (the decoded, full-form vocabulary: every path inline, no `#` definitions or short forms),
//! and an applier over `serde_json::Value`.
//!
//! Origin: the `Seg`/`Op` types and the applier are copied from `crates/pi-client/src/delta.rs`
//! (the legacy client's port of `packages/chord/src/delta/index.ts`), which is being retired;
//! this module must not depend on it. Only the stored-op parser is new here. Semantics follow
//! the reference `apply()` (`chord/src/delta/index.ts:326-420`): `p`/`m` may address the root,
//! `s`/`d`/`a`/`t` may not; an array index may address an element or append one past the end;
//! `t` counts UTF-16 code units; reserved prototype names are refused in paths.

use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeltaError(pub String);

impl std::fmt::Display for DeltaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "document delta: {}", self.0)
    }
}

type Result<T> = std::result::Result<T, DeltaError>;

fn delta(message: impl Into<String>) -> DeltaError {
    DeltaError(message.into())
}

/// Segments that reach the prototype chain in JavaScript consumers. Forbidden in paths.
const RESERVED_SEGMENTS: [&str; 3] = ["__proto__", "constructor", "prototype"];

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Seg {
    Key(String),
    Index(usize),
}

pub type Path = Vec<Seg>;

fn parse_path(value: &Value) -> Result<Path> {
    let items = value
        .as_array()
        .ok_or_else(|| delta("path is not an array"))?;
    let mut path = Vec::with_capacity(items.len());
    for item in items {
        match item {
            Value::String(s) => {
                if RESERVED_SEGMENTS.contains(&s.as_str()) {
                    return Err(delta(format!("unsafe path segment: {s}")));
                }
                path.push(Seg::Key(s.clone()));
            }
            Value::Number(n) => match n.as_u64() {
                Some(i) => path.push(Seg::Index(i as usize)),
                None => return Err(delta(format!("unsafe path segment: {n}"))),
            },
            other => return Err(delta(format!("unsafe path segment: {other}"))),
        }
    }
    Ok(path)
}

fn path_text(path: &[Seg]) -> String {
    let parts: Vec<String> = path
        .iter()
        .map(|s| match s {
            Seg::Key(k) => format!("{k:?}"),
            Seg::Index(i) => i.to_string(),
        })
        .collect();
    format!("[{}]", parts.join(","))
}

#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    Replace(Value),
    Set(Path, Value),
    Delete(Path),
    Append(Path, String),
    /// Drop the first `n` UTF-16 code units of the string at the path.
    Trim(Path, usize),
    Splice {
        path: Path,
        index: usize,
        remove: usize,
        items: Vec<Value>,
    },
    /// `new[i] = old[permutation[i]]` on the array at the path.
    Move(Path, Vec<usize>),
}

impl Op {
    fn validate(&self) -> Result<()> {
        match self {
            Op::Set(p, _) | Op::Delete(p) | Op::Append(p, _) | Op::Trim(p, _) if p.is_empty() => {
                Err(delta("path is empty"))
            }
            Op::Move(_, permutation) => assert_permutation(permutation),
            _ => Ok(()),
        }
    }
}

fn assert_permutation(permutation: &[usize]) -> Result<()> {
    let mut seen = vec![false; permutation.len()];
    for &index in permutation {
        if index >= permutation.len() || seen[index] {
            return Err(delta("m permutation is not a bijection"));
        }
        seen[index] = true;
    }
    Ok(())
}

fn natural(value: &Value, what: &str) -> Result<usize> {
    value
        .as_u64()
        .map(|n| n as usize)
        .ok_or_else(|| delta(format!("{what} must be a non-negative integer")))
}

/// Parse one stored (full-form) op: `["r", v]`, `["s", path, v]`, `["d", path]`,
/// `["a", path, text]`, `["t", path, n]`, `["p", path, index, remove, items]`,
/// `["m", path, permutation]`.
pub fn parse_op(value: &Value) -> Result<Op> {
    let op = value
        .as_array()
        .filter(|a| !a.is_empty())
        .ok_or_else(|| delta("op is not a tuple"))?;
    let verb = op[0].as_str().ok_or_else(|| delta("unknown op verb"))?;
    let op = match (verb, op.len()) {
        ("r", 2) => Op::Replace(op[1].clone()),
        ("s", 3) => Op::Set(parse_path(&op[1])?, op[2].clone()),
        ("d", 2) => Op::Delete(parse_path(&op[1])?),
        ("a", 3) => Op::Append(
            parse_path(&op[1])?,
            op[2]
                .as_str()
                .ok_or_else(|| delta("a value must be a string"))?
                .to_owned(),
        ),
        ("t", 3) => Op::Trim(parse_path(&op[1])?, natural(&op[2], "t count")?),
        ("p", 5) => Op::Splice {
            path: parse_path(&op[1])?,
            index: natural(&op[2], "p index")?,
            remove: natural(&op[3], "p remove")?,
            items: op[4]
                .as_array()
                .cloned()
                .ok_or_else(|| delta("p items is not an array"))?,
        },
        ("m", 3) => {
            let permutation = op[2]
                .as_array()
                .ok_or_else(|| delta("m permutation is not an array"))?
                .iter()
                .map(|v| natural(v, "m permutation entry"))
                .collect::<Result<Vec<_>>>()?;
            Op::Move(parse_path(&op[1])?, permutation)
        }
        (other, n) => return Err(delta(format!("unknown op {other} with arity {n}"))),
    };
    op.validate()?;
    Ok(op)
}

pub fn parse_ops(value: &Value) -> Result<Vec<Op>> {
    value
        .as_array()
        .ok_or_else(|| delta("ops is not an array"))?
        .iter()
        .map(parse_op)
        .collect()
}

fn is_container(value: &Value) -> bool {
    matches!(value, Value::Object(_) | Value::Array(_))
}

fn resolve_mut<'a>(root: &'a mut Value, path: &[Seg]) -> Result<&'a mut Value> {
    let mut node = root;
    for seg in path {
        node = match (node, seg) {
            (Value::Array(items), Seg::Index(i)) => {
                items.get_mut(*i).ok_or_else(|| unresolvable(path))?
            }
            (Value::Array(_), Seg::Key(k)) => {
                return Err(delta(format!("unsafe path segment: {k}")));
            }
            (Value::Object(map), Seg::Key(k)) => {
                map.get_mut(k).ok_or_else(|| unresolvable(path))?
            }
            (Value::Object(map), Seg::Index(i)) => map
                .get_mut(&i.to_string())
                .ok_or_else(|| unresolvable(path))?,
            _ => return Err(unresolvable(path)),
        };
    }
    if !is_container(node) {
        return Err(unresolvable(path));
    }
    Ok(node)
}

fn unresolvable(path: &[Seg]) -> DeltaError {
    delta(format!("unresolvable path: {}", path_text(path)))
}

fn utf16_to_byte_offset(text: &str, units: usize) -> Result<usize> {
    let mut seen = 0usize;
    for (byte, ch) in text.char_indices() {
        if seen == units {
            return Ok(byte);
        }
        seen += ch.len_utf16();
        if seen > units {
            return Err(delta("trim splits a surrogate pair"));
        }
    }
    Ok(text.len())
}

/// Apply `ops` to `root` (a `Replace` discards it).
pub fn apply(mut root: Value, ops: Vec<Op>) -> Result<Value> {
    for op in ops {
        op.validate()?;
        match op {
            Op::Replace(value) => root = value,
            Op::Splice {
                path,
                index,
                remove,
                items,
            } => {
                let Value::Array(array) = resolve_mut(&mut root, &path)? else {
                    return Err(unresolvable(&path));
                };
                let start = index.min(array.len());
                let end = start.saturating_add(remove).min(array.len());
                array.splice(start..end, items);
            }
            Op::Move(path, permutation) => {
                let Value::Array(array) = resolve_mut(&mut root, &path)? else {
                    return Err(unresolvable(&path));
                };
                if array.len() != permutation.len() {
                    return Err(unresolvable(&path));
                }
                let previous = std::mem::take(array);
                *array = permutation.iter().map(|&i| previous[i].clone()).collect();
            }
            Op::Set(path, value) => {
                let (parent, key) = parent_and_key(&mut root, &path)?;
                write(parent, key, value)?;
            }
            Op::Delete(path) => {
                let (parent, key) = parent_and_key(&mut root, &path)?;
                match (parent, key) {
                    (Value::Array(items), Seg::Index(i)) => {
                        if i >= items.len() {
                            return Err(unresolvable(&path));
                        }
                        items.remove(i);
                    }
                    (Value::Array(_), Seg::Key(k)) => {
                        return Err(delta(format!("unsafe path segment: {k}")));
                    }
                    (Value::Object(map), key) => {
                        map.remove(&key_name(&key));
                    }
                    _ => return Err(unresolvable(&path)),
                }
            }
            Op::Append(path, suffix) => {
                let (parent, key) = parent_and_key(&mut root, &path)?;
                let Some(Value::String(current)) = read(parent, &key) else {
                    return Err(unresolvable(&path));
                };
                current.push_str(&suffix);
            }
            Op::Trim(path, count) => {
                let (parent, key) = parent_and_key(&mut root, &path)?;
                let Some(Value::String(current)) = read(parent, &key) else {
                    return Err(unresolvable(&path));
                };
                let at = utf16_to_byte_offset(current, count)?;
                current.drain(..at);
            }
        }
    }
    Ok(root)
}

fn key_name(seg: &Seg) -> String {
    match seg {
        Seg::Key(k) => k.clone(),
        Seg::Index(i) => i.to_string(),
    }
}

fn parent_and_key<'a>(root: &'a mut Value, path: &[Seg]) -> Result<(&'a mut Value, Seg)> {
    let (key, parent_path) = path.split_last().ok_or_else(|| delta("path is empty"))?;
    Ok((resolve_mut(root, parent_path)?, key.clone()))
}

fn read<'a>(parent: &'a mut Value, key: &Seg) -> Option<&'a mut Value> {
    match (parent, key) {
        (Value::Array(items), Seg::Index(i)) => items.get_mut(*i),
        (Value::Object(map), key) => map.get_mut(&key_name(key)),
        _ => None,
    }
}

fn write(parent: &mut Value, key: Seg, value: Value) -> Result<()> {
    match (parent, key) {
        (Value::Array(items), Seg::Index(i)) => {
            if i > items.len() {
                return Err(delta(format!("unsafe path segment: {i}")));
            }
            if i == items.len() {
                items.push(value);
            } else {
                items[i] = value;
            }
            Ok(())
        }
        (Value::Array(_), Seg::Key(k)) => Err(delta(format!("unsafe path segment: {k}"))),
        (Value::Object(map), key) => {
            map.insert(key_name(&key), value);
            Ok(())
        }
        _ => Err(delta("parent is not a container")),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn run(base: Value, ops: Value) -> Result<Value> {
        apply(base, parse_ops(&ops)?)
    }

    #[test]
    fn applies_stored_full_form_ops() {
        let out = run(
            json!({"models": {}, "text": "ab", "list": [1, 2, 3]}),
            json!([
                ["s", ["models", "p/m"], {"input": 1}],
                ["a", ["text"], "c"],
                ["t", ["text"], 1],
                ["p", ["list"], 1, 1, [9, 8]],
                ["m", ["list"], [3, 2, 1, 0]],
                ["d", ["list", 0]]
            ]),
        )
        .unwrap();
        assert_eq!(
            out,
            json!({"models": {"p/m": {"input": 1}}, "text": "bc", "list": [8, 9, 1]})
        );
        assert_eq!(
            run(json!({}), json!([["r", {"a": 1}]])).unwrap(),
            json!({"a": 1})
        );
    }

    #[test]
    fn rejects_short_forms_and_unsafe_paths() {
        assert!(parse_ops(&json!([["s", 1]])).is_err());
        assert!(parse_ops(&json!([["#", 0, ["a"]]])).is_err());
        assert!(parse_ops(&json!([["s", ["__proto__"], 1]])).is_err());
        assert!(parse_ops(&json!([["s", [], 1]])).is_err());
        assert!(run(json!({"a": [1]}), json!([["s", ["a", 3], 2]])).is_err());
        assert!(run(json!({"t": "\u{1F600}z"}), json!([["t", ["t"], 1]])).is_err());
    }
}
