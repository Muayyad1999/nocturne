#!/usr/bin/env python3
"""Generate random regular expressions together with what CPython does with them.

    python3 tools/fuzz_regex.py <count> <seed> <out.json>
    PYREGEX_FUZZ=<out.json> cargo test --release -p nocturne-textfsm \\
        agrees_with_python_on_random_patterns -- --nocapture

Each case is {"pattern", "text"} plus either "error" (the message of re.error)
or "result": null, or [whole match, group 1, ...] of re.match(pattern, text).
The Rust test must reproduce every case, except for the loops over empty
matches described in src/pyregex/loops.rs.
"""

import json
import random
import re
import signal
import sys
import warnings

warnings.simplefilter("ignore")

LITERALS = list("abcxyz019 _-.,:/") + ["é", "ß", "Ω", "日", "²", " ", "K"]
ESCAPES = [r"\d", r"\D", r"\s", r"\S", r"\w", r"\W", r"\.", r"\-", r"\<", r"\>", r"\ ", r"\b", r"\B", r"\t", r"\x41", r"\/", r"\_"]
CLASS_ITEMS = ["a", "b", "c", "x", "0", "9", "_", "-", " ", ".", "é", "a-c", "0-9", "x-z", r"\d", r"\w", r"\s", r"\S", r"\W", r"\D", r"\-", r"\]", "[", "&", "~", "^", r"\\", "A-Z", "α-ω"]
TEXT_CHARS = list("abcxyzABC019 _-.,:/") + ["é", "É", "ß", "Ω", "ω", "日", "²", " ", "\t", "\x1f", "K", "k", "٣"]


def atom(rng, depth):
    roll = rng.random()
    if roll < 0.30:
        c = rng.choice(LITERALS)
        return re.escape(c) if rng.random() < 0.3 and c.isascii() else c
    if roll < 0.45:
        return rng.choice(ESCAPES)
    if roll < 0.52:
        return "."
    if roll < 0.68:
        items = "".join(rng.choice(CLASS_ITEMS) for _ in range(rng.randint(1, 4)))
        return "[" + ("^" if rng.random() < 0.25 else "") + items + "]"
    if roll < 0.72:
        return rng.choice(["^", "$"])
    if roll < 0.74:
        return rng.choice([r"\1", r"\2", "(?<=a)", "(?<!a)", "(?<=[ab].)", r"(?<=\w)", r"(?<!\s)", "(?(1)a|b)", "(?(2)x)", "(?#note)"])
    if depth >= 3:
        return rng.choice(LITERALS)
    inner = alternation(rng, depth + 1)
    roll = rng.random()
    if roll < 0.55:
        return "(" + inner + ")"
    if roll < 0.75:
        return "(?:" + inner + ")"
    if roll < 0.85:
        return "(?i:" + inner + ")"
    if roll < 0.90:
        return "(?=" + inner + ")"
    if roll < 0.95:
        return "(?!" + inner + ")"
    if roll < 0.97:
        return "(?>" + inner + ")"
    if roll < 0.98:
        return "(?P<n%d>" % rng.randint(1, 3) + inner + ")"
    return "(?s:" + inner + ")"


def piece(rng, depth):
    a = atom(rng, depth)
    roll = rng.random()
    if roll < 0.55:
        return a
    q = rng.choice(["*", "+", "?", "{2}", "{1,2}", "{0,2}", "{1,}", "{,2}"])
    roll = rng.random()
    suffix = "?" if roll < 0.2 else "+" if roll < 0.25 else ""
    return a + q + suffix


def sequence(rng, depth):
    return "".join(piece(rng, depth) for _ in range(rng.randint(0 if depth else 1, 4)))


def alternation(rng, depth):
    return "|".join(sequence(rng, depth) for _ in range(1 if rng.random() < 0.7 else rng.randint(2, 3)))


def text(rng):
    return "".join(rng.choice(TEXT_CHARS) for _ in range(rng.randint(0, 8)))


def on_alarm(signum, frame):
    raise TimeoutError


def main():
    signal.signal(signal.SIGALRM, on_alarm)
    count, seed, out = int(sys.argv[1]), int(sys.argv[2]), sys.argv[3]
    rng = random.Random(seed)
    cases = []
    while len(cases) < count:
        pattern = alternation(rng, 0)
        try:
            compiled = re.compile(pattern)
        except re.error as error:
            cases.append({"pattern": pattern, "text": "", "error": error.msg})
            continue
        except Exception:  # RecursionError, OverflowError
            continue
        for _ in range(4):
            subject = text(rng)
            # Python's engine has no step limit: skip patterns that blow up.
            signal.setitimer(signal.ITIMER_REAL, 0.5)
            try:
                m = compiled.match(subject)
            except TimeoutError:
                break
            finally:
                signal.setitimer(signal.ITIMER_REAL, 0)
            result = None if m is None else [m.group(0)] + list(m.groups())
            cases.append({"pattern": pattern, "text": subject, "result": result})
    with open(out, "w", encoding="utf-8") as fh:
        json.dump(cases, fh, ensure_ascii=False)
    print(len(cases), "cases,", sum(1 for c in cases if "error" in c), "errors,",
          sum(1 for c in cases if c.get("result") is not None), "matches")


if __name__ == "__main__":
    main()
