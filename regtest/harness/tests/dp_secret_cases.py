"""Cases `test_dp_harness.py` runs in a pytest subprocess against a fake dwcli (not collected on
its own: the file name does not match `test_*.py`)."""

from __future__ import annotations


def test_funder_import_fails(funder):
    raise AssertionError("unreachable: the fake dwcli's import fails")


def test_created_wallet_then_failure(dp):
    root = dp.root("member")
    wallet = root.create("member")
    shown = f"{wallet!r} {wallet.mnemonic} {[wallet.mnemonic]}"
    raise AssertionError(f"a failure after a create: {shown}")


def test_harness_only(dp):
    assert dp.logs.is_dir()


def test_without_the_harness():
    assert True
