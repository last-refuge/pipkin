//! Model-id glob semantics used by the pinned resolver: minimatch defaults + nocase.
//! No filesystem access. Components use ECMAScript regexes in a native Rust matcher.
mod braces;
mod component;

pub(super) struct Pattern {
    negate: bool,
    alternatives: Vec<Vec<Component>>,
}
enum Component {
    Globstar,
    Regex(regress::Regex, bool),
}

impl Pattern {
    pub(super) fn new(pattern: &str) -> Option<Self> {
        if pattern.starts_with('#') {
            return Some(Self {
                negate: false,
                alternatives: vec![],
            });
        }
        let bangs = pattern.bytes().take_while(|b| *b == b'!').count();
        let negate = bangs % 2 == 1;
        let mut alternatives = Vec::new();
        for expanded in braces::expand(&pattern[bangs..])? {
            let mut parts = Vec::new();
            for part in path_parts(&expanded) {
                if part == ".."
                    && parts
                        .last()
                        .is_some_and(|p| !["", ".", "..", "**"].contains(p))
                {
                    parts.pop();
                } else {
                    parts.push(part);
                }
            }
            let mut compiled = Vec::new();
            for part in parts {
                compiled.push(if part == "**" {
                    Component::Globstar
                } else {
                    {
                        let (regex, unicode) = component::compile(part)?;
                        Component::Regex(regex, unicode)
                    }
                });
            }
            alternatives.push(compiled);
        }
        Some(Self {
            negate,
            alternatives,
        })
    }

    pub(super) fn is_match(&self, value: &str) -> bool {
        let parts = path_parts(value);
        let units: Vec<Vec<u16>> = parts.iter().map(|p| p.encode_utf16().collect()).collect();
        self.alternatives.iter().any(|pattern| {
            // Reachable path positions after each component. No exponential globstar recursion.
            let mut positions = vec![false; parts.len() + 1];
            positions[0] = true;
            for component in pattern {
                let mut next = vec![false; parts.len() + 1];
                match component {
                    Component::Globstar => {
                        for i in 0..=parts.len() {
                            if positions[i] && i < parts.len() {
                                next[i] = true;
                            }
                            if i < parts.len() && next[i] && !parts[i].starts_with('.') {
                                next[i + 1] = true;
                            }
                        }
                    }
                    Component::Regex(re, unicode) => {
                        for i in 0..parts.len() {
                            if positions[i]
                                && if *unicode {
                                    re.find_from_utf16(&units[i], 0).next().is_some()
                                } else {
                                    re.find_from_ucs2(&units[i], 0).next().is_some()
                                }
                            {
                                next[i + 1] = true;
                            }
                        }
                    }
                }
                positions = next;
            }
            positions[parts.len()]
                || (parts.last() == Some(&"") && parts.len() > 1 && positions[parts.len() - 1])
        }) != self.negate
    }
}

fn path_parts(path: &str) -> Vec<&str> {
    if path.is_empty() {
        return vec![""];
    }
    let mut parts: Vec<_> = path.split('/').filter(|p| !p.is_empty()).collect();
    if path.starts_with('/') {
        parts.insert(0, "");
    }
    if path.ends_with('/') {
        parts.push("");
    }
    parts
}
