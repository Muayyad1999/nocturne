# nocturne-textfsm — working notes (resume file)

Spec: `docs/dev/spec-textfsm.md`. Upstream: `/home/user/.cache/vendor/ntc-templates`
(commit `84ae82fd022807810d73a19655cca113b734de58`, 979 templates + `index`).
Python reference: `pip install textfsm` (2.1.1) — `textfsm/parser.py`, `clitable.py`, `texttable.py`.
Cargo: always `CARGO_TARGET_DIR=/home/user/.cache/nocturne-target/textfsm cargo <cmd> -p nocturne-textfsm`.
Formatting: `cargo fmt -p nocturne-textfsm` ONLY (never a bare `cargo fmt`; it would rewrite other crates).
Rules: touch only this crate, no git commands, do not edit the workspace Cargo.toml.
Scratch (not committed): `/tmp/claude-0/-home-user-netops-ai/862b16f9-1524-5810-bc17-cca256bcf369/scratchpad`
(`venv/` has textfsm+pyyaml; `baseline.json` = Python reference output for all 1897 cases, 0 failures upstream).

## Status (keep current)

Design decisions taken (see code docs for detail):
- `src/pyregex/`: full port of CPython's regex parser -> AST -> emit Rust syntax. Groups are
  emitted unnamed (names kept in our AST). `\s` -> `[\s\x1c-\x1f]`, `\w` -> `[\p{L}\p{N}_]`,
  `\d` -> `\d` (verified equal to Python over all code points), `\Z` -> `\z`, `\<`/`\>` literal.
  `regex` crate unless AST has look-around/backref/atomic/possessive/conditional -> `fancy-regex`.
- Run engine must assign EVERY named group incl. non-participating ones (None), model
  `value` separately from the List accumulator, and process options in declared order.
- List item None is rendered "None" (that is what clitable/ntc-templates return; corpus expects it).
- Index: 6 rows list several templates `a:b`; the corpus EXPECTS clitable's merge of extra
  columns (keyed on `Key` values) -> `ntc::parse` must merge; `ntc::find` returns the first.
- Platform cells may be regexes: `(hp|aruba)_procurve`, `.*vyos.*`, `cisco_(asa|ftd)`; matching is
  `re.match` (prefix). Bundle: flate2/zlib blob built by build.rs (1.2 MB raw -> ~261 KB).
- Corpus: 1897 cases (tests/<platform>/<command>/*.raw + .yml; skip tests/mocks). All YAML
  values are double-quoted strings or lists of them.

Done:
- [x] bundle copied into `templates/` (+ LICENSE, README.md)
- [x] `src/pyregex/{ast,parser,emit,mod}.rs` written, compiles, smoke test passes

In progress / next (in order):
- [ ] regex oracle test (scratch `py/regex_oracle.py` -> compare every rule regex vs Python)
- [ ] perf check of Unicode classes (compile time of big templates)
- [ ] `src/template.rs` (template parser), `src/fsm.rs` (run engine), `src/value.rs`, `src/error.rs`
- [ ] build.rs + `src/ntc/{bundle,index,merge,mod}.rs`
- [ ] unit tests (semantics verified against Python), `tests/conformance.rs`, `tests/fixtures*`
- [ ] clippy, fmt, timing measurement, final report

## Reference semantics that are easy to get wrong (from parser.py)

Template parsing
- Value line: `line.rstrip().split(' ')`; < 3 tokens = error. If token[2] does not start
  with `(` then token[1] is the option list (split on `,`, unknown/duplicate = error),
  name = token[2], regex = rest joined by ' '. Otherwise name = token[1].
- Name <= 48 chars. Regex must start with `(`, end with `)`, and not end with `\)`.
- Value block ends at the first blank line; `^\s*#` lines are comments; other text = error.
- State name: `^\w+$`, <= 48 chars, not a line/record operator name, unique.
  Rules follow until a blank line; each must start with ` ^`, `  ^` or `\t^`.
- Rule: strip; split at the LAST `\s->` (greedy `(.*)(\s->(.*))`). Match part goes through
  `string.Template.substitute`: `$$` -> `$`, `$name`, `${name}`; any other `$` or an unknown
  name is a template error.
- Action: `\s+LineOp(.RecOp)?(\s+NewState)?$` | `\s+RecOp(\s+NewState)?$` | `(\s+NewState)?$`
  where NewState = `\w+|".*"`. `Continue` + new state = error. Quoted new state only with Error.
- Validation: `Start` required; `End`/`EOF` must be empty; `End` is dropped; every target
  state must exist (except End/EOF and Error rules).

Run
- `text.splitlines()` (splits on \n, \r, \r\n, \v, \f, \x1c-\x1e, \x85, U+2028, U+2029).
- For each rule: `re.match` (anchored at start only). On match assign EVERY named group in
  the rule regex, including groups that did not participate (value becomes None; this
  resets Filldown memory and appends None to a List).
- Record op first (Record/Clear/Clearall), then line op. Error raises. Continue keeps the
  line and goes on with the next rule (no state change). Otherwise switch state and stop.
- After a line, stop if state is End or EOF. At the end: implicit Record unless state is
  End or the template declares an (empty) EOF state.
- Record: options run in declared order. Required: skip + clear when value is falsy
  (None, '' or []). Row is dropped (WITHOUT clearing) when every cell is None or [];
  a matched empty string '' counts as present. None cells become ''.
- Clear: value = None, Filldown restores last assigned value; List cleared unless Filldown.
- Clearall: everything including Filldown memory.
- Fillup: on assigning a truthy value, walk result rows from the end and fill the column
  until a row already has a truthy value there.
- List: appends each match; with nested named groups appends a dict of the groups.
  clitable then turns every cell into `str(...)` (list items individually).

clitable index
- CSV: header row, `#` comment lines, cells stripped, rows with wrong cell count dropped.
- Command cell: `[[xyz]]` -> `(x(y(z)?)?)?`; Platform/Command/Hostname are regexes used with
  `re.match` (prefix match!). First matching row wins. Template cell may be `a:b` (use first).
- ntc `parse_output` lower-cases header names for the dict keys.
- Upstream test: platform = dir, command = dir name with `_` -> ' ', raw read in text mode
  (universal newlines), compared to `parsed_sample` in the .yml.
