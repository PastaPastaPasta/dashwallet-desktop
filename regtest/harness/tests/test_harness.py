"""Failure handling of the harness itself."""

import stat
import time
import urllib.error

import pytest

from dwd_regtest import LocalBinaryNode, RPCClient


def test_local_backend_reports_dashd_exit_quickly(tmp_path):
    """A dashd that dies at startup must fail start() at once with its log, not after the RPC timeout."""
    bin_dir = tmp_path / "dashcore" / "bin"
    bin_dir.mkdir(parents=True)
    fake = bin_dir / "dashd"
    fake.write_text("#!/bin/sh\necho 'Error: simulated startup failure'\nexit 3\n")
    fake.chmod(fake.stat().st_mode | stat.S_IXUSR)

    node = LocalBinaryNode(dashcore_dir=tmp_path / "dashcore")
    started = time.monotonic()
    with pytest.raises(RuntimeError) as excinfo:
        node.start()
    assert time.monotonic() - started < 10
    message = str(excinfo.value)
    assert "exited with status 3" in message
    assert "simulated startup failure" in message


def test_wrong_rpc_credentials_raise_http_401(regtest_node):
    """wait_for_rpc treats 401 as fatal; dashd must answer bad credentials with HTTP 401."""
    client = RPCClient(regtest_node.rpc_url, "dwd", "not-the-password")
    with pytest.raises(urllib.error.HTTPError) as excinfo:
        client.getblockchaininfo()
    assert excinfo.value.code == 401
