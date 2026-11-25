#!/usr/bin/env python3
"""Tests of ci/github/funds-gate.py and ci/funds-paths.txt. Run: python3 ci/github/funds-gate-test.py"""

import importlib.util
import subprocess
import sys
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("funds_gate", HERE / "funds-gate.py")
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)

RULES = gate.load_patterns()
HEAD = "a" * 40
OLD = "b" * 40
FUNDS_FILE = "rust/crates/dw-engine/src/platform/unfinished_locks.rs"


def comment(body, assoc="OWNER", kind="User", at="2026-11-25T05:00:00Z", cid=1):
    return {"id": cid, "body": body, "author_association": assoc, "created_at": at,
            "user": {"login": "someone", "type": kind}, "html_url": f"https://example.invalid/c{cid}"}


def record(kind, sha=HEAD, verdict="approve", model="claude-opus-5-5"):
    return f"<!-- dwd-review kind={kind} verdict={verdict} sha={sha} model={model} -->"


BOTH = record("funds") + "\n" + record("review")


class Paths(unittest.TestCase):
    def test_funds_paths(self):
        for f in [
            FUNDS_FILE,
            "rust/crates/dw-engine/src/send/plan.rs",
            "rust/crates/dw-engine/src/coins.rs",
            "rust/crates/dw-engine/src/restore_order_tests.rs",
            "rust/crates/dw-engine/src/platform/lease/lock.rs",
            "rust/crates/dw-engine/src/masternodes/withdrawal/sign.rs",
            "rust/crates/dw-vault/src/signer.rs",
            "rust/crates/dw-app/src/api/identity/shielded.rs",
            "rust/Cargo.lock",
            "apps/desktop/src/format/amount.ts",
            "ci/funds-paths.txt",
            "ci/github/funds-gate.py",
            ".github/workflows/funds-gate.yml",
            "rust/crates/dw-engine/src/governance/proposals.rs",
            "apps/desktop/src/dashpay/withdraw.ts",
            "apps/desktop/src/coinjoin/move.ts",
            "Sources/MacUI/PSBT/PSBTView.swift",
            "Sources/CrossUI/CoinSelectionScreen.swift",
            "Sources/WalletRuntime/Services/TransactionSender.swift",
            "Sources/WalletFeatures/CoinJoin/MixedCoinsViewModel.swift",
            "apps/desktop/src/masternodes/changeKeys.ts",
            "apps/desktop/src/screens/pay/model.ts",
            "apps/desktop/src/platform/mainnetOff.ts",
            "apps/desktop/src/tools/psbt.ts",
            "rust/crates/dw-app/src/api/social.rs",
            "rust/crates/dw-app/src/api/history.rs",
            "Sources/DashKit/EngineModels.swift",
            "rust/crates/dw-engine/src/history.rs",
            "rust/crates/dw-engine/src/a_file_added_later.rs",
            "rust/crates/dw-engine/Cargo.toml",
            "rust/crates/dw-engine/src/platform/contacts_model.rs",
            "rust/crates/dw-engine/src/platform/contact_payout.rs",
            "rust/bin/dwcli/src/pay.rs",
            "rust/crates/dw-app/capabilities/send.json",
            "rust/crates/dw-engine/src/governance/sync.rs",
            "rust/crates/dw-engine/src/governance/records.rs",
            "rust/crates/dw-engine/src/platform/contacts.rs",
            "rust/crates/dw-engine/src/platform/profiles.rs",
            "rust/crates/dw-app/src/dto/governance.rs",
            "rust/crates/dw-app/src/shell/links.rs",
            "ci/github/funds-gate-test.py",
            "apps/desktop/src/screens/pay/form.tsx",
            "apps/desktop/src/screens/tools/Psbt.tsx",
            "apps/desktop/src/masternodes/keys.ts",
            "apps/desktop/src/payments/hold.ts",
            "apps/desktop/src/screens/ShieldedSheet.tsx",
            "apps/desktop/src/contacts/actions.ts",
            "apps/desktop/src/dashpay/editProfile.ts",
            "apps/desktop/src/governance/model.ts",
            "apps/desktop/src/settings/files.ts",
            "rust/crates/dw-app/src/api/files.rs",
            "rust/crates/dw-app/src/shell/files.rs",
        ]:
            self.assertTrue(gate.is_funds(f, RULES), f)

    def test_not_funds_paths(self):
        for f in [
            "docs/ci.md",
            "rust/crates/dw-engine/src/platform/avatar_tests/fixtures.rs",
            "rust/crates/dw-engine/src/platform/profile.rs",
            "rust/crates/dw-engine/src/platform/contacts_tests/rows.rs",
            "rust/crates/dw-engine/src/rates/service.rs",
            "rust/crates/dw-engine/src/governance/voting.rs",
            "rust/crates/dw-engine/src/csv_export.rs",
            "rust/crates/dw-appdb/src/avatar.rs",
            "apps/desktop/src/screens/Overview.tsx",
            "apps/desktop/src/screens/masternodes/VoteSheet.tsx",
            "apps/desktop/src/format/amountish/x.ts",
        ]:
            self.assertFalse(gate.is_funds(f, RULES), f)

    def test_every_pattern_matches_a_tracked_file(self):
        tracked = subprocess.run(["git", "ls-files", "--cached", "--others", "--exclude-standard"], cwd=HERE.parent.parent, capture_output=True, text=True, check=True).stdout.split()
        for include, rx in RULES:
            if include:
                self.assertTrue(any(rx.match(f) for f in tracked), f"no tracked file matches {rx.pattern}")


class Gate(unittest.TestCase):
    def run_gate(self, comments, files=(FUNDS_FILE,), complete=True):
        return gate.evaluate(HEAD, list(files), comments, RULES, complete)

    def test_no_funds_paths_passes_without_records(self):
        ok, funds, _ = self.run_gate([], files=["docs/ci.md"])
        self.assertTrue(ok)
        self.assertEqual(funds, [])

    def test_funds_path_without_record_is_refused(self):
        ok, funds, lines = self.run_gate([])
        self.assertFalse(ok)
        self.assertEqual(funds, [FUNDS_FILE])
        self.assertIn("MISSING funds-reviewer verdict", "\n".join(lines))

    def test_both_records_at_head_pass(self):
        self.assertTrue(self.run_gate([comment(BOTH)])[0])

    def test_records_in_separate_comments_pass(self):
        self.assertTrue(self.run_gate([comment(record("funds"), cid=1), comment(record("review"), cid=2)])[0])

    def test_funds_verdict_alone_is_refused(self):
        self.assertFalse(self.run_gate([comment(record("funds"))])[0])

    def test_review_alone_is_refused(self):
        self.assertFalse(self.run_gate([comment(record("review"))])[0])

    def test_push_after_verdict_invalidates_it(self):
        ok, _, lines = self.run_gate([comment(record("funds", OLD) + record("review", OLD))])
        self.assertFalse(ok)
        self.assertIn("older head", "\n".join(lines))

    def test_short_sha_does_not_count(self):
        self.assertFalse(self.run_gate([comment(record("funds", HEAD[:12]) + record("review", HEAD[:12]))])[0])

    def test_sonnet_is_never_a_reviewer(self):
        ok, _, lines = self.run_gate([comment(record("funds") + record("review", model="claude-sonnet-5-5"))])
        self.assertFalse(ok)
        self.assertIn("never a reviewer", "\n".join(lines))

    def test_non_opus_model_is_refused(self):
        self.assertFalse(self.run_gate([comment(record("funds", model="gpt-6.2-astra") + record("review"))])[0])

    def test_later_block_withdraws_approve(self):
        later = comment(record("funds", verdict="block"), at="2026-11-25T06:00:00Z", cid=2)
        self.assertFalse(self.run_gate([later, comment(BOTH, cid=1)])[0])

    def test_later_approve_replaces_block(self):
        first = comment(record("funds", verdict="block") + record("review"), cid=1)
        self.assertTrue(self.run_gate([first, comment(record("funds"), at="2026-11-25T06:00:00Z", cid=2)])[0])

    def test_untrusted_and_bot_comments_do_not_count(self):
        self.assertFalse(self.run_gate([comment(BOTH, assoc="NONE")])[0])
        self.assertFalse(self.run_gate([comment(BOTH, assoc="CONTRIBUTOR")])[0])
        self.assertFalse(self.run_gate([comment(BOTH, kind="Bot")])[0])

    def test_truncated_file_list_is_treated_as_funds(self):
        self.assertFalse(self.run_gate([], files=["docs/ci.md"], complete=False)[0])


if __name__ == "__main__":
    unittest.main(verbosity=1)
