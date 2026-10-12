//! POSIX classes and minimatch's invalid-range/literal-bracket handling.
use super::escape;

pub(super) fn parse(chars: &[char]) -> Option<(String, usize, bool)> {
    let (mut i, mut negate, mut unicode, mut escaping, mut saw) = (1, false, false, false, false);
    let mut ranges = Vec::new();
    let mut negs = Vec::new();
    let mut range_start = None;
    while i < chars.len() {
        let c = chars[i];
        if i == 1 && (c == '!' || c == '^') {
            negate = true;
            i += 1;
            continue;
        }
        if c == ']' && saw && !escaping {
            break;
        }
        saw = true;
        if c == '\\' && !escaping {
            escaping = true;
            i += 1;
            continue;
        }
        if c == '[' && !escaping {
            let rest: String = chars[i..].iter().collect();
            if let Some((name, expr, u, neg)) =
                CLASSES.iter().find(|(name, ..)| rest.starts_with(name))
            {
                if range_start.is_some() {
                    return Some(("$.".into(), chars.len(), false));
                }
                if *neg {
                    negs.push(expr.to_string());
                } else {
                    ranges.push(expr.to_string());
                }
                unicode |= u;
                i += name.len();
                continue;
            }
        }
        escaping = false;
        if let Some(start) = range_start.take() {
            if c > start {
                ranges.push(format!("{}-{}", class_escape(start), class_escape(c)));
            } else if c == start {
                ranges.push(class_escape(c));
            }
            i += 1;
            continue;
        }
        if chars.get(i + 1) == Some(&'-') {
            if chars.get(i + 2) == Some(&']') {
                ranges.push(format!("{}\\-", class_escape(c)));
                i += 2;
                continue;
            }
            range_start = Some(c);
            i += 2;
            continue;
        }
        ranges.push(class_escape(c));
        i += 1;
    }
    if i == chars.len() {
        return None;
    }
    if ranges.is_empty() && negs.is_empty() {
        return Some(("$.".into(), chars.len(), false));
    }
    if negs.is_empty() && ranges.len() == 1 && !negate {
        let s = &ranges[0];
        if s.chars().count() == 1 || (s.starts_with('\\') && s.chars().count() == 2) {
            return Some((escape(&s.chars().last().unwrap().to_string()), i + 1, false));
        }
    }
    let positive = format!("[{}{}]", if negate { "^" } else { "" }, ranges.join(""));
    let negative = format!("[{}{}]", if negate { "" } else { "^" }, negs.join(""));
    let re = if ranges.is_empty() {
        negative
    } else if negs.is_empty() {
        positive
    } else {
        format!("({positive}|{negative})")
    };
    Some((re, i + 1, unicode))
}
fn class_escape(c: char) -> String {
    if c as u32 > 0xffff {
        escape(&c.to_string())
    } else if "[]\\-".contains(c) {
        format!("\\{c}")
    } else {
        c.to_string()
    }
}
const CLASSES: &[(&str, &str, bool, bool)] = &[
    ("[:alnum:]", r"\p{L}\p{Nl}\p{Nd}", true, false),
    ("[:alpha:]", r"\p{L}\p{Nl}", true, false),
    ("[:ascii:]", r"\x00-\x7f", false, false),
    ("[:blank:]", r"\p{Zs}\t", true, false),
    ("[:cntrl:]", r"\p{Cc}", true, false),
    ("[:digit:]", r"\p{Nd}", true, false),
    ("[:graph:]", r"\p{Z}\p{C}", true, true),
    ("[:lower:]", r"\p{Ll}", true, false),
    ("[:print:]", r"\p{C}", true, false),
    ("[:punct:]", r"\p{P}", true, false),
    ("[:space:]", r"\p{Z}\t\r\n\v\f", true, false),
    ("[:upper:]", r"\p{Lu}", true, false),
    ("[:word:]", r"\p{L}\p{Nl}\p{Nd}\p{Pc}", true, false),
    ("[:xdigit:]", "A-Fa-f0-9", false, false),
];
