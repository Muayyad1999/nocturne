#!/usr/bin/env python3
"""Engine-semantics test cases and the Python reference.

    pip install textfsm pyyaml

    python3 tools/semantics.py check tests/semantics/*.yml
    python3 tools/semantics.py fuzz <count> <seed> <out.json>

Every case states what the reference implementation (google/textfsm) does for
a template and an input; `tests/semantics.rs` holds the Rust engine to the
same expectations.

`check` proves the expectations of the hand-written cases, so run it whenever
a case is added or changed. `fuzz` writes random cases with the outcome the
reference produces; run them with

    TEXTFSM_CASES=<out.json> cargo test -p nocturne-textfsm --test semantics

Rows are compared the way ntc-templates hands them out: every cell passed
through `str()`, list items individually (as `textfsm.clitable` does).
"""

import io
import json
import random
import re
import sys

import textfsm

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


def problems(case):
    """How the reference disagrees with what the case states."""
    actual = run(case)
    if "template_error" in case:
        if "template_error" not in actual:
            return ["expected a template error, got %r" % actual]
        expected_line = case["template_error"].get("line")
        actual_line = actual["template_error"]["line"]
        # The reference does not report a line for every error.
        if expected_line is not None and actual_line is not None and expected_line != actual_line:
            return ["template error on line %s, case says %s (%s)" % (
                actual_line, expected_line, actual["template_error"]["text"])]
        return []
    if "run_error" in case:
        if actual.get("run_error") != case["run_error"]:
            return ["expected %r, got %r" % (case["run_error"], actual)]
        return []
    if "rows" not in actual:
        return ["expected rows, got %r" % actual]
    found = []
    if "header" in case and case["header"] != actual["header"]:
        found.append("header %r, case says %r" % (actual["header"], case["header"]))
    if actual["rows"] != case["rows"]:
        found.append("rows %r, case says %r" % (actual["rows"], case["rows"]))
    return found


def check(paths):
    import yaml

    failures = total = 0
    for path in paths:
        with open(path, encoding="utf-8") as handle:
            for case in yaml.safe_load(handle):
                total += 1
                for problem in problems(case):
                    failures += 1
                    print("%s: %s: %s" % (path, case["name"], problem))
    print("%d cases checked against textfsm %s, %d problems" % (total, textfsm.__version__, failures))
    return 1 if failures else 0


# --- random cases --------------------------------------------------------------

# Value regexes with texts they match.
VALUE_REGEXES = {
    r"(\d+)": ["1", "22", "007"],
    r"(\w+)": ["a", "ab", "a1", "x_2"],
    r"(\S*)": ["", "a", "-", "a-1"],
    r"(\S+)": ["a", "-", "a-1", "x"],
    r"([a-c]*)": ["", "a", "abc", "cb"],
    r"(x|y|)": ["x", "y", ""],
    r"(\d*)": ["", "1", "22"],
}
# Regex fragments with texts they match.
FRAGMENTS = {
    "a": ["a"], "b": ["b"], "c": ["c"], "x": ["x"], "-": ["-"], " ": [" "], "": [""],
    r"\s*": ["", " ", "  "], r"\s+": [" ", "  "], r"\w+": ["a", "ab", "1"], r"\d+": ["1", "22"],
    r"\S+": ["a", "-", "a1"], ".": ["a", "-", " ", "1"], r"\S*": ["", "ab"], "(?:a|b)": ["a", "b"],
    "[xy]?": ["", "x", "y"],
}
OPTIONS = ["Required", "Filldown", "Fillup", "List", "Key"]
WORDS = ["a", "b", "c", "x", "y", "1", "22", "ab", "-", "a1", ""]
OPERATORS = [
    "", "", " -> Record", " -> Record", " -> Next", " -> Next.Record", " -> NoRecord", " -> Continue",
    " -> Continue.Record", " -> Continue.Clear", " -> Continue.Clearall", " -> Clear", " -> Clearall",
    " -> Next.Clear", " -> Next.Clearall", " -> Next.NoRecord",
]


def random_case(rng, name):
    """A random template and an input made mostly of lines its rules match."""
    values = []
    for index in range(rng.randint(1, 4)):
        options = rng.sample(OPTIONS, rng.choice([0, 0, 0, 1, 1, 2, 3]))
        values.append(("V%d" % index, options, rng.choice(list(VALUE_REGEXES))))
    states = ["Start"] + rng.sample(["First", "Second"], rng.randint(0, 2))
    lines = []
    for value_name, options, regex in values:
        lines.append("Value %s%s %s" % (",".join(options) + " " if options else "", value_name, regex))
    samplers = []
    for state in states:
        lines += ["", state]
        for _ in range(rng.randint(0, 5)):
            rule, sampler = random_rule(rng, values, states)
            lines.append("  " + rule)
            samplers.append(sampler)
    if rng.random() < 0.15:
        lines += ["", "EOF"]
    if rng.random() < 0.05:
        lines += ["", "End"]

    input_lines = []
    for _ in range(rng.randint(0, 14)):
        if samplers and rng.random() < 0.75:
            input_lines.append(rng.choice(samplers)())
        else:
            input_lines.append(rng.choice(["", " ", "-"]).join(rng.choice(WORDS) for _ in range(rng.randint(0, 4))))
    ending = rng.choice(["\n", "\n", "\n", "\r\n", ""])
    text = ending.join(input_lines) + (ending if rng.random() < 0.8 else "")
    return {"name": name, "template": "\n".join(lines) + "\n", "input": text}


def random_rule(rng, values, states):
    """A rule and a function that makes up a line the rule's regex matches."""
    parts = []  # (regex text, texts it matches)
    unused = list(values)
    rng.shuffle(unused)
    for position in range(rng.randint(0, 4)):
        if position:
            separator = rng.choice([" ", r"\s*", r"\s+", "", "-"])
            parts.append((separator, FRAGMENTS[separator]))
        roll = rng.random()
        if roll < 0.5 and unused:
            value_name, _, regex = unused.pop()
            reference = "${%s}" % value_name
            if rng.random() < 0.25:
                parts.append(("(?:%s)?" % reference, VALUE_REGEXES[regex] + [""]))
            else:
                parts.append((reference, VALUE_REGEXES[regex]))
        else:
            fragment = rng.choice([f for f in FRAGMENTS if f.strip()])
            parts.append((fragment, FRAGMENTS[fragment]))
    anchored = rng.random() < 0.3
    if rng.random() < 0.06:
        action = rng.choice([" -> Error", ' -> Error "bad line"', " -> Error Oops", " -> Error.Record"])
    else:
        action = rng.choice(OPERATORS)
        if "Continue" not in action and rng.random() < 0.25:
            action = (action or " ->") + " " + rng.choice(states + ["End", "EOF"])
    rule = "^" + "".join(regex for regex, _ in parts) + ("$$" if anchored else "") + action

    def sample():
        line = "".join(rng.choice(texts) for _, texts in parts)
        return line if anchored or rng.random() < 0.7 else line + rng.choice([" tail", "x", " "])

    return rule, sample


def damage(rng, template):
    """Changes a template slightly, usually into something invalid."""
    roll = rng.random()
    if roll < 0.6:
        position = rng.randrange(len(template))
        junk = rng.choice([" ", "  ", "\t", "$", "{", "}", "(", ")", "-", ">", ".", "^", "#", "\n", '"', "E", "1", "\\"])
        return template[:position] + (junk if roll < 0.35 else "") + template[position + (roll >= 0.2):]
    lines = template.split("\n")
    first, second = rng.randrange(len(lines)), rng.randrange(len(lines))
    if roll < 0.8:
        lines[first], lines[second] = lines[second], lines[first]
    else:
        lines.insert(first, lines[second])
    return "\n".join(lines)


def fuzz(count, seed, out_path):
    rng = random.Random(seed)
    cases = []
    outcomes = {"rows": 0, "run_error": 0, "template_error": 0}
    for index in range(count):
        case = random_case(rng, "random %d" % index)
        if rng.random() < 0.3:
            case["template"] = damage(rng, case["template"])
        try:
            outcome = run(case)
        except Exception:  # pylint: disable=broad-except
            # The reference itself crashes, e.g. on a backslash in a Value name.
            continue
        if "template_error" in outcome:
            case["template_error"] = {"line": outcome["template_error"]["line"]}
            outcomes["template_error"] += 1
        elif "run_error" in outcome:
            case["run_error"] = outcome["run_error"]
            outcomes["run_error"] += 1
        else:
            case["header"], case["rows"] = outcome["header"], outcome["rows"]
            outcomes["rows"] += 1
        cases.append(case)
    with open(out_path, "w", encoding="utf-8") as handle:
        json.dump(cases, handle, ensure_ascii=False, indent=0)
    print("%d cases written: %r" % (len(cases), outcomes))
    return 0


def main(arguments):
    if len(arguments) >= 2 and arguments[0] == "check":
        return check(arguments[1:])
    if len(arguments) == 4 and arguments[0] == "fuzz":
        return fuzz(int(arguments[1]), int(arguments[2]), arguments[3])
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
