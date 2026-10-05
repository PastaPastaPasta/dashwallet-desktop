#!/usr/bin/env python3
"""Rewrites JSON files in the layout used under testdata/: top-level keys on
their own lines and every element of a top-level array on one line, so
diffs of regenerated vectors stay readable and the files stay small.

Usage: jsonfmt.py FILE...
"""
import json
import sys


def dump(value):
    return json.dumps(value, ensure_ascii=False, separators=(", ", ": "))


def format_doc(doc):
    lines = ["{"]
    items = list(doc.items())
    for i, (key, value) in enumerate(items):
        comma = "," if i + 1 < len(items) else ""
        if isinstance(value, list) and value:
            lines.append(f" {dump(key)}: [")
            for j, element in enumerate(value):
                lines.append("  " + dump(element) + ("," if j + 1 < len(value) else ""))
            lines.append(" ]" + comma)
        elif isinstance(value, dict) and value and all(isinstance(v, list) for v in value.values()):
            lines.append(f" {dump(key)}: " + format_doc(value).replace("\n", "\n ") + comma)
        else:
            lines.append(f" {dump(key)}: {dump(value)}{comma}")
    lines.append("}")
    return "\n".join(lines)


for path in sys.argv[1:]:
    with open(path, encoding="utf-8") as f:
        doc = json.load(f)
    with open(path, "w", encoding="utf-8") as f:
        f.write(format_doc(doc) + "\n")
