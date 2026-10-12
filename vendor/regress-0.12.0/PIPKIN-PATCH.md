# Pipkin's regress patch

Base: crates.io `regress` 0.12.0 archive, SHA-256
`32eef8b209c3c1c15dbad02c1f30f9539f00dc7253e0cbcdaae442a50a09d7c1`.
Upstream: https://github.com/ridiculousfish/regress. Retained upstream notices apply.

Native model globs require ECMAScript legacy (non-`u`) case folding. The upstream
`fold_code_point` and `unfold_uppercase_char` mapped non-ASCII characters such as
U+017F (long s) and U+0131 (dotless i) to ASCII S/I. ECMAScript Canonicalize excludes
that conversion in non-Unicode mode. The local patch applies this exclusion to both
forward and reverse folding; Unicode folding is unchanged. The parser also joins adjacent
surrogate escapes only in Unicode mode; legacy mode keeps the two UTF-16 units distinct,
as required for astral literals/classes and `?` matching. This is a local patch,
not a claim of an upstream release.

Executed Pi/minimatch fixtures in `fixtures/native-model-scope` exercise the behavior
through Pipkin's actual model resolver, including both legacy and Unicode classes.
