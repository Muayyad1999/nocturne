//! Tests of the regex layer.
//!
//! Wherever an expectation describes what Python does, it was produced with
//! the `re` module of CPython 3.13.

use super::emit::translate;
use super::parser::parse;
use super::*;

/// The whole match followed by every group, like `(m.group(0), *m.groups())`.
fn groups(pattern: &str, text: &str) -> Option<Vec<Option<String>>> {
    let regex = Regex::new(pattern).unwrap_or_else(|error| panic!("{pattern:?}: {error}"));
    let captures = regex.match_start(text).unwrap()?;
    Some((0..=regex.group_count()).map(|group| captures.get(group).map(str::to_owned)).collect())
}

/// Shorthand for the expected value of [`groups`].
fn found(expected: &[Option<&str>]) -> Option<Vec<Option<String>>> {
    Some(expected.iter().map(|group| group.map(str::to_owned)).collect())
}

/// The pattern as handed to the Rust engines, without the anchoring wrapper.
fn translated(pattern: &str) -> String {
    let translation = translate(&parse(pattern).unwrap());
    let inner = translation.pattern.strip_prefix(r"\A(?").and_then(|rest| rest.strip_suffix(')'));
    inner.unwrap_or_else(|| panic!("unexpected wrapper: {}", translation.pattern)).to_owned()
}

fn syntax_error(pattern: &str) -> (String, usize) {
    match Regex::new(pattern) {
        Err(RegexError::Syntax(error)) => (error.message, error.position),
        other => panic!("{pattern:?}: expected a syntax error, got {other:?}"),
    }
}

#[test]
fn matches_at_the_start_only() {
    assert_eq!(groups("b", "ab"), None);
    assert_eq!(groups("a", "ab"), found(&[Some("a")]));
    // Every alternative is anchored, not just the first one.
    assert_eq!(groups("a|b", "xb"), None);
    assert_eq!(groups("x|b", "bx"), found(&[Some("b")]));
    assert_eq!(groups("", "anything"), found(&[Some("")]));
    assert_eq!(groups("a$", "ab"), None);
    assert_eq!(groups("ab$", "ab"), found(&[Some("ab")]));
}

#[test]
fn reports_groups_by_python_number() {
    assert_eq!(
        groups(r"(?P<a>x)(y)?(?:(z)|(w))", "xw"),
        found(&[Some("xw"), Some("x"), None, None, Some("w")])
    );
    let regex = Regex::new(r"(?P<first>a)(b)(?P<third>c)").unwrap();
    assert_eq!(regex.named_groups().collect::<Vec<_>>(), [(1, "first"), (3, "third")]);
    assert_eq!(regex.group_count(), 3);
    let captures = regex.match_start("abc").unwrap().unwrap();
    assert_eq!(captures.get(4), None);
}

#[test]
fn translates_classes_to_their_python_meaning() {
    assert_eq!(translated(r"\d\D"), r":\d\D");
    assert_eq!(translated(r"\s\S"), r":[\s\x1c-\x1f][^\s\x1c-\x1f]");
    assert_eq!(translated(r"\w\W"), r":[\p{L}\p{N}_][^\p{L}\p{N}_]");
    assert_eq!(translated(r"[\w\s.-]"), r":[\p{L}\p{N}_\s\x1c-\x1f.\-]");
    assert_eq!(translated(r"[^\S\d]"), r":[^[^\s\x1c-\x1f]\d]");
    assert_eq!(translated(r"[\W\D]"), r":[[^\p{L}\p{N}_]\D]");
    assert_eq!(translated(r"[a-f\x00-\x1f\]]"), r":[a-f\x00-\x1f\]]");
    assert_eq!(translated(r"[[\]&~^-]"), r":[\[\]\&\~\^\-]");
    assert_eq!(translated(r"[]a]"), r":[\]a]");
    assert_eq!(translated(r"[^]a]"), r":[^\]a]");
    assert_eq!(translated(r"[+--]"), r":[+-\-]");
    assert_eq!(translated(r"[\b\n]"), r":[\x08\x0a]");
}

#[test]
fn translates_literals_and_assertions() {
    // Escaped punctuation is plain text in Python.
    assert_eq!(translated(r"\<a\>\-\ \#\%"), r":<a>- #%");
    assert_eq!(translated(r"a\.b\\c\/d"), r":a\.b\\c/d");
    assert_eq!(translated(r"}]"), r":\}\]");
    assert_eq!(translated(r"\t\x41é\101\0"), ":\\x09A\u{e9}A\\x00");
    assert_eq!(translated(r"^a$\A\b\B\Z"), r":^a$\A\b\B\z");
}

#[test]
fn translates_quantifiers() {
    assert_eq!(translated(r"a*b+c?d{2}e{2,}f{2,5}g{,3}"), r":a*b+c?d{2}e{2,}f{2,5}g{0,3}");
    assert_eq!(translated(r"a*?b+?c??d{2,3}?"), r":a*?b+?c??d{2,3}?");
    assert_eq!(translated(r"a{,}"), r":a*");
    // Braces that do not form a quantifier are literal.
    assert_eq!(translated(r"a{b{1,c{}{2"), r":a\{b\{1,c\{\}\{2");
    assert_eq!(translated(r"(?:ab)+(a|b)*[ab]?"), r":(?:ab)+(a|b)*[ab]?");
    // A comment is not an item: the quantifier applies to what precedes it.
    assert_eq!(translated(r"ab(?#comment)+"), r":ab+");
}

#[test]
fn translates_groups_and_flags() {
    assert_eq!(translated(r"(?P<n>a)(b)(?:c)"), r":(a)(b)(?:c)");
    assert_eq!(translated(r"(?i:a(?s-i:.))"), r":(?i:a(?s-i:.))");
    assert_eq!(translated(r"(?i)a"), r"i:a");
    assert_eq!(translated(r"(?ms)(?i)a"), r"ims:a");
    assert_eq!(translated(r"(?x) a b # c"), r":ab");
    assert_eq!(translated(r"(?x: a [ ] \  b )c d"), r":(?:a[ ] b)c d");
    assert_eq!(translated(r"(?u:a)"), r":(?:a)");
    assert_eq!(translated(r"a||b"), r":a||b");
    assert_eq!(translated(r"(|a)"), r":(|a)");
}

#[test]
fn plain_patterns_stay_off_the_backtracking_engine() {
    for pattern in [r"^\s+(?P<A>\S+)\s*$", r"(a|b)*c{2,3}", r"(?i:x)[^y]\b"] {
        assert!(!Regex::new(pattern).unwrap().uses_backtracking(), "{pattern}");
    }
    for pattern in [r"a(?=b)", r"a(?!b)", r"(?<=a)b", r"(?<!a)b", r"(a)\1", r"(?P<n>a)(?P=n)", r"(?>a)", r"a*+", r"(a)?(?(1)b|c)"] {
        assert!(Regex::new(pattern).unwrap().uses_backtracking(), "{pattern}");
    }
}

#[test]
fn translates_backtracking_constructs() {
    assert_eq!(translated(r"(?P<n>a)(?P=n)\1"), r":(a)(?:\1)(?:\1)");
    assert_eq!(translated(r"(a)?(?(1)b|c)(?(1)d)"), r":(a)?(?(1)b|c)(?(1)d)");
    assert_eq!(translated(r"(?>a|b)c++d{2,3}+"), r":(?>a|b)(?>c+)(?>d{2,3})");
    assert_eq!(translated(r"(?=a)(?!b)(?<=c)(?<!d)"), r":(?=a)(?!b)(?<=c)(?<!d)");
    // A repeated look-around is evaluated once, or optionally.
    assert_eq!(translated(r"(?=a)+(?!b)*(?<=c)??"), r":(?=a)(?:(?!b)|)(?:|(?<=c))");
}

#[test]
fn marks_the_end_of_loops_with_an_empty_final_iteration() {
    assert_eq!(translated(r"(a*)*b"), r":(a*)*()b");
    let translation = translate(&parse(r"(x)(?:(a*)(b?))+(y)").unwrap());
    assert_eq!(translation.pattern, r"\A(?:(x)(?:(a*)(b?))+()(y))");
    let slots: Vec<_> =
        translation.groups.iter().map(|slot| (slot.primary, slot.empty_iteration)).collect();
    assert_eq!(slots, [(1, None), (2, Some(4)), (3, Some(4)), (5, None)]);
    // Back-references follow the renumbering.
    assert_eq!(translated(r"(a*)*(b)\2"), r":(a*)*()(b)(?:\3)");
}

/// Every character must mean itself after translation, in both engines, as a
/// literal, inside a class and inside a negated class.
#[test]
fn literals_survive_translation_on_both_engines() {
    let mut characters: Vec<char> = (0u8..=0x7f).map(char::from).collect();
    characters.extend(['\u{85}', '\u{a0}', 'é', 'ß', '€', '日', '\u{2028}', '\u{feff}', '😀']);
    for &c in &characters {
        // Letters and digits stand for themselves; everything else may be escaped.
        let literal = if c.is_ascii_alphanumeric() { c.to_string() } else { format!("\\{c}") };
        let other = if c == 'x' { 'y' } else { 'x' };
        // The look-ahead forces the backtracking engine.
        for suffix in ["", "(?=)"] {
            let context = format!("{c:?} with suffix {suffix:?}");
            let is = |pattern: String, text: char| {
                let regex = Regex::new(&pattern).unwrap_or_else(|error| panic!("{context}: {error}"));
                assert_eq!(regex.uses_backtracking(), !suffix.is_empty(), "{context}");
                regex.match_start(&text.to_string()).unwrap().is_some()
            };
            assert!(is(format!("{literal}{suffix}$"), c), "literal {context}");
            assert!(!is(format!("{literal}{suffix}$"), other), "literal {context}");
            assert!(is(format!("[{literal}]{suffix}$"), c), "class {context}");
            assert!(!is(format!("[{literal}]{suffix}$"), other), "class {context}");
            assert!(is(format!("[q{literal}]{suffix}$"), c), "class {context}");
            assert!(!is(format!("[^{literal}]{suffix}$"), c), "negated class {context}");
            assert!(is(format!("[^{literal}]{suffix}$"), other), "negated class {context}");
        }
    }
}

#[test]
fn classes_match_what_python_matches() {
    let matches = |pattern: &str, text: &str| groups(pattern, text).is_some();
    // \s: Unicode white space and the separators \x1c-\x1f.
    for space in [" ", "\t", "\n", "\x0b", "\x0c", "\r", "\x1c", "\x1f", "\u{85}", "\u{a0}", "\u{2003}", "\u{3000}"] {
        assert!(matches(r"\s$", space) && !matches(r"\S$", space), "{space:?}");
        assert!(matches(r"[\s]$", space) && !matches(r"[\S]$", space), "{space:?}");
    }
    for not_space in ["a", "\x1b", "\u{200b}", "_"] {
        assert!(!matches(r"\s$", not_space) && matches(r"\S$", not_space), "{not_space:?}");
    }
    // \w: letters, every kind of number, and the underscore.
    for word in ["a", "Z", "0", "_", "é", "ß", "日", "٣", "②", "½", "Ⅷ", "ª"] {
        assert!(matches(r"\w$", word) && !matches(r"\W$", word), "{word:?}");
        assert!(matches(r"[\w]$", word) && !matches(r"[^\w]$", word), "{word:?}");
    }
    for not_word in ["-", " ", "‿", "\u{301}", "€", "\u{200d}"] {
        assert!(!matches(r"\w$", not_word) && matches(r"\W$", not_word), "{not_word:?}");
    }
    // \d: decimal digits of any script, nothing else.
    for digit in ["0", "9", "٣", "५"] {
        assert!(matches(r"\d$", digit) && !matches(r"\D$", digit), "{digit:?}");
    }
    for not_digit in ["a", "②", "½", "Ⅷ"] {
        assert!(!matches(r"\d$", not_digit) && matches(r"\D$", not_digit), "{not_digit:?}");
    }
    // Ignoring case.
    assert!(matches(r"(?i:straße)$", "STRAßE") && matches(r"(?i:[a-z]+)$", "Abc"));
    assert!(matches(r"(?i:é)$", "É") && !matches(r"é$", "É"));
}

/// Messages and positions as reported by `re.error` (`msg`, `pos`).
#[test]
fn syntax_errors_are_those_of_python() {
    let cases: &[(&str, &str, usize)] = &[
        (r"a**", "multiple repeat", 2),
        (r"a+*", "multiple repeat", 2),
        (r"a{2}{3}", "multiple repeat", 4),
        (r"a*??", "multiple repeat", 3),
        (r"x{1,2}{3}", "multiple repeat", 6),
        (r"x{1,2}?+", "multiple repeat", 7),
        (r"*a", "nothing to repeat", 0),
        (r"(*a)", "nothing to repeat", 1),
        (r"a|*", "nothing to repeat", 2),
        (r"^*", "nothing to repeat", 1),
        (r"\b+", "nothing to repeat", 2),
        (r"a{2,1}", "min repeat greater than max repeat", 2),
        (r"[a", "unterminated character set", 0),
        (r"[", "unterminated character set", 0),
        (r"[a-", "unterminated character set", 0),
        (r"[]", "unterminated character set", 0),
        (r"[^]", "unterminated character set", 0),
        (r"[z-a]", "bad character range z-a", 1),
        (r"[\w-z]", r"bad character range \w-z", 1),
        (r"[a-\d]", r"bad character range a-\d", 1),
        (r"\y", r"bad escape \y", 0),
        (r"\x1", r"incomplete escape \x1", 0),
        (r"\u12", r"incomplete escape \u12", 0),
        (r"\U0011ffff", r"bad escape \U0011ffff", 0),
        (r"[\8]", r"bad escape \8", 1),
        (r"\777", r"octal escape value \777 outside of range 0-0o377", 0),
        (r"[\777]", r"octal escape value \777 outside of range 0-0o377", 1),
        ("abc\\", "bad escape (end of pattern)", 3),
        (r"(a", "missing ), unterminated subpattern", 0),
        (r"(?>a", "missing ), unterminated subpattern", 0),
        (r"(?!", "missing ), unterminated subpattern", 0),
        (r"a)", "unbalanced parenthesis", 1),
        (r"(?", "unexpected end of pattern", 2),
        (r"(?P", "unexpected end of pattern", 3),
        (r"(?<", "unexpected end of pattern", 3),
        (r"(?Px)", "unknown extension ?Px", 1),
        (r"(?P>a)", "unknown extension ?P>", 1),
        (r"(?<a)", "unknown extension ?<a", 1),
        (r"(?z)", "unknown extension ?z", 1),
        (r"(?z:a)", "unknown extension ?z", 1),
        (r"(?P<", "missing group name", 4),
        (r"(?P<>a)", "missing group name", 4),
        (r"(?P<1a>x)", "bad character in group name '1a'", 4),
        (r"(?P<a-b>x)", "bad character in group name 'a-b'", 4),
        (r"(?P<a>x)(?P<a>y)", "redefinition of group name 'a' as group 2; was group 1", 12),
        (r"(?P=b)", "unknown group name 'b'", 4),
        (r"(?P<n>a(?P=n))", "cannot refer to an open group", 11),
        (r"(a\1)", "cannot refer to an open group", 2),
        (r"\2", "invalid group reference 2", 1),
        (r"(a)\2", "invalid group reference 2", 4),
        (r"\10", "invalid group reference 10", 1),
        (r"a(?i)", "global flags not at the start of the expression", 1),
        (r"(?i", "missing -, : or )", 3),
        (r"(?i-i:a)", "bad inline flags: flag turned on and off", 5),
        (r"(?-:a)", "missing flag", 3),
        (r"(?i-:a)", "missing flag", 4),
        (r"(?ix-", "missing flag", 5),
        (r"(?-u:a)", "bad inline flags: cannot turn off flags 'a', 'u' and 'L'", 4),
        (r"(?L)a", "bad inline flags: cannot use 'L' flag with a str pattern", 3),
        (r"(?#abc", "missing ), unterminated comment", 0),
        (r"(?(1)a|b)", "invalid group reference 1", 3),
        (r"(a)(?(1)b|c|d)", "conditional backref with more than two branches", 11),
        (r"(?(n)a)", "unknown group name 'n'", 3),
        (r"(?(0)a)", "bad group number", 3),
        (r"(?(1", "missing ), unterminated name", 3),
        (r"(a)(?<=(b)\2)", "cannot refer to group defined in the same lookbehind subpattern", 12),
    ];
    let mut wrong = Vec::new();
    for &(pattern, message, position) in cases {
        let actual = syntax_error(pattern);
        if actual != (message.to_owned(), position) {
            wrong.push(format!("{pattern:?}: {actual:?}, expected {:?}", (message, position)));
        }
    }
    assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
}

#[test]
fn rejects_what_cannot_be_translated() {
    // Python reports no position for this one.
    assert_eq!(syntax_error(r"x(?<=a+)").0, "look-behind requires fixed-width pattern");
    assert_eq!(syntax_error(r"(?<=a|bc)").0, "look-behind requires fixed-width pattern");
    // Python raises OverflowError.
    assert_eq!(syntax_error(r"a{99999999999}").0, "the repetition number is too large");
    // Valid in Python, not supported here.
    assert_eq!(syntax_error(r"(?a)b").0, "the ASCII flag (?a) is not supported");
    assert_eq!(syntax_error(r"\N{DASH}").0, r"named Unicode escapes (\N{...}) are not supported");
}

#[test]
fn accepts_what_python_accepts() {
    // Group references inside look-behinds, octal escapes, odd but legal classes.
    for pattern in [
        r"(a)(?<=\1b(c))\2",
        r"(?<=(a))\1",
        r"(?<=ab|cd)e",
        r"(?<=\d{3})x",
        r"(a)\1|\0\01\012\377",
        r"[\0-\377][a\-z][-a][a-][--a][]-a]",
        r"(?i)(?m)(?s)(?x)(?u)a",
        r"(?:)*(?:^)*()+",
        r"a{0}b{0,0}c{1}",
        r"(?P<_x1>a)(?P<é>b)(?P=é)",
    ] {
        if let Err(error) = Regex::new(pattern) {
            panic!("{pattern:?}: {error}");
        }
    }
}

/// `(pattern, text, [whole match, groups...])` as matched by Python.
const LOOPS_OVER_EMPTY_MATCHES: &[(&str, &str, &[Option<&str>])] = &[
    (r"(a*)*", "aaa", &[Some("aaa"), Some("")]),
    (r"(a*)*", "b", &[Some(""), Some("")]),
    (r"(a*)+", "b", &[Some(""), Some("")]),
    (r"(a|)*", "aa", &[Some("aa"), Some("")]),
    (r"(\s*-*)*\s*$", "-------   -----------", &[Some("-------   -----------"), Some("")]),
    (r"((a)|b*)*", "abb", &[Some("abb"), Some(""), Some("a")]),
    (r"((a)|(b*))*", "abb", &[Some("abb"), Some(""), Some("a"), Some("")]),
    (r"(?:(a)|b)*", "ab", &[Some("ab"), Some("a")]),
    (r"(a?)*", "aa", &[Some("aa"), Some("")]),
    (r"(a?){2,3}", "a", &[Some("a"), Some("")]),
    (r"(a?){3}", "a", &[Some("a"), Some("")]),
    (r"(?:(a)?)*", "aab", &[Some("aa"), Some("a")]),
    (r"(?:(a)|())*", "aa", &[Some("aa"), Some("a"), Some("")]),
    (r"(a*)+$", "aaa", &[Some("aaa"), Some("")]),
    (r"(\w*\s*)*$", "ab cd", &[Some("ab cd"), Some("")]),
    (r"(\w+\s*)*$", "ab cd", &[Some("ab cd"), Some("cd")]),
    (r"(\S*)\s*(\S*)*", "ab cd", &[Some("ab cd"), Some("ab"), Some("")]),
    (r"(a|b*){2,3}", "abb", &[Some("abb"), Some("")]),
    (r"(a|b*){2,}", "abb", &[Some("abb"), Some("")]),
    (r"(a|b*)*", "abb", &[Some("abb"), Some("")]),
    (r"(a|b*)+c", "abbc", &[Some("abbc"), Some("")]),
    (r"(a*){2,}", "aa", &[Some("aa"), Some("")]),
    (r"(a*)*?", "aa", &[Some(""), None]),
    (r"(a*)*?b", "aab", &[Some("aab"), Some("aa")]),
    (r"(a*)+?b", "aab", &[Some("aab"), Some("aa")]),
    (r"(?:(a*)(b*))*", "aabb", &[Some("aabb"), Some(""), Some("")]),
    (r"(?:(a*)(b*))*c", "aabbc", &[Some("aabbc"), Some(""), Some("")]),
    (r"((a*)(b*))*c", "aabbabc", &[Some("aabbabc"), Some(""), Some(""), Some("")]),
    (r"(b|a*)*", "aab", &[Some("aab"), Some("")]),
    (r"(a?b?)*", "abab", &[Some("abab"), Some("")]),
    (r"(a?b?)*c", "ababc", &[Some("ababc"), Some("")]),
    (r"(?:a?(b?))*c", "abac", &[Some("abac"), Some("")]),
    (r"(x*)(?:(a*))*", "xaa", &[Some("xaa"), Some("x"), Some("")]),
    (r"(?:(a*)*b)*", "aabab", &[Some("aabab"), Some("")]),
    (r"((?:a*)*)", "aa", &[Some("aa"), Some("aa")]),
    (r"(a*)*(a*)", "aa", &[Some("aa"), Some(""), Some("")]),
    (r"(a{0,2})*", "aaaaa", &[Some("aaaaa"), Some("")]),
    (r"(a*+)*", "aa", &[Some("aa"), Some("")]),
    (r"(?>(a*))*", "aa", &[Some("aa"), Some("")]),
    (r"(a*)*+", "aa", &[Some("aa"), Some("")]),
    (r"(?:b|(a*))*", "aab", &[Some("aab"), Some("")]),
    (r"(^a*)*", "aa", &[Some("aa"), Some("aa")]),
    (r"(?i:(A*))*", "aa", &[Some("aa"), Some("")]),
    (r"((?=a)a*|b*)*", "aabb", &[Some("aabb"), Some("")]),
];

#[test]
fn loops_end_with_the_empty_iteration_python_takes() {
    for &(pattern, text, expected) in LOOPS_OVER_EMPTY_MATCHES {
        assert_eq!(groups(pattern, text), found(expected), "{pattern:?} on {text:?}");
    }
}

/// The documented difference (see `loops.rs`): `(pattern, text, Python, this crate)`.
#[test]
fn known_difference_for_other_loops_over_empty_matches() {
    let cases: &[(&str, &str, &[Option<&str>], &[Option<&str>])] = &[
        (r"(a*|b)*", "aab", &[Some("aa"), Some("")], &[Some("aab"), Some("b")]),
        (r"(?:(a*)|b)*", "aab", &[Some("aa"), Some("")], &[Some("aab"), Some("aa")]),
        (r"(|a)*b", "aab", &[Some("aab"), Some("")], &[Some("aab"), Some("a")]),
        (r"(a*?)*b", "aab", &[Some("aab"), Some("")], &[Some("aab"), Some("aa")]),
        (r"(a??)*b", "aab", &[Some("aab"), Some("")], &[Some("aab"), Some("a")]),
        (r"(a*$)*", "aa", &[Some("aa"), Some("")], &[Some("aa"), Some("aa")]),
        (r"(a*\b)*", "aa", &[Some("aa"), Some("")], &[Some("aa"), Some("aa")]),
    ];
    for &(pattern, text, python, ours) in cases {
        assert_ne!(python, ours);
        assert_eq!(groups(pattern, text), found(ours), "{pattern:?} on {text:?}");
    }
}

/// Replays every regex evaluation the Python reference performs on the upstream
/// corpus. `PYREGEX_ORACLE` points at the file written by `tools/regex_oracle.py`.
#[test]
fn agrees_with_python_on_the_corpus() {
    let Some(path) = std::env::var_os("PYREGEX_ORACLE") else {
        eprintln!("PYREGEX_ORACLE is not set; skipping the regex oracle test");
        return;
    };
    #[derive(serde::Deserialize)]
    struct Entry {
        template: String,
        patterns: Vec<String>,
        values: Vec<String>,
        lines: Vec<String>,
        misses: Vec<(usize, usize)>,
        hits: Vec<(usize, usize, String, Vec<Option<String>>)>,
    }

    let oracle = std::fs::read_to_string(path).unwrap();
    let mut failures = Vec::new();
    let (mut patterns, mut attempts, mut backtracking_patterns) = (0, 0, 0);
    let mut backtracking_templates = Vec::new();
    let started = std::time::Instant::now();
    let mut compile_time = std::time::Duration::ZERO;
    for line in oracle.lines() {
        let entry: Entry = serde_json::from_str(line).unwrap();
        for value in &entry.values {
            if let Err(error) = Regex::new(value) {
                failures.push(format!("{}: value {value:?}: {error}", entry.template));
            }
        }
        let compile_started = std::time::Instant::now();
        let compiled: Vec<_> = entry.patterns.iter().map(|pattern| Regex::new(pattern)).collect();
        compile_time += compile_started.elapsed();
        patterns += compiled.len();
        let fancy = compiled.iter().flatten().filter(|regex| regex.uses_backtracking()).count();
        if fancy > 0 {
            backtracking_patterns += fancy;
            backtracking_templates.push(entry.template.clone());
        }
        for (pattern, result) in entry.patterns.iter().zip(&compiled) {
            if let Err(error) = result {
                failures.push(format!("{}: {pattern:?}: {error}", entry.template));
            }
        }
        for &(pattern, line) in &entry.misses {
            attempts += 1;
            let Ok(regex) = &compiled[pattern] else { continue };
            match regex.match_start(&entry.lines[line]) {
                Ok(None) => {}
                Ok(Some(captures)) => failures.push(format!(
                    "{}: {:?} must not match {:?} (matched {:?})",
                    entry.template,
                    entry.patterns[pattern],
                    entry.lines[line],
                    captures.get(0)
                )),
                Err(error) => failures.push(format!("{}: {error}", entry.template)),
            }
        }
        for (pattern, line, whole, groups) in &entry.hits {
            attempts += 1;
            let Ok(regex) = &compiled[*pattern] else { continue };
            match regex.match_start(&entry.lines[*line]) {
                Ok(Some(captures)) => {
                    let actual: Vec<_> = (1..=regex.group_count())
                        .map(|group| captures.get(group).map(str::to_owned))
                        .collect();
                    if captures.get(0) != Some(whole.as_str()) || &actual != groups {
                        failures.push(format!(
                            "{}: {:?} on {:?}: expected {whole:?} {groups:?}, got {:?} {actual:?}",
                            entry.template,
                            entry.patterns[*pattern],
                            entry.lines[*line],
                            captures.get(0)
                        ));
                    }
                }
                Ok(None) => failures.push(format!(
                    "{}: {:?} must match {:?}",
                    entry.template, entry.patterns[*pattern], entry.lines[*line]
                )),
                Err(error) => failures.push(format!("{}: {error}", entry.template)),
            }
        }
    }
    eprintln!(
        "regex oracle: {patterns} patterns ({backtracking_patterns} on the backtracking engine, in {} templates), \
         {attempts} evaluations, {} failures, compile {compile_time:?}, total {:?}",
        backtracking_templates.len(),
        failures.len(),
        started.elapsed()
    );
    eprintln!("templates using the backtracking engine: {backtracking_templates:?}");
    for failure in failures.iter().take(40) {
        eprintln!("  {failure}");
    }
    assert!(failures.is_empty(), "{} disagreements with the Python reference", failures.len());
}
