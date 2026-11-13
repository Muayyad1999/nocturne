#!/usr/bin/env python3
"""Build data/oui.tsv.gz, the vendor registry embedded in nocturne-tools.

Source: the IEEE Registration Authority's public registries
(https://standards-oui.ieee.org):

    MA-L  oui/oui.csv        24-bit prefixes (the classic OUI)
    MA-M  oui28/mam.csv      28-bit prefixes
    MA-S  oui36/oui36.csv    36-bit prefixes
    IAB   iab/iab.csv        36-bit prefixes (legacy individual address blocks)

Usage:

    python3 build_oui.py                 # download today's registries
    python3 build_oui.py --source DIR    # use oui.csv, mam.csv, oui36.csv, iab.csv from DIR
    python3 build_oui.py --source DIR --date 2026-11-13 --report

Output format (gzip, UTF-8, one line per assigned prefix, sorted by prefix):

    #date<TAB>YYYY-MM-DD
    PREFIXHEX<TAB>Organisation

PREFIXHEX is 6, 7 or 9 upper-case hex digits. A prefix the registry lists more
than once keeps every organisation, joined with " / ". Organisation names are
whitespace-normalised and lose trailing legal-form noise ("Cisco Systems, Inc"
becomes "Cisco Systems") when that noise is unmistakable, see `trim_legal`.
The output is byte-for-byte reproducible for the same input and date.
"""

from __future__ import annotations

import argparse
import csv
import datetime
import gzip
import html
import http.client
import io
import pathlib
import re
import sys
import time
import urllib.request

REGISTRIES = {
    "oui.csv": ("https://standards-oui.ieee.org/oui/oui.csv", 6),
    "mam.csv": ("https://standards-oui.ieee.org/oui28/mam.csv", 7),
    "oui36.csv": ("https://standards-oui.ieee.org/oui36/oui36.csv", 9),
    "iab.csv": ("https://standards-oui.ieee.org/iab/iab.csv", 9),
}

# Legal forms that are recognised in any letter case.
LEGAL_ANY_CASE = {
    "inc", "incorporated", "ltd", "limited", "llc", "l.l.c", "llp", "gmbh", "mbh", "corp",
    "corporation", "plc", "ltda", "bhd", "kgaa", "oyj", "s.a.s", "s.p.a", "s.r.l", "s.r.o",
    "s.a", "s.l", "s.l.u", "b.v", "n.v", "a/s", "a.s", "k.k", "d.o.o",
}
# Short forms that could be an ordinary word or abbreviation in another
# spelling, so they only count in exactly this one.
LEGAL_EXACT = {"AG", "AB", "AS", "ASA", "SA", "SL", "NV", "BV", "KG", "KK", "OY", "Oy", "ApS", "SAS", "SRL", "SpA", "Kft", "JSC"}
# Words that belong to a legal form only when one follows ("Co., Ltd", "Pte. Ltd", "Sdn. Bhd").
LEGAL_PREFIX = {"co", "pvt", "private", "pte", "pty", "sdn"}

# Words, with `&` and `+` as tokens of their own ("GmbH&Co.KG", "GmbH + Co. KG").
TOKEN = re.compile(r"[^\s,&+]+|[&+]")
# "Co.Ltd", "CO.,LTD", "Corp.Ltd", "Co.KG" written as one word.
GLUED = re.compile(r"(?i)^(co|corp)\.?(ltd|limited|kg)\.?$")
# "(SHENZHEN)CO." or "Tech.Co." - a "Co." stuck to the word before it. The
# word must be long enough not to be an abbreviation like "Ni.Co.".
STUCK_CO = re.compile(r"(?i)^(.*\)|.{3,}\.)(co\.?)$")


def normalise(name: str) -> str:
    """Undo the registry's HTML escaping and collapse runs of (Unicode) whitespace."""
    return " ".join(html.unescape(name).split())


WRAPPED = re.compile(r"^\((.*)\)$")


def core(token: str) -> str:
    """The word without its full stops and, for "(Pty)", its brackets."""
    word = token.strip(".")
    wrapped = WRAPPED.match(word)
    return wrapped.group(1).strip(".") if wrapped else word


def is_legal(token: str) -> bool:
    return core(token).lower() in LEGAL_ANY_CASE or core(token) in LEGAL_EXACT


def is_legal_prefix(token: str) -> bool:
    return core(token).lower() in LEGAL_PREFIX


def is_noise(token: str) -> bool:
    """Stray punctuation between legal words, as in "Co ., Ltd" or "Co,. Ltd"."""
    return all(char in ".;:-" for char in token)


def tokenize(name: str) -> list[tuple[str, int, int]]:
    tokens = []
    for match in TOKEN.finditer(name):
        text, start = match.group(), match.start()
        glued = GLUED.match(text)
        stuck = STUCK_CO.match(text)
        if glued:
            tokens.append((glued.group(1), start, start + glued.end(1)))
            tokens.append((glued.group(2), start + glued.end(1), match.end()))
        elif stuck:
            # The joining dot of "Tech.Co." goes with the "Co.".
            head = stuck.group(1).removesuffix(".")
            tokens.append((head, start, start + len(head)))
            tokens.append((stuck.group(2), start + stuck.end(1), match.end()))
        else:
            tokens.append((text, start, match.end()))
    return tokens


def trim_legal(name: str) -> str:
    """Drop trailing legal-form words; return the name unchanged when in doubt."""
    tokens = tokenize(name)
    keep = len(tokens)

    def skip_noise(position: int) -> int:
        while position > 0 and is_noise(tokens[position - 1][0]):
            position -= 1
        return position

    stripped_legal = False
    while True:
        position = skip_noise(keep)
        if position > 0 and is_legal(tokens[position - 1][0]):
            keep = position - 1
            stripped_legal = True
            continue
        # "Co., Ltd", "Pte. Ltd", "GmbH & Co. KG": words that lead up to a legal form.
        if stripped_legal and position > 0 and is_legal_prefix(tokens[position - 1][0]):
            keep = position - 1
            position = skip_noise(keep)
            if position > 1 and tokens[position - 1][0] in "&+" and is_legal(tokens[position - 2][0]):
                keep = position - 1
            continue
        break

    if keep == len(tokens) or keep == 0:
        return name
    # Something like "GmbH & Co. Betriebs KG": the legal form is interleaved
    # with other words and cannot be cut off cleanly.
    if any(is_legal(text) or is_legal_prefix(text) for text, _, _ in tokens[max(keep - 2, 0) : keep]):
        return name
    end = tokens[keep - 1][2]
    trimmed = name[:end].rstrip(" ,;")
    # "BIGHOUSE.,INC.": a full stop that only introduced the legal form.
    if trimmed.endswith(".") and name[end:].lstrip().startswith(","):
        trimmed = trimmed[:-1].rstrip(" ,;.")
    # A dangling connective or an opened bracket means the legal form was
    # part of the name proper.
    if len(trimmed) < 2 or trimmed.split()[-1].lower() in {"&", "+", "and", "und", "et", "of", "the", "-"}:
        return name
    if trimmed.count("(") != trimmed.count(")") and name.count("(") == name.count(")"):
        return name
    return trimmed


def download(url: str, attempts: int = 4) -> str:
    """Fetch one registry; the IEEE server drops a connection now and then."""
    for attempt in range(1, attempts + 1):
        try:
            request = urllib.request.Request(url, headers={"User-Agent": "oui-registry-fetch"})
            with urllib.request.urlopen(request, timeout=120) as response:
                return response.read().decode("utf-8")
        except (OSError, http.client.HTTPException) as error:
            print(f"{url}: {error} (attempt {attempt} of {attempts})", file=sys.stderr)
            time.sleep(2 * attempt)
    raise SystemExit(f"cannot download {url}")


def read_registry(text: str, digits: int) -> list[tuple[str, str]]:
    rows = []
    reader = csv.reader(io.StringIO(text))
    header = next(reader)
    if header[:3] != ["Registry", "Assignment", "Organization Name"]:
        raise SystemExit(f"unexpected CSV header: {header}")
    for row in reader:
        prefix = row[1].strip().upper()
        if len(prefix) != digits or not re.fullmatch(r"[0-9A-F]+", prefix):
            raise SystemExit(f"unexpected assignment {row[1]!r} (expected {digits} hex digits)")
        name = normalise(row[2])
        if not name:
            raise SystemExit(f"assignment {prefix} has no organisation name")
        rows.append((prefix, name))
    return rows


def verify(path: pathlib.Path, sources: dict[str, list[tuple[str, str]]]) -> None:
    """Read the file back and check it against the registries it was built from."""
    with gzip.open(path, "rt", encoding="utf-8") as handle:
        lines = handle.read().split("\n")
    if not lines[0].startswith("#date\t") or lines[-1] != "":
        raise SystemExit("verification failed: header or final newline is missing")
    entries = dict(line.split("\t") for line in lines[1:-1])
    prefixes = [line.split("\t")[0] for line in lines[1:-1]]
    if prefixes != sorted(set(prefixes)):
        raise SystemExit("verification failed: prefixes are not sorted and unique")
    listed = 0
    for rows in sources.values():
        for prefix, name in rows:
            listed += 1
            stored = entries.get(prefix)
            if stored is None:
                raise SystemExit(f"verification failed: {prefix} ({name}) is missing")
            # Trimming only ever removes the end of a name (and at most one
            # full stop in front of the legal form).
            if not any(name.startswith(part.rstrip(".")) for part in stored.split(" / ")):
                raise SystemExit(f"verification failed: {prefix} is {stored!r}, the registry says {name!r}")
    if set(entries) != {prefix for rows in sources.values() for prefix, _ in rows}:
        raise SystemExit("verification failed: the file has prefixes the registries do not")
    print(f"verified {len(entries)} prefixes against {listed} registry rows", file=sys.stderr)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--source", type=pathlib.Path, help="directory holding the four CSV files (default: download)")
    parser.add_argument("--date", help="registry date to record, YYYY-MM-DD (default: today, UTC)")
    parser.add_argument("--out", type=pathlib.Path, default=pathlib.Path(__file__).with_name("oui.tsv.gz"))
    parser.add_argument("--report", action="store_true", help="print every distinct name change to stdout")
    args = parser.parse_args()

    date = args.date or datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%d")
    datetime.date.fromisoformat(date)

    assignments: dict[str, list[str]] = {}
    changes: dict[str, str] = {}
    sources: dict[str, list[tuple[str, str]]] = {}
    for filename, (url, digits) in REGISTRIES.items():
        if args.source:
            text = (args.source / filename).read_text(encoding="utf-8")
        else:
            text = download(url)
        rows = read_registry(text, digits)
        sources[filename] = rows
        print(f"{filename}: {len(rows)} assignments", file=sys.stderr)
        for prefix, name in rows:
            trimmed = trim_legal(name)
            if trimmed != name:
                changes[name] = trimmed
            names = assignments.setdefault(prefix, [])
            if trimmed not in names:
                names.append(trimmed)

    lines = [f"#date\t{date}"]
    for prefix in sorted(assignments):
        lines.append(f"{prefix}\t{' / '.join(sorted(assignments[prefix]))}")
    payload = ("\n".join(lines) + "\n").encode("utf-8")

    buffer = io.BytesIO()
    with gzip.GzipFile(fileobj=buffer, mode="wb", compresslevel=9, mtime=0, filename="") as archive:
        archive.write(payload)
    args.out.write_bytes(buffer.getvalue())
    print(
        f"{args.out}: {len(assignments)} prefixes, {len(payload)} bytes, {len(buffer.getvalue())} compressed; "
        f"{len(changes)} distinct names trimmed",
        file=sys.stderr,
    )
    verify(args.out, sources)
    if args.report:
        for name in sorted(changes):
            print(f"{name}\t->\t{changes[name]}")


if __name__ == "__main__":
    main()
