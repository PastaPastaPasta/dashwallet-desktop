#!/usr/bin/env python3
"""Funds review gate (docs/ci.md, "Funds review gate").

A pull request that changes a file listed in ci/funds-paths.txt passes only when its comments carry,
for its exact head commit, a funds-reviewer verdict and an Opus code review, each one an approve:

    <!-- dwd-review kind=funds verdict=approve sha=<40-hex head> model=claude-opus-5-5 -->
    <!-- dwd-review kind=review verdict=approve sha=<40-hex head> model=claude-opus-5-5 -->

Each record is a line of its own, outside quotes and code blocks, so quoting a record never casts it.
Only comments by the repository's owner, members and collaborators count, never a bot's. A push
moves the head, so every record for the old head stops counting. For one kind at the head, the
latest record wins, so a later `verdict=block` withdraws an approve.

    funds-gate.py match [--paths FILE]                      file names on stdin; prints the funds ones
    funds-gate.py check --repo R --pr N [--sha S] [--status] exit 0 pass, 1 refused, 2 error
    funds-gate.py record --repo R --pr N --sha S --kind funds|review --verdict approve|block
                         --model claude-opus-… [--note TEXT]   posts a record comment
"""

import argparse
import json
import os
import re
import subprocess
import sys
from pathlib import Path

PATHS_FILE = Path(__file__).resolve().parent.parent / "funds-paths.txt"
STATUS_CONTEXT = "Funds review gate"
KINDS = ("funds", "review")
LABELS = {"funds": "funds-reviewer verdict", "review": "Opus review"}
TRUSTED = {"OWNER", "MEMBER", "COLLABORATOR"}
OPUS = re.compile(r"^claude-opus-\d")
RECORD = re.compile(r"( {0,3})<!--[ \t]*dwd-review[ \t]+([^>]*?)[ \t]*-->[ \t]*")
FENCE = re.compile(r" {0,3}(`{3,}|~{3,})(.*)")
SHA = re.compile(r"^[0-9a-f]{40}$")


def glob_regex(glob):
    out, i = "", 0
    while i < len(glob):
        if glob.startswith("**", i):
            out, i = out + ".*", i + 2
        elif glob[i] == "*":
            out, i = out + "[^/]*", i + 1
        elif glob[i] == "?":
            out, i = out + "[^/]", i + 1
        else:
            out, i = out + re.escape(glob[i]), i + 1
    return re.compile(out + r"\Z")


def load_patterns(path=PATHS_FILE):
    rules = []
    for line in Path(path).read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        negate = line.startswith("!")
        rules.append((not negate, glob_regex(line[1:] if negate else line)))
    if not rules:
        raise SystemExit(f"{path}: no patterns")
    return rules


def is_funds(path, rules):
    hit = False
    for include, rx in rules:
        if rx.match(path):
            hit = include
    return hit


def record_lines(body):
    """(in_code, line) for each line; a fence follows CommonMark, an unclosed one runs to the end."""
    fence = None
    for line in body.replace("\r\n", "\n").replace("\r", "\n").split("\n"):
        m = FENCE.fullmatch(line)
        if fence:
            if m and m.group(1)[0] == fence[0] and len(m.group(1)) >= len(fence) and not m.group(2).strip():
                fence = None
            yield True, line
        elif m and not (m.group(1)[0] == "`" and "`" in m.group(2)):
            fence = m.group(1)
            yield True, line
        else:
            yield False, line


def parse_records(comments):
    """Records from trusted, non-bot comments, oldest first."""
    records = []
    ordered = sorted(comments, key=lambda c: (c.get("created_at") or "", c.get("id") or 0))
    for c in ordered:
        user = c.get("user") or {}
        if c.get("author_association") not in TRUSTED or user.get("type") == "Bot":
            continue
        for in_code, line in record_lines(c.get("body") or ""):
            m = RECORD.fullmatch(line)
            if not m:
                continue
            fields = dict(kv.split("=", 1) for kv in m.group(2).split() if "=" in kv)
            # A block counts wherever it stands, so a stray one can only refuse; an approve counts only at
            # the first column outside code.
            if fields.get("verdict") != "block" and (m.group(1) or in_code):
                continue
            fields["author"] = user.get("login", "?")
            fields["url"] = c.get("html_url", "")
            records.append(fields)
    return records


def model_problem(model):
    if not model:
        return "no model"
    if "sonnet" in model or "haiku" in model:
        return f"{model} is never a reviewer (Opus does code judgment)"
    if not OPUS.match(model):
        return f"{model} is not an Opus model"
    return None


def evaluate(head, files, comments, rules, complete=True):
    """Returns (passed, funds_files, lines)."""
    funds = sorted({f for f in files if is_funds(f, rules)})
    if not complete:
        funds = funds or ["(file list truncated by the API: treated as funds)"]
    if not funds:
        return True, [], ["No funds paths changed."]
    lines = [f"Funds paths changed ({len(funds)}):"] + [f"  {f}" for f in funds[:25]]
    if len(funds) > 25:
        lines.append(f"  … and {len(funds) - 25} more")
    latest = {}
    stale = 0
    for r in parse_records(comments):
        if r.get("kind") not in KINDS:
            continue
        if r.get("sha") != head:
            stale += 1
            continue
        latest[r["kind"]] = r
    ok = True
    for kind in KINDS:
        r = latest.get(kind)
        label = LABELS[kind]
        if r is None:
            lines.append(f"MISSING {label} (kind={kind}) for head {head}.")
            ok = False
            continue
        problem = model_problem(r.get("model", ""))
        if r.get("verdict") != "approve":
            lines.append(f"REFUSED {label}: verdict={r.get('verdict')} at {head[:12]} ({r['url']}).")
            ok = False
        elif problem:
            lines.append(f"REFUSED {label}: {problem} ({r['url']}).")
            ok = False
        else:
            lines.append(f"OK {label}: {r['model']} approve at {head[:12]} by {r['author']} ({r['url']}).")
    if stale:
        lines.append(f"{stale} record(s) name an older head and no longer count.")
    if not ok:
        lines.append(
            "Record one with: ci/github/funds-gate.py record --repo <owner/repo> --pr <N> --sha <head> "
            "--kind funds|review --verdict approve --model claude-opus-5-5 (docs/ci.md, \"Funds review gate\")."
        )
    return ok, funds, lines


def gh(*args, data=None):
    proc = subprocess.run(
        ["gh", "api", *args],
        input=json.dumps(data) if data is not None else None,
        capture_output=True,
        text=True,
    )
    if proc.returncode != 0:
        raise RuntimeError(f"gh api {' '.join(args)}: {proc.stderr.strip()}")
    return proc.stdout


def gh_pages(path):
    out = gh("--paginate", "--slurp", path)
    return [item for page in json.loads(out) for item in page]


class Moved(RuntimeError):
    pass


def fetch_pr(repo, pr):
    return json.loads(gh(f"repos/{repo}/pulls/{pr}"))


def fetch(repo, pr, info):
    files = []
    listed = gh_pages(f"repos/{repo}/pulls/{pr}/files?per_page=100")
    for f in listed:
        files.append(f["filename"])
        if f.get("previous_filename"):
            files.append(f["previous_filename"])
    complete = len(listed) >= info.get("changed_files", 0)
    comments = gh_pages(f"repos/{repo}/issues/{pr}/comments?per_page=100")
    after = fetch_pr(repo, pr)
    for side in ("head", "base"):
        if after[side]["sha"] != info[side]["sha"]:
            raise Moved(f"#{pr} {side} moved from {info[side]['sha']} to {after[side]['sha']} during the check; run it again")
    return files, complete, comments


def post_status(repo, sha, state, summary):
    run = os.environ.get("GITHUB_RUN_ID")
    data = {
        "state": state,
        "context": STATUS_CONTEXT,
        "description": summary[:140],
    }
    if run:
        data["target_url"] = f"{os.environ.get('GITHUB_SERVER_URL', 'https://github.com')}/{repo}/actions/runs/{run}"
    gh("-X", "POST", f"repos/{repo}/statuses/{sha}", "--input", "-", data=data)


def cmd_match(args):
    rules = load_patterns(args.paths)
    for line in sys.stdin:
        f = line.strip()
        if f and is_funds(f, rules):
            print(f)
    return 0


def policy_unchanged(repo, pr, paths):
    """A retry may only classify with the patterns it loaded if the new base carries the same list."""
    base = fetch_pr(repo, pr)["base"]["sha"]
    if gh("-H", "Accept: application/vnd.github.raw", f"repos/{repo}/contents/ci/funds-paths.txt?ref={base}") != Path(paths).read_text():
        raise RuntimeError(f"ci/funds-paths.txt changed at base {base} during the check; run it again")


def cmd_check(args):
    rules = load_patterns(args.paths)
    try:
        for attempt in range(3):
            info = fetch_pr(args.repo, args.pr)
            head = info["head"]["sha"]
            if args.sha and args.sha != head:
                print(f"REFUSED #{args.pr}: head is {head}, expected {args.sha}.")
                return 1
            # The status is keyed by commit, not pull request: only a pull request into the default branch
            # writes it, so a stacked pull request at the same commit cannot turn it green.
            post = args.status and info["base"]["ref"] == info["base"]["repo"]["default_branch"]
            if args.status and not post:
                print(f"#{args.pr} targets {info['base']['ref']}, not the default branch: no status posted.")
            if post:
                post_status(args.repo, head, "pending", "Checking funds paths and review records")
            try:
                files, complete, comments = fetch(args.repo, args.pr, info)
                break
            except Moved as e:
                if attempt == 2:
                    raise
                print(f"funds gate: {e}; reading it again", file=sys.stderr)
                policy_unchanged(args.repo, args.pr, args.paths)
    except (RuntimeError, json.JSONDecodeError, KeyError) as e:
        print(f"funds gate: {e}", file=sys.stderr)
        return 2
    passed, funds, lines = evaluate(head, files, comments, rules, complete)
    print(f"Funds review gate, #{args.pr} at {head}:")
    print("\n".join(lines))
    print("PASS" if passed else "REFUSED")
    if post:
        if not funds:
            summary = "No funds paths changed"
        elif passed:
            summary = f"Funds verdict and Opus review at {head[:12]}"
        else:
            summary = f"{len(funds)} funds path(s): needs funds verdict + Opus review at {head[:12]}"
        try:
            post_status(args.repo, head, "success" if passed else "failure", summary)
        except RuntimeError as e:
            print(f"funds gate: {e}", file=sys.stderr)
            return 2
    return 0 if passed else 1


def cmd_record(args):
    if not SHA.match(args.sha):
        raise SystemExit(f"--sha must be the full 40-hex head commit, not {args.sha!r}")
    problem = model_problem(args.model)
    if problem:
        raise SystemExit(f"refused: {problem}")
    head = json.loads(gh(f"repos/{args.repo}/pulls/{args.pr}"))["head"]["sha"]
    if head != args.sha:
        raise SystemExit(f"refused: #{args.pr} head is now {head}, not the reviewed {args.sha}; review the new head")
    title = "Funds review" if args.kind == "funds" else "Code review"
    note = Path(args.note_file).read_text().strip() if args.note_file else (args.note or "").strip()
    if "dwd-review" in note:
        raise SystemExit("refused: the note contains \"dwd-review\"; quote the verdict line without the record marker")
    body = [
        f"<!-- dwd-review kind={args.kind} verdict={args.verdict} sha={args.sha} model={args.model} -->",
        f"**{title}: {args.verdict}** at `{args.sha[:12]}` ({args.model})",
    ]
    if note:
        body += ["", note]
    body += [
        "",
        "---",
        f"🤖 Posted autonomously by Claude ({args.model}) on behalf of pasta.",
    ]
    out = gh("-X", "POST", f"repos/{args.repo}/issues/{args.pr}/comments", "--input", "-", data={"body": "\n".join(body)})
    print(json.loads(out)["html_url"])
    return 0


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="cmd", required=True)
    m = sub.add_parser("match")
    m.add_argument("--paths", default=PATHS_FILE)
    c = sub.add_parser("check")
    c.add_argument("--repo", required=True)
    c.add_argument("--pr", type=int, required=True)
    c.add_argument("--sha")
    c.add_argument("--status", action="store_true", help="post the result as a commit status on the head")
    c.add_argument("--paths", default=PATHS_FILE)
    r = sub.add_parser("record")
    r.add_argument("--repo", required=True)
    r.add_argument("--pr", type=int, required=True)
    r.add_argument("--sha", required=True)
    r.add_argument("--kind", choices=KINDS, required=True)
    r.add_argument("--verdict", choices=("approve", "block"), required=True)
    r.add_argument("--model", required=True)
    r.add_argument("--note")
    r.add_argument("--note-file")
    args = p.parse_args(argv)
    return {"match": cmd_match, "check": cmd_check, "record": cmd_record}[args.cmd](args)


if __name__ == "__main__":
    sys.exit(main())
