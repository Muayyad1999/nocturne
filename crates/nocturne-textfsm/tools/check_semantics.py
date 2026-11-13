#!/usr/bin/env python3
"""Check the engine-semantics test cases against the Python reference.

    pip install textfsm pyyaml
    python3 tools/check_semantics.py tests/semantics/*.yml

Every case in those files states what the reference implementation
(google/textfsm) does for a template and an input; `tests/semantics.rs` holds
the Rust engine to the same expectations. This script proves the expectations
themselves, so run it whenever a case is added or changed.

Rows are compared the way ntc-templates hands them out: every cell passed
through `str()`, list items individually (as `textfsm.clitable` does).
"""

import io
import re
import sys

import textfsm
import yaml

ERROR_WITH_MESSAGE = re.compile(r"Error: (.*)\. Rule Line: (\d+)\. Input Line: (.*)\.$", re.S)
ERROR_WITHOUT_MESSAGE = re.compile(r"State Error raised\. Rule Line: (\d+)\. Input Line: (.*)$", re.S)
TEMPLATE_LINE = re.compile(r"Line:? (\d+)")


def cells(row):
    return [[str(item) for item in cell] if isinstance(cell, list) else str(cell) for cell in row]


def unquote(message):
    if len(message) >= 2 and message[0] == '"' and message[-1] == '"':
        return message[1:-1]
    return message


def run(case):
    """Returns what the reference does, in the vocabulary of the case files."""
    try:
        fsm = textfsm.TextFSM(io.StringIO(case["template"]))
    except textfsm.TextFSMTemplateError as error:
        line = TEMPLATE_LINE.search(str(error))
        return {"template_error": {"line": int(line.group(1)) if line else None, "text": str(error)}}
    try:
        rows = fsm.ParseText(case.get("input", ""))
    except textfsm.TextFSMError as error:
        with_message = ERROR_WITH_MESSAGE.match(str(error))
        if with_message:
            message, rule_line, input_line = with_message.groups()
            return {"run_error": {"message": unquote(message), "rule_line": int(rule_line), "input_line": input_line}}
        rule_line, input_line = ERROR_WITHOUT_MESSAGE.match(str(error)).groups()
        return {"run_error": {"message": None, "rule_line": int(rule_line), "input_line": input_line}}
    return {"header": fsm.header, "rows": [cells(row) for row in rows]}


def check(case):
    actual = run(case)
    problems = []
    if "template_error" in case:
        if "template_error" not in actual:
            return ["expected a template error, got %r" % actual]
        expected_line = case["template_error"].get("line")
        actual_line = actual["template_error"]["line"]
        # The reference does not report a line for every error.
        if expected_line is not None and actual_line is not None and expected_line != actual_line:
            problems.append("template error on line %s, case says %s (%s)" % (
                actual_line, expected_line, actual["template_error"]["text"]))
        return problems
    if "run_error" in case:
        if actual.get("run_error") != case["run_error"]:
            problems.append("expected %r, got %r" % (case["run_error"], actual))
        return problems
    if "rows" not in actual:
        return ["expected rows, got %r" % actual]
    if "header" in case and case["header"] != actual["header"]:
        problems.append("header %r, case says %r" % (actual["header"], case["header"]))
    if actual["rows"] != case["rows"]:
        problems.append("rows %r, case says %r" % (actual["rows"], case["rows"]))
    return problems


def main():
    failures = total = 0
    for path in sys.argv[1:]:
        with open(path, encoding="utf-8") as handle:
            for case in yaml.safe_load(handle):
                total += 1
                for problem in check(case):
                    failures += 1
                    print("%s: %s: %s" % (path, case["name"], problem))
    print("%d cases checked against textfsm %s, %d problems" % (total, textfsm.__version__, failures))
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
