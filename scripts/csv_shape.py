#!/usr/bin/env python3
"""Describe a CSV's *shape* without disclosing any of its contents.

Written so a real bank, PayPal or marketplace export can be characterised
well enough to build a parser against, while the file itself stays on the
machine it was downloaded to.

**What it prints:** the delimiter, the encoding it decoded as, the header
names verbatim, the row count, the distinct field counts (exports are often
ragged), and per column a *classification* of each cell — empty, an integer,
a decimal in one of several notations, a date, a datetime, an IBAN-shaped
token, an email-shaped token, or free text with a length range.

**What it never prints:** a cell value, or any substring of one. Column
*names* are a format description, not data, so those are shown as they are;
everything else is reduced to a category and a count. That is the whole
point: the output is safe to paste into a chat or an issue, and a sample row
is not.

Usage:
    python3 scripts/csv_shape.py local-data/paypal-export.csv
    just csv-shape local-data/paypal-export.csv

Verified by hand against the shapes a real export actually arrives in (this
repo has no Python test runner, so this list is the record):

* the C24 fixtures, where the answer is independently known from a real
  export — it recovers the 14-column header, the ragged 13/14 field counts,
  `dd.MM.yyyy` dates, `dd.MM.yyyy HH:mm` card timestamps, money-with-symbol
  amounts, and that IBAN/BIC/Verwendungszweck are populated only on non-card
  rows;
* UTF-8 with a BOM (what C24 sends) and **cp1252** (what a German export
  often sends) — umlauts in header names decode correctly in both;
* **CRLF** line endings, which do not leak a stray `\r` into the last column;
* a **semicolon** delimiter, which German tooling emits as readily as a comma;
* quoted fields containing an embedded delimiter *or* an embedded newline —
  neither splits a row, so the row count stays right;
* German grouped decimals (`1.234,56`) distinguished from plain comma
  decimals (`-9,99`).
"""

from __future__ import annotations

import csv
import io
import re
import sys
from collections import Counter

# Tried in order. German exports are routinely cp1252 and routinely have a
# UTF-8 BOM; both must decode before the header can even be read.
ENCODINGS = ("utf-8-sig", "utf-8", "cp1252", "latin-1")

PATTERNS: tuple[tuple[str, re.Pattern[str]], ...] = (
    ("integer", re.compile(r"^-?\d+$")),
    # 1.234,56 — German grouped decimal
    ("decimal:de-grouped", re.compile(r"^-?\d{1,3}(?:\.\d{3})+,\d+$")),
    # 1,234.56 — English grouped decimal
    ("decimal:en-grouped", re.compile(r"^-?\d{1,3}(?:,\d{3})+\.\d+$")),
    ("decimal:comma", re.compile(r"^-?\d+,\d+$")),
    ("decimal:dot", re.compile(r"^-?\d+\.\d+$")),
    ("date:dd.mm.yyyy", re.compile(r"^\d{2}\.\d{2}\.\d{4}$")),
    ("date:yyyy-mm-dd", re.compile(r"^\d{4}-\d{2}-\d{2}$")),
    ("date:dd/mm/yyyy", re.compile(r"^\d{2}/\d{2}/\d{4}$")),
    ("time:hh:mm", re.compile(r"^\d{1,2}:\d{2}(?::\d{2})?$")),
    ("datetime:dd.mm.yyyy hh:mm", re.compile(r"^\d{2}\.\d{2}\.\d{4} \d{1,2}:\d{2}")),
    ("datetime:iso", re.compile(r"^\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}")),
    # Shape only: two letters, two digits, then alphanumerics.
    ("iban-shaped", re.compile(r"^[A-Z]{2}\d{2}[A-Z0-9 ]{10,32}$")),
    ("email-shaped", re.compile(r"^[^@\s]+@[^@\s]+\.[A-Za-z]{2,}$")),
    ("currency-code", re.compile(r"^[A-Z]{3}$")),
    # A money cell carrying its symbol, e.g. "-1,00 €" or "$12.34".
    ("money-with-symbol", re.compile(r"^[-+]?\s*[€$£]?\s*[\d.,]+\s*[€$£]?$")),
)


def classify(cell: str) -> str:
    stripped = cell.strip()
    if stripped == "":
        return "empty"
    for name, pattern in PATTERNS:
        if pattern.match(stripped):
            return name
    return "text"


def decode(path: str) -> tuple[str, str]:
    with open(path, "rb") as handle:
        raw = handle.read()
    for encoding in ENCODINGS:
        try:
            return raw.decode(encoding), encoding
        except UnicodeDecodeError:
            continue
    # Last resort: never fail on an undecodable byte, just say so.
    return raw.decode("utf-8", errors="replace"), "utf-8 (with replacements)"


def sniff_delimiter(text: str) -> str:
    sample = text[:8192]
    try:
        return csv.Sniffer().sniff(sample, delimiters=",;\t|").delimiter
    except csv.Error:
        # Sniffer gives up on single-column or unusual files; count instead.
        first = sample.splitlines()[0] if sample.splitlines() else ""
        counts = {d: first.count(d) for d in ",;\t|"}
        best = max(counts, key=lambda d: counts[d])
        return best if counts[best] else ","


def describe(path: str) -> int:
    text, encoding = decode(path)
    delimiter = sniff_delimiter(text)
    rows = list(csv.reader(io.StringIO(text), delimiter=delimiter))
    if not rows:
        print(f"{path}: empty file")
        return 1

    header, data = rows[0], rows[1:]
    shown = delimiter.replace("\t", "\\t")
    print(f"file:      {path}")
    print(f"encoding:  {encoding}")
    print(f"delimiter: {shown!r}")
    print(f"columns:   {len(header)}")
    print(f"data rows: {len(data)}")

    widths = Counter(len(row) for row in data if any(cell.strip() for cell in row))
    if len(widths) > 1:
        print("\nRAGGED: data rows do not all have the same field count.")
        for width, count in sorted(widths.items()):
            print(f"  {width} fields: {count} rows")
    elif widths:
        only = next(iter(widths))
        if only != len(header):
            print(f"\nRAGGED: header has {len(header)} fields, every data row has {only}.")

    print("\nHeader, verbatim (column names are format, not data):")
    for index, name in enumerate(header):
        print(f"  [{index:2}] {name!r}")

    print("\nPer-column cell classification (counts only, no values):")
    for index, name in enumerate(header):
        kinds: Counter[str] = Counter()
        lengths: list[int] = []
        for row in data:
            cell = row[index] if index < len(row) else ""
            kind = classify(cell)
            kinds[kind] += 1
            if kind == "text":
                lengths.append(len(cell.strip()))
        summary = ", ".join(f"{kind}={count}" for kind, count in kinds.most_common())
        extra = ""
        if lengths:
            extra = f"  (text length {min(lengths)}-{max(lengths)})"
        print(f"  [{index:2}] {name!r}: {summary}{extra}")

    return 0


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        print(__doc__)
        print("error: expected exactly one CSV path", file=sys.stderr)
        return 2
    return describe(argv[1])


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
