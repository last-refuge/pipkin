//! Brace alternatives and inclusive numeric/alphabetic sequences, before glob parsing.
//! Semantics follow brace-expansion 5.0.12; see its retained notice/provenance.
const MAX_RESULTS: usize = 100_000;
const MAX_UNITS: usize = 4_000_000;

pub(super) fn expand(pattern: &str) -> Option<Vec<String>> {
    // minimatch only calls brace-expansion when a brace pair is present.
    if !pattern.contains('{') || !pattern.contains('}') {
        return Some(vec![pattern.to_owned()]);
    }
    let pattern = if let Some(tail) = pattern.strip_prefix("{}") {
        format!(r"\{{\}}{tail}")
    } else {
        pattern.to_owned()
    };
    Some(expand_inner(&pattern, 0, true))
}

fn balanced(s: &str) -> Option<(usize, usize)> {
    let mut stack = Vec::new();
    let mut first = None;
    let mut escaped = false;
    for (i, c) in s.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' {
            escaped = true;
            continue;
        }
        if c == '{' {
            stack.push(i);
        } else if c == '}'
            && let Some(start) = stack.pop()
        {
            if stack.is_empty() {
                return Some((start, i));
            }
            first.get_or_insert((start, i));
        }
    }
    first
}
fn options(s: &str) -> Vec<String> {
    let (mut nesting, mut start, mut escaped) = (0, 0, false);
    let mut out = Vec::new();
    for (i, c) in s.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' {
            escaped = true;
            continue;
        }
        match c {
            '{' => nesting += 1,
            '}' => nesting -= 1,
            ',' if nesting == 0 => {
                out.push(s[start..i].to_owned());
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(s[start..].to_owned());
    out
}
fn combine(acc: &[String], prefix: &str, values: &[String], drop_empty: bool) -> Vec<String> {
    let mut out = Vec::new();
    let mut units = 0;
    for a in acc {
        for v in values {
            if out.len() == MAX_RESULTS {
                return out;
            }
            let s = format!("{a}{prefix}{v}");
            if drop_empty && s.is_empty() {
                continue;
            }
            units += s.encode_utf16().count();
            if units > MAX_UNITS {
                return out;
            }
            out.push(s);
        }
    }
    out
}
fn expand_inner(s: &str, depth: usize, mut top: bool) -> Vec<String> {
    if depth > 1000 {
        return vec![s.to_owned()];
    }
    let mut tail = s.to_owned();
    let mut acc = vec![String::new()];
    let (mut first, mut drop_empty, mut rewrites) = (true, false, 0);
    loop {
        let Some((start, end)) = balanced(&tail) else {
            return combine(&acc, &tail, &[String::new()], drop_empty);
        };
        let (pre, body, post) = (&tail[..start], &tail[start + 1..end], &tail[end + 1..]);
        if pre.ends_with('$') {
            acc = combine(
                &acc,
                &tail[..end + 1],
                &[String::new()],
                drop_empty && post.is_empty(),
            );
            first = false;
            if post.is_empty() {
                return acc;
            }
            tail = post.to_owned();
            continue;
        }
        let seq = sequence(body);
        let opts = options(body);
        let has_comma = opts.len() > 1 || body.contains(',');
        if seq.is_none() && !has_comma {
            if rewrites < 1000
                && post
                    .split_once(',')
                    .is_some_and(|(_, after)| !after.starts_with(',') && after.contains('}'))
            {
                rewrites += 1;
                tail = format!("{pre}{{{body}\\}}{post}");
                top = true;
                continue;
            }
            return combine(&acc, &tail, &[String::new()], drop_empty);
        }
        if first {
            drop_empty = top && seq.is_none();
            first = false;
        }
        let values = if let Some(seq) = seq {
            seq
        } else {
            let opts = if opts.len() == 1 {
                expand_inner(&opts[0], depth + 1, false)
                    .into_iter()
                    .map(|s| format!("{{{s}}}"))
                    .collect()
            } else {
                opts
            };
            let mut out = Vec::new();
            let mut units = 0;
            'outer: for opt in opts {
                for v in expand_inner(&opt, depth + 1, false) {
                    if drop_empty
                        && post.is_empty()
                        && pre.is_empty()
                        && acc.iter().all(String::is_empty)
                        && v.is_empty()
                    {
                        continue;
                    }
                    if out.len() == MAX_RESULTS || units + v.encode_utf16().count() > MAX_UNITS {
                        break 'outer;
                    }
                    units += v.encode_utf16().count();
                    out.push(v);
                }
            }
            out
        };
        acc = combine(&acc, pre, &values, drop_empty && post.is_empty());
        if post.is_empty() {
            return acc;
        }
        tail = post.to_owned();
    }
}

fn sequence(body: &str) -> Option<Vec<String>> {
    let p: Vec<_> = body.split("..").collect();
    if !(2..=3).contains(&p.len()) {
        return None;
    }
    let integer = |s: &str| {
        let s = s.strip_prefix('-').unwrap_or(s);
        !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit())
    };
    let numeric = integer(p[0]) && integer(p[1]);
    let alpha = p[0].len() == 1
        && p[1].len() == 1
        && p[0].as_bytes()[0].is_ascii_alphabetic()
        && p[1].as_bytes()[0].is_ascii_alphabetic();
    if !numeric && !alpha {
        return None;
    }
    let mut step = if p.len() == 3 && integer(p[2]) {
        p[2].parse::<i64>().ok()?.checked_abs()?.max(1)
    } else if p.len() == 2 {
        1
    } else {
        return None;
    };
    let (mut a, b) = if numeric {
        (p[0].parse::<i64>().ok()?, p[1].parse::<i64>().ok()?)
    } else {
        (p[0].as_bytes()[0] as i64, p[1].as_bytes()[0] as i64)
    };
    if a > b {
        step = -step;
    }
    let width = p[0].len().max(p[1].len());
    let padded = numeric
        && p[..2].iter().any(|s| {
            let s = s.strip_prefix('-').unwrap_or(s);
            s.len() > 1 && s.starts_with('0')
        });
    let mut out = Vec::new();
    let mut units = 0;
    while (step > 0 && a <= b) || (step < 0 && a >= b) {
        if out.len() >= MAX_RESULTS {
            break;
        }
        let value = if alpha {
            if a == 92 {
                String::new()
            } else {
                (a as u8 as char).to_string()
            }
        } else if padded {
            if a < 0 {
                format!("-{:0width$}", a.unsigned_abs(), width = width - 1)
            } else {
                format!("{a:0width$}")
            }
        } else {
            a.to_string()
        };
        units += value.len();
        if units > MAX_UNITS {
            break;
        }
        out.push(value);
        let Some(next) = a.checked_add(step) else {
            break;
        };
        a = next;
    }
    Some(out)
}
