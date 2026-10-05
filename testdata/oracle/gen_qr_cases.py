#!/usr/bin/env python3
"""Writes testdata/qr_cases.json from libqrencode, the encoder dash-qt uses.

dash-qt calls QRcode_encodeString(uri, 0, QR_ECLEVEL_L, QR_MODE_8, 1); the
`qrencode` CLI with `-l L -8 -m 0` makes the same call. Needs `qrencode` on
PATH (Homebrew `qrencode`, 4.1.1 when these vectors were made).

    python3 testdata/oracle/gen_qr_cases.py > testdata/qr_cases.json
"""

import json
import subprocess

CASES = [
    "dash:XwnLY9Tf7Zsef8gMGL2fhWA9ZmMjt4KPwg",
    "dash:yQWsoTNJq59DqBg4Z2Qup3k3qchPaWz29n?amount=1.50000000&label=Caf%C3%A9&message=order%2042",
    "XwnLY9Tf7Zsef8gMGL2fhWA9ZmMjt4KPwg",
    "1234567890",
    "dash:XwnLY9Tf7Zsef8gMGL2fhWA9ZmMjt4KPwg?label=" + "%E2%82%AC" * 20,
    "dash:XwnLY9Tf7Zsef8gMGL2fhWA9ZmMjt4KPwg?message=" + "x" * 207,
] + [
    # Deterministic filler at sizes that cross version boundaries, so many
    # versions and all eight masks get exercised.
    "dash:" + "".join("XwnLY9Tf7Zsef8gMGL2fhWA9ZmMjt4KPwg"[(i * 7) % 34] for i in range(n))
    for n in (1, 12, 13, 28, 47, 48, 72, 101, 128, 160, 191, 220, 250)
]


def matrix(text: str) -> list[str]:
    out = subprocess.run(
        ["qrencode", "-l", "L", "-8", "-m", "0", "-t", "ASCII", text],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    # ASCII output draws each module as two characters: "##" dark, "  " light.
    return ["".join("1" if line[i] == "#" else "0" for i in range(0, len(line), 2))
            for line in out.splitlines() if line]


def main() -> None:
    version = subprocess.run(["qrencode", "--version"], capture_output=True, text=True)
    cases = []
    for text in CASES:
        assert len(text) <= 255, len(text)
        rows = matrix(text)
        cases.append({"text": text, "size": len(rows), "rows": rows})
    print(json.dumps({
        "source": (version.stdout or version.stderr).splitlines()[0],
        "call": "qrencode -l L -8 -m 0 -t ASCII <text>",
        "cases": cases,
    }, indent=1))


if __name__ == "__main__":
    main()
