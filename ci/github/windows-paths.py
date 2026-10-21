#!/usr/bin/env python3
"""Fails if a tracked path cannot be checked out on Windows.

Git for Windows refuses the whole checkout ("error: invalid path") when one path
component is a reserved device name (CON, PRN, AUX, NUL, COM1-9, LPT1-9, CONIN$,
CONOUT$, with or without an extension), ends in a dot or a space, or contains
<>:"|?*\\ or a control character. A fuzz seed named `nul` (b6b3da4b) broke every
Windows job that way; the Linux lint job runs this so the next one fails there.

    ci/github/windows-paths.py            # checks `git ls-files` of the work tree
"""

import re
import subprocess
import sys

RESERVED = re.compile(r"(con|prn|aux|nul|com[1-9]|lpt[1-9]|conin\$|conout\$)", re.IGNORECASE)
INVALID_CHARS = set('<>:"|?*\\') | {chr(c) for c in range(32)}


def problem(component: str) -> str | None:
    if component.endswith((".", " ")):
        return "ends in a dot or a space"
    bad = sorted(INVALID_CHARS.intersection(component))
    if bad:
        return f"contains {', '.join(repr(c) for c in bad)}"
    if RESERVED.fullmatch(component.split(".", 1)[0].rstrip(" ")):
        return "is a reserved device name"
    return None


def main() -> int:
    listed = subprocess.run(["git", "ls-files", "-z"], check=True, capture_output=True).stdout
    paths = [p for p in listed.decode("utf-8", "surrogateescape").split("\0") if p]
    failures = []
    for path in paths:
        for component in path.split("/"):
            reason = problem(component)
            if reason:
                failures.append(f"{path!r}: {component!r} {reason}")
                break
    for failure in failures:
        print(f"Windows-invalid path: {failure}", file=sys.stderr)
    if failures:
        return 1
    print(f"{len(paths)} tracked paths are valid on Windows")
    return 0


if __name__ == "__main__":
    sys.exit(main())
