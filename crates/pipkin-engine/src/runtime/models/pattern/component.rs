//! Component parser/regex translation for minimatch 10.2.6 defaults.
//! See packaging/upstream-notices/minimatch-10.2.6 for provenance and license.
mod class;

#[derive(Clone)]
enum Part {
    Raw(String),
    Group {
        kind: char,
        branches: Vec<Vec<Part>>,
        empty: bool,
    },
}

pub(super) fn compile(pattern: &str) -> Option<(regress::Regex, bool)> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut pos = 0;
    let mut parts = parse(&chars, &mut pos, None, 0);
    flatten(&mut parts);
    let mut unicode = false;
    let source = sequence(&parts, true, true, false, false, &[], &mut unicode);
    regress::Regex::with_flags(
        &format!("^{source}$"),
        regress::Flags {
            icase: true,
            unicode,
            ..Default::default()
        },
    )
    .ok()
    .map(|re| (re, unicode))
}

fn adopts(parent: char, child: char) -> bool {
    match parent {
        '!' => "?@".contains(child),
        '?' => "?@".contains(child),
        '@' => "?@".contains(child),
        '*' => "*+?@".contains(child),
        '+' => "+@?*".contains(child),
        _ => false,
    }
}
fn push_raw(parts: &mut Vec<Part>, raw: &mut String) {
    if !raw.is_empty() {
        parts.push(Part::Raw(std::mem::take(raw)));
    }
}
fn parse(chars: &[char], pos: &mut usize, parent: Option<char>, depth: usize) -> Vec<Part> {
    let mut parts = Vec::new();
    let mut raw = String::new();
    while *pos < chars.len() {
        let c = chars[*pos];
        if parent.is_some() && (c == '|' || c == ')') {
            break;
        }
        *pos += 1;
        if c == '\\' {
            raw.push(c);
            if *pos < chars.len() {
                raw.push(chars[*pos]);
                *pos += 1;
            }
            continue;
        }
        if c == '[' {
            raw.push(c);
            let start = *pos;
            let mut first = true;
            while *pos < chars.len() {
                let b = chars[*pos];
                raw.push(b);
                *pos += 1;
                if b == '\\' && *pos < chars.len() {
                    raw.push(chars[*pos]);
                    *pos += 1;
                    first = false;
                    continue;
                }
                if b == ']' && !first {
                    break;
                }
                if first && (b == '!' || b == '^') && *pos == start + 1 {
                    continue;
                }
                first = false;
            }
            continue;
        }
        let adopt = parent.is_some_and(|p| adopts(p, c));
        if "!?+*@".contains(c) && chars.get(*pos) == Some(&'(') && (depth <= 2 || adopt) {
            let start = *pos - 1;
            *pos += 1;
            let mut branches = Vec::new();
            loop {
                branches.push(parse(chars, pos, Some(c), depth + usize::from(!adopt)));
                if chars.get(*pos) == Some(&'|') {
                    *pos += 1;
                    continue;
                }
                break;
            }
            if chars.get(*pos) == Some(&')') {
                *pos += 1;
                push_raw(&mut parts, &mut raw);
                // Preserve the pinned parser's empty-tail flag, including a final nested group.
                let empty = branches
                    .last()
                    .is_none_or(|branch| !matches!(branch.last(), Some(Part::Raw(_))));
                parts.push(Part::Group {
                    kind: c,
                    branches,
                    empty,
                });
            } else {
                // An unfinished group becomes an ordinary glob, with literal parentheses/pipes.
                raw.extend(chars[start..].iter());
            }
            continue;
        }
        raw.push(c);
    }
    push_raw(&mut parts, &mut raw);
    parts
}

fn flatten(parts: &mut [Part]) {
    for part in parts {
        let Part::Group {
            kind,
            branches,
            empty,
        } = part
        else {
            continue;
        };
        for branch in branches.iter_mut() {
            flatten(branch);
        }
        for _ in 0..10 {
            let mut changed = false;
            let mut i = 0;
            while i < branches.len() {
                if branches[i].len() != 1 {
                    i += 1;
                    continue;
                }
                let Part::Group {
                    kind: child,
                    branches: sub,
                    ..
                } = &branches[i][0]
                else {
                    i += 1;
                    continue;
                };
                let (child, mut sub) = (*child, sub.clone());
                let plain = match *kind {
                    '!' => child == '@',
                    '?' => "?@".contains(child),
                    '@' => child == '@',
                    '*' => "*+?@".contains(child),
                    '+' => "+@".contains(child),
                    _ => false,
                };
                let with_empty = match *kind {
                    '!' | '@' => child == '?',
                    '+' => "?*".contains(child),
                    _ => false,
                };
                if plain || with_empty {
                    if with_empty {
                        sub.push(vec![]);
                    }
                    branches.splice(i..=i, sub);
                    changed = true;
                    i += 1;
                    continue;
                }
                let usurp = if branches.len() == 1 {
                    match (*kind, child) {
                        ('!', '!') => Some('@'),
                        ('?', '*' | '+') => Some('*'),
                        ('@', c) => Some(c),
                        ('+', '?' | '*') => Some('*'),
                        _ => None,
                    }
                } else {
                    None
                };
                if let Some(k) = usurp {
                    *kind = k;
                    *branches = sub;
                    *empty = false;
                    changed = true;
                    break;
                }
                i += 1;
            }
            if !changed {
                break;
            }
        }
    }
}

fn sequence(
    parts: &[Part],
    start: bool,
    end: bool,
    dot: bool,
    negative: bool,
    outer_tail: &[Part],
    unicode: &mut bool,
) -> String {
    let no_empty = start && end && parts.iter().all(|p| matches!(p, Part::Raw(_)));
    let mut source = String::new();
    let mut child_start = start;
    for (i, part) in parts.iter().enumerate() {
        match part {
            Part::Raw(raw) => source.push_str(&raw_regex(raw, no_empty, unicode)),
            Part::Group {
                kind,
                branches,
                empty,
            } => {
                let mut tail = parts[i + 1..].to_vec();
                tail.extend_from_slice(outer_tail);
                source.push_str(&group(
                    *kind,
                    branches,
                    *empty,
                    child_start,
                    end && i + 1 == parts.len(),
                    dot,
                    &tail,
                    unicode,
                ));
            }
        }
        child_start &= matches!(part, Part::Group { kind: '!', .. });
    }
    if start && let Some(Part::Raw(raw)) = parts.first() {
        let traversal = parts.len() == 1 && (raw == "." || raw == "..");
        if !traversal {
            let risky = |s: &str| s.starts_with(['[', '.']);
            let no_traversal = (dot && risky(&source))
                || source.strip_prefix("\\.").is_some_and(risky)
                || source.strip_prefix("\\.\\.").is_some_and(risky);
            if no_traversal {
                source = format!("(?!(?:^|/)\\.\\.?(?:$|/)){source}");
            } else if !dot && risky(&source) {
                source = format!("(?!\\.){source}");
            }
        }
    }
    if negative {
        source.push_str("(?:$|/)");
    }
    source
}

#[allow(clippy::too_many_arguments)]
fn group(
    kind: char,
    branches: &[Vec<Part>],
    empty: bool,
    start: bool,
    end: bool,
    dot: bool,
    tail: &[Part],
    unicode: &mut bool,
) -> String {
    let body_for = |allow_dot: bool, unicode: &mut bool| {
        branches
            .iter()
            .map(|branch| {
                let mut branch = branch.clone();
                if kind == '!' {
                    branch.extend_from_slice(tail);
                }
                sequence(
                    &branch,
                    start,
                    end || kind == '!',
                    allow_dot,
                    kind == '!',
                    tail,
                    unicode,
                )
            })
            .filter(|s| !(start && end) || !s.is_empty())
            .collect::<Vec<_>>()
            .join("|")
    };
    let body = body_for(dot, unicode);
    if start && end && body.is_empty() && kind != '!' {
        return escape(&format!("{kind}()"));
    }
    if kind == '!' && empty {
        return format!("{}[^/]+?", if start && !dot { "(?!\\.)" } else { "" });
    }
    if kind == '!' {
        return format!(
            "(?:(?!(?:{body})){}[^/]*?)",
            if start && !dot { "(?!\\.)" } else { "" }
        );
    }
    if "*+".contains(kind) && !dot {
        let dot_body = body_for(true, unicode);
        if dot_body != body {
            return format!(
                "(?:(?:{body})(?:{dot_body})*?){}",
                if kind == '*' { "?" } else { "" }
            );
        }
    }
    format!(
        "(?:{body}){}",
        if kind == '@' {
            String::new()
        } else {
            kind.to_string()
        }
    )
}

pub(super) fn escape(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c as u32 > 0xffff {
            for u in c.encode_utf16(&mut [0; 2]) {
                out.push_str(&format!("\\u{u:04x}"));
            }
        } else {
            if "^$\\.*+?()[]{}|".contains(c) {
                out.push('\\');
            }
            out.push(c);
        }
    }
    out
}
fn raw_regex(raw: &str, no_empty: bool, unicode: &mut bool) -> String {
    let chars: Vec<char> = raw.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        i += 1;
        match c {
            '\\' => {
                let c = chars.get(i).copied().unwrap_or('\\');
                if i < chars.len() {
                    i += 1;
                }
                out.push_str(&escape(&c.to_string()));
            }
            '*' => {
                while chars.get(i) == Some(&'*') {
                    i += 1;
                }
                out.push_str(if no_empty && chars.iter().all(|c| *c == '*') {
                    "[^/]+?"
                } else {
                    "[^/]*?"
                });
            }
            '?' => out.push_str("[^/]"),
            '[' => {
                if let Some((source, used, u)) = class::parse(&chars[i - 1..]) {
                    out.push_str(&source);
                    i += used - 1;
                    *unicode |= u;
                } else {
                    out.push_str("\\[");
                }
            }
            _ => out.push_str(&escape(&c.to_string())),
        }
    }
    out
}
