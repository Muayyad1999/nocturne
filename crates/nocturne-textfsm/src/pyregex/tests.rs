use super::*;

#[test]
fn smoke() {
    let regex = Regex::new(r"^\s+(?P<A>\d+)$").unwrap();
    let captures = regex.match_start("  42").unwrap().unwrap();
    assert_eq!(captures.get(1), Some("42"));
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
