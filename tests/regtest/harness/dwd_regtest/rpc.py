"""Minimal Dash Core JSON-RPC client (stdlib only).

Amounts are decoded as `decimal.Decimal` so balances compare exactly.
"""

from __future__ import annotations

import base64
import itertools
import json
import urllib.error
import urllib.request
from decimal import Decimal
from typing import Any

# Dash Core error code returned while the node is still loading (RPC_IN_WARMUP).
RPC_IN_WARMUP = -28


class RPCError(Exception):
    """A JSON-RPC error object returned by dashd."""

    def __init__(self, method: str, code: int, message: str):
        super().__init__(f"{method}: [{code}] {message}")
        self.method = method
        self.code = code
        self.message = message


class RPCClient:
    """Calls dashd RPC at `url` with HTTP basic auth.

    `wallet` selects the `/wallet/<name>` endpoint for wallet RPCs.
    """

    def __init__(self, url: str, user: str, password: str, wallet: str | None = None, timeout: float = 60.0):
        self.url = url.rstrip("/")
        self.user = user
        self.password = password
        self.wallet = wallet
        self.timeout = timeout
        self._ids = itertools.count(1)
        token = base64.b64encode(f"{user}:{password}".encode()).decode()
        self._auth_header = f"Basic {token}"

    def for_wallet(self, name: str) -> "RPCClient":
        return RPCClient(self.url, self.user, self.password, wallet=name, timeout=self.timeout)

    def call(self, method: str, *params: Any) -> Any:
        endpoint = self.url if self.wallet is None else f"{self.url}/wallet/{self.wallet}"
        body = json.dumps(
            {"jsonrpc": "1.0", "id": next(self._ids), "method": method, "params": list(params)},
            default=_encode_decimal,
        ).encode()
        request = urllib.request.Request(
            endpoint,
            data=body,
            headers={"Authorization": self._auth_header, "Content-Type": "application/json"},
        )
        try:
            with urllib.request.urlopen(request, timeout=self.timeout) as response:
                payload = response.read()
        except urllib.error.HTTPError as err:
            # dashd answers RPC errors with HTTP 404/500 and a JSON body; anything else is transport-level.
            payload = err.read()
            if not payload:
                raise
        reply = json.loads(payload, parse_float=Decimal)
        if reply.get("error"):
            error = reply["error"]
            raise RPCError(method, error.get("code", 0), error.get("message", ""))
        return reply["result"]

    def __getattr__(self, method: str):
        if method.startswith("_"):
            raise AttributeError(method)
        return lambda *params: self.call(method, *params)


def _encode_decimal(value: Any) -> Any:
    if isinstance(value, Decimal):
        # dashd's AmountFromValue accepts amounts as strings, which keeps them exact.
        return str(value)
    raise TypeError(f"not JSON serializable: {type(value).__name__}")
