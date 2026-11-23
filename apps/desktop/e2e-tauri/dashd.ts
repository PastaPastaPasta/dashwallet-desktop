// A regtest dashd for the T1 runs (regtest-send.ts, tools.ts): genesis, RPC through dash-cli,
// private ports, its log straight into a file. dashd is the chain and the judge.

import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { closeSync, openSync } from "node:fs";
import { createServer } from "node:net";
import { dirname, join } from "node:path";

/** A free loopback port. */
export function freePort(): Promise<number> {
  return new Promise((ok, fail) => {
    const s = createServer();
    s.once("error", fail);
    s.listen(0, "127.0.0.1", () => {
      const { port } = s.address() as { port: number };
      s.close(() => ok(port));
    });
  });
}

/** A regtest dashd at genesis with RPC through dash-cli; `stop` ends it. */
export class Dashd {
  child: ChildProcess;
  dashd: string;
  datadir: string;
  rpcPort: number;
  p2pPort: number;

  constructor(dashd: string, datadir: string, p2pPort: number, rpcPort: number, logFile: string) {
    this.dashd = dashd;
    this.datadir = datadir;
    this.p2pPort = p2pPort;
    this.rpcPort = rpcPort;
    // Straight into the file: a pipe is read by this process's event loop, which `spawnSync`
    // (dash-cli, dwcli) blocks, and dashd stops at a full pipe (mining froze at block 25).
    const log = openSync(logFile, "w");
    // As regtest/README.md's node, on private ports.
    this.child = spawn(
      dashd,
      [
        "-regtest",
        `-datadir=${datadir}`,
        "-server",
        "-printtoconsole",
        "-txindex=1",
        "-blockfilterindex=1",
        "-peerblockfilters=1",
        "-peerbloomfilters=1",
        "-listen=1",
        "-listenonion=0",
        `-port=${p2pPort}`,
        `-bind=127.0.0.1:${p2pPort}`,
        `-rpcport=${rpcPort}`,
        `-rpcbind=127.0.0.1:${rpcPort}`,
        "-rpcallowip=127.0.0.1",
        "-rpcuser=dw",
        "-rpcpassword=dw",
        "-fallbackfee=0.00001",
      ],
      { stdio: ["ignore", log, log] },
    );
    closeSync(log);
  }

  tryCli(...args: string[]): { ok: boolean; out: string } {
    const r = spawnSync(join(dirname(this.dashd), "dash-cli"), ["-regtest", `-datadir=${this.datadir}`, `-rpcport=${this.rpcPort}`, "-rpcuser=dw", "-rpcpassword=dw", ...args], {
      encoding: "utf8",
    });
    return { ok: r.status === 0, out: (r.status === 0 ? r.stdout : r.stderr).trim() };
  }

  cli(...args: string[]): string {
    const r = this.tryCli(...args);
    if (!r.ok) throw new Error(`dash-cli ${args.join(" ")}: ${r.out}`);
    return r.out;
  }

  json = <T>(...args: string[]) => JSON.parse(this.cli(...args)) as T;
  height = () => Number(this.cli("getblockcount"));

  mine(blocks: number): string[] {
    return this.json<string[]>("generatetoaddress", String(blocks), this.cli("-rpcwallet=miner", "getnewaddress"));
  }

  /** Stops dashd: `stop` over RPC, SIGKILL after 30 s. */
  stop(): Promise<void> {
    if (this.child.exitCode !== null) return Promise.resolve();
    const exited = new Promise<void>((ok) => this.child.once("exit", () => ok()));
    this.tryCli("stop");
    const kill = setTimeout(() => this.child.kill("SIGKILL"), 30_000);
    return exited.finally(() => clearTimeout(kill));
  }
}

export interface RawTx {
  txid: string;
  confirmations?: number;
  blockhash?: string;
  vout: { value: number; scriptPubKey: { address?: string; addresses?: string[] } }[];
}

export const paysTo = (tx: RawTx, address: string) =>
  tx.vout.filter((o) => o.scriptPubKey.address === address || o.scriptPubKey.addresses?.includes(address)).reduce((n, o) => n + Math.round(o.value * 1e8), 0);
