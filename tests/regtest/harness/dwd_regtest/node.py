"""Start, stop and drive a single regtest dashd.

Two backends share one interface:

* `DockerComposeNode` runs the `dashd` service of tests/regtest/docker-compose.yml under a
  unique compose project name, with free loopback host ports for RPC and P2P.
* `LocalBinaryNode` runs `dashd` from an unpacked release (DASHCORE_DIR, as produced by
  scripts/fetch-dashcore.sh) in a temporary datadir.

Both start dashd with the same flags as scripts/node-entrypoint.sh (txindex, block filters,
RPC + P2P listening), so an SPV client can connect to `p2p_port`.
"""

from __future__ import annotations

import os
import shutil
import socket
import subprocess
import tempfile
import time
import urllib.error
from dataclasses import dataclass, field
from decimal import Decimal
from pathlib import Path
from typing import Callable, TypeVar

from .rpc import RPC_IN_WARMUP, RPCClient, RPCError

REGTEST_DIR = Path(__file__).resolve().parents[2]
COMPOSE_FILE = REGTEST_DIR / "docker-compose.yml"
RPC_USER = "dwd"
RPC_PASSWORD = "dwd"
# A coinbase output can be spent after 100 confirmations, so 101 blocks yield one mature block reward.
COINBASE_MATURITY_BLOCKS = 101

T = TypeVar("T")


class TimeoutExpired(AssertionError):
    """A `wait_until` condition did not become true in time."""


def wait_until(predicate: Callable[[], T], timeout: float, interval: float = 0.25, what: str = "condition") -> T:
    """Poll `predicate` until it returns a truthy value, and return that value."""
    deadline = time.monotonic() + timeout
    while True:
        value = predicate()
        if value:
            return value
        if time.monotonic() >= deadline:
            raise TimeoutExpired(f"timed out after {timeout}s waiting for {what}")
        time.sleep(interval)


def free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


@dataclass
class RegtestNode:
    """Common operations on a running regtest dashd. Subclasses implement start/stop."""

    rpc_port: int = field(default_factory=free_port)
    p2p_port: int = field(default_factory=free_port)
    host: str = "127.0.0.1"

    @property
    def rpc_url(self) -> str:
        return f"http://{self.host}:{self.rpc_port}"

    @property
    def rpc(self) -> RPCClient:
        return RPCClient(self.rpc_url, RPC_USER, RPC_PASSWORD)

    def wallet(self, name: str) -> RPCClient:
        return self.rpc.for_wallet(name)

    def start(self) -> None:
        raise NotImplementedError

    def stop(self) -> None:
        raise NotImplementedError

    def logs(self) -> str:
        raise NotImplementedError

    def check_alive(self) -> None:
        """Raise if the node process is known to have exited. Backends without a process handle do nothing."""

    def wait_for_rpc(self, timeout: float = 60.0) -> None:
        def ready() -> bool:
            self.check_alive()
            try:
                self.rpc.getblockchaininfo()
                return True
            except RPCError as err:
                if err.code == RPC_IN_WARMUP:
                    return False
                raise
            except urllib.error.HTTPError as err:
                # 401/403 mean wrong credentials or a rejected client address; retrying cannot help.
                if err.code in (401, 403):
                    raise
                return False
            except OSError:
                # Connection refused/reset while dashd (or the port forward) is still coming up.
                return False

        wait_until(ready, timeout, what=f"dashd RPC at {self.rpc_url}")

    # --- chain helpers -------------------------------------------------------------------

    def mine(self, blocks: int, address: str | None = None) -> list[str]:
        """Mine `blocks` blocks paying to `address` (a fresh `miner`-wallet address by default)."""
        if address is None:
            address = self.ensure_wallet("miner").getnewaddress()
        return self.rpc.generatetoaddress(blocks, address)

    def ensure_wallet(self, name: str) -> RPCClient:
        """Load or create a descriptor wallet called `name` and return its RPC client."""
        loaded = self.rpc.listwallets()
        if name not in loaded:
            existing = {entry["name"] for entry in self.rpc.listwalletdir()["wallets"]}
            if name in existing:
                self.rpc.loadwallet(name)
            else:
                self.rpc.createwallet(name)
        return self.wallet(name)

    def send_to_address(self, address: str, amount: Decimal, from_wallet: str = "miner") -> str:
        return self.wallet(from_wallet).sendtoaddress(address, amount)

    def wait_for_tx(self, txid: str, confirmations: int = 0, timeout: float = 30.0) -> dict:
        """Wait until `txid` is in the mempool (confirmations=0) or has >= `confirmations`.

        Uses getrawtransaction, which covers confirmed transactions because the node runs
        with -txindex.
        """

        def seen() -> dict | None:
            try:
                tx = self.rpc.getrawtransaction(txid, True)
            except RPCError as err:
                # -5: not in mempool / txindex yet
                if err.code == -5:
                    return None
                raise
            if tx.get("confirmations", 0) >= confirmations:
                return tx
            return None

        return wait_until(seen, timeout, what=f"tx {txid} with {confirmations} confirmation(s)")


@dataclass
class DockerComposeNode(RegtestNode):
    project: str = field(default_factory=lambda: f"dwd-regtest-{os.getpid()}-{int(time.time())}")
    build: bool = True

    def _compose(self, *args: str, check: bool = True, capture: bool = False) -> subprocess.CompletedProcess:
        env = dict(
            os.environ,
            DWD_COMPOSE_PROJECT=self.project,
            DWD_RPC_HOST_PORT=str(self.rpc_port),
            DWD_P2P_HOST_PORT=str(self.p2p_port),
            DWD_RPC_USER=RPC_USER,
            DWD_RPC_PASSWORD=RPC_PASSWORD,
        )
        return subprocess.run(
            ["docker", "compose", "-f", str(COMPOSE_FILE), "-p", self.project, *args],
            env=env,
            check=check,
            text=True,
            capture_output=capture,
        )

    def start(self) -> None:
        args = ["up", "-d", "--wait", "--wait-timeout", "120"]
        if self.build:
            args.append("--build")
        self._compose(*args, "dashd")
        self.wait_for_rpc()

    def stop(self) -> None:
        self._compose("down", "--volumes", "--timeout", "30", check=False)

    def logs(self) -> str:
        return self._compose("logs", "--no-color", "dashd", check=False, capture=True).stdout


@dataclass
class LocalBinaryNode(RegtestNode):
    dashcore_dir: Path = field(default_factory=lambda: Path(os.environ["DASHCORE_DIR"]))
    datadir: Path | None = None
    _process: subprocess.Popen | None = None
    _owns_datadir: bool = False

    def start(self) -> None:
        dashd = self.dashcore_dir / "bin" / "dashd"
        if not dashd.exists():
            raise FileNotFoundError(f"{dashd} not found; run scripts/fetch-dashcore.sh <dir> and set DASHCORE_DIR")
        if self.datadir is None:
            self.datadir = Path(tempfile.mkdtemp(prefix="dwd-regtest-"))
            self._owns_datadir = True
        self._log = open(self.datadir / "dashd.stdout", "w")
        self._process = subprocess.Popen(
            [
                str(dashd),
                "-regtest",
                f"-datadir={self.datadir}",
                "-printtoconsole",
                "-server",
                "-txindex=1",
                "-blockfilterindex=1",
                "-peerblockfilters=1",
                "-peerbloomfilters=1",
                "-listen=1",
                f"-port={self.p2p_port}",
                f"-bind=127.0.0.1:{self.p2p_port}",
                f"-rpcport={self.rpc_port}",
                f"-rpcbind=127.0.0.1:{self.rpc_port}",
                "-rpcallowip=127.0.0.1",
                f"-rpcuser={RPC_USER}",
                f"-rpcpassword={RPC_PASSWORD}",
                "-fallbackfee=0.00001",
                "-debug=net",
                "-debug=mempool",
                "-debug=rpc",
            ],
            stdout=self._log,
            stderr=subprocess.STDOUT,
        )
        try:
            self.wait_for_rpc()
        except Exception as err:
            # stop() deletes the temporary datadir, so carry the console log in the error.
            tail = "\n".join(self.logs().splitlines()[-50:])
            self.stop()
            raise RuntimeError(f"dashd did not become ready: {err}\n--- dashd log (tail) ---\n{tail}") from err

    def check_alive(self) -> None:
        if self._process is not None and self._process.poll() is not None:
            raise RuntimeError(f"dashd exited with status {self._process.returncode}")

    def stop(self) -> None:
        if self._process is not None and self._process.poll() is None:
            try:
                self.rpc.stop()
            except (RPCError, OSError):
                self._process.terminate()
            try:
                self._process.wait(timeout=30)
            except subprocess.TimeoutExpired:
                self._process.kill()
                self._process.wait()
        self._process = None
        if getattr(self, "_log", None) is not None:
            self._log.close()
        if self._owns_datadir and self.datadir is not None and not os.environ.get("DWD_KEEP_DATADIR"):
            shutil.rmtree(self.datadir, ignore_errors=True)

    def logs(self) -> str:
        if self.datadir is None:
            return ""
        path = self.datadir / "dashd.stdout"
        return path.read_text(errors="replace") if path.exists() else ""


def node_from_env() -> RegtestNode:
    """Pick the backend from DWD_REGTEST_BACKEND (`docker`, the default, or `local`)."""
    backend = os.environ.get("DWD_REGTEST_BACKEND", "docker")
    if backend == "docker":
        return DockerComposeNode(build=os.environ.get("DWD_REGTEST_BUILD", "1") != "0")
    if backend == "local":
        return LocalBinaryNode()
    raise ValueError(f"unknown DWD_REGTEST_BACKEND={backend!r} (expected 'docker' or 'local')")
