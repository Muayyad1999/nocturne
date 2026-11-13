#!/usr/bin/env python3
"""Record every regex evaluation the Python reference performs on the corpus.

    pip install textfsm
    python3 tools/regex_oracle.py /path/to/ntc-templates <out.jsonl>
    PYREGEX_ORACLE=<out.jsonl> cargo test --release -p nocturne-textfsm \\
        agrees_with_python_on_the_corpus -- --nocapture

Runs every test case of the ntc-templates checkout through the reference
TextFSM and notes, for each rule the engine tries on each input line, whether
the rule's regex matched and what its groups captured. The Rust test replays
all of it.

One JSON object per template:
  {"template": name,
   "patterns": [rule regex after ${VALUE} substitution, in state order],
   "values": [value regexes],
   "lines": [distinct input lines],
   "misses": [[pattern_index, line_index], ...],
   "hits": [[pattern_index, line_index, whole_match, [groups...]], ...]}
"""

import collections
import glob
import json
import os
import sys

import textfsm
from textfsm import clitable


class TracingFSM(textfsm.TextFSM):
    def __init__(self, template, sink):
        super().__init__(template)
        self._sink = sink
        self._rule_index = {}
        index = 0
        for state in self.state_list:
            for rule in self.states[state]:
                self._rule_index[id(rule)] = index
                index += 1

    def _CheckRule(self, rule, line):
        matched = rule.regex_obj.match(line)
        key = (self._rule_index[id(rule)], line)
        if key not in self._sink:
            self._sink[key] = None if matched is None else (matched.group(0), list(matched.groups()))
        return matched


def main():
    root, out_path = sys.argv[1], sys.argv[2]
    template_dir = os.path.join(root, "ntc_templates", "templates")
    table = clitable.CliTable("index", template_dir)

    sinks = collections.defaultdict(dict)
    for raw_file in sorted(glob.glob(os.path.join(root, "tests", "*", "*", "*.raw"))):
        rel = os.path.relpath(raw_file, os.path.join(root, "tests")).split(os.sep)
        platform, command = rel[0], " ".join(rel[1].split("_"))
        row = table.index.GetRowMatch({"Command": command, "Platform": platform})
        assert row, (platform, command)
        with open(raw_file, "r", encoding="utf-8") as fh:
            text = fh.read()
        for name in table.index.index[row]["Template"].split(":"):
            with open(os.path.join(template_dir, name), "r", encoding="utf-8") as fh:
                fsm = TracingFSM(fh, sinks[name])
            fsm.ParseText(text)

    attempts = hits = 0
    with open(out_path, "w", encoding="utf-8") as out:
        for path in sorted(glob.glob(os.path.join(template_dir, "*.textfsm"))):
            name = os.path.basename(path)
            with open(path, "r", encoding="utf-8") as fh:
                fsm = textfsm.TextFSM(fh)
            patterns = [rule.regex for state in fsm.state_list for rule in fsm.states[state]]
            line_index = {}
            entry = {
                "template": name[: -len(".textfsm")],
                "patterns": patterns,
                "values": [value.regex for value in fsm.values],
                "lines": [],
                "misses": [],
                "hits": [],
            }
            for (pattern_index, line), outcome in sinks.get(name, {}).items():
                if line not in line_index:
                    line_index[line] = len(entry["lines"])
                    entry["lines"].append(line)
                attempts += 1
                if outcome is None:
                    entry["misses"].append([pattern_index, line_index[line]])
                else:
                    hits += 1
                    entry["hits"].append([pattern_index, line_index[line], outcome[0], outcome[1]])
            out.write(json.dumps(entry, ensure_ascii=False))
            out.write("\n")
    print("templates", len(sinks), "distinct attempts", attempts, "hits", hits)


if __name__ == "__main__":
    main()
