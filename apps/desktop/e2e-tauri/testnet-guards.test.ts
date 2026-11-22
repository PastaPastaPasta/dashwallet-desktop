import { spawnSync } from "node:child_process";
import { mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { faucetOutcome, releaseLock, takeLock } from "./testnet-guards.ts";

const TXID = "6cc9c41b192cc4cf99788aa5efc097a65bcb10a7a43eea7ff24394493c275d28";

describe("the testnet fixture's lock (H-04 O4a)", () => {
  let dir: string;
  let lock: string;
  beforeEach(() => {
    dir = mkdtempSync(join(tmpdir(), "dw-fixture-lock-"));
    lock = join(dir, ".lock");
  });
  afterEach(() => rmSync(dir, { recursive: true, force: true }));

  it("takes a free lock with its pid already in it, and leaves no other file", () => {
    takeLock(lock, 4242);
    expect(readFileSync(lock, "utf8")).toBe("4242\n");
    expect(readdirSync(dir)).toEqual([".lock"]);
    releaseLock(lock, 4241);
    expect(readdirSync(dir)).toEqual([".lock"]);
    releaseLock(lock, 4242);
    expect(readdirSync(dir)).toEqual([]);
  });

  it("holds an empty or unreadable lock: its writer may not have written the pid yet", () => {
    for (const text of ["", "\n", "12ab\n"]) {
      writeFileSync(lock, text);
      expect(() => takeLock(lock)).toThrow(/held/);
      expect(readFileSync(lock, "utf8")).toBe(text);
    }
  });

  it("holds a lock whose process runs, and takes over one whose process is gone", () => {
    writeFileSync(lock, `${process.pid}\n`);
    expect(() => takeLock(lock, 4242)).toThrow(/held|holds/);
    const gone = spawnSync(process.execPath, ["-e", ""]).pid!;
    writeFileSync(lock, `${gone}\n`);
    takeLock(lock, 4242);
    expect(readFileSync(lock, "utf8")).toBe("4242\n");
    expect(readdirSync(dir)).toEqual([".lock"]);
  });

  it("leaves a stale lock alone while another run is taking it over", () => {
    const gone = spawnSync(process.execPath, ["-e", ""]).pid!;
    writeFileSync(lock, `${gone}\n`);
    writeFileSync(`${lock}.takeover`, "4243\n");
    expect(() => takeLock(lock, 4242)).toThrow(/taking over/);
    expect(readFileSync(lock, "utf8")).toBe(`${gone}\n`);
  });
});

describe("the faucet helper's outcome (H-04 O4a)", () => {
  const run = (status: number | null, stderr: string, stdout = "") => faucetOutcome({ status, stdout, stderr });

  it("is paid only on exit 0 with a txid", () => {
    expect(run(0, "", JSON.stringify({ txid: TXID }))).toEqual({ kind: "paid", txid: TXID });
    expect(run(0, "", "{}").kind).toBe("unknown");
    expect(run(0, "", "not json").kind).toBe("unknown");
  });

  it("keeps an ambiguous failure unknown: a 5xx after the request, a timeout, a crash", () => {
    expect(run(1, "Faucet returned HTTP 502; not retrying.", "{}").kind).toBe("unknown");
    expect(run(1, "Outcome unknown: the payout may still have been broadcast.").kind).toBe("unknown");
    expect(run(1, "Traceback (most recent call last):\n  ConnectionResetError").kind).toBe("unknown");
    expect(faucetOutcome({ status: null, stderr: "", error: new Error("spawnSync python3 ETIMEDOUT") }).kind).toBe("unknown");
  });

  it("is refused only when the helper says it made no request or the faucet turned it down", () => {
    expect(run(2, "Refusing to send: not a valid Dash testnet P2PKH address (y...).").kind).toBe("refused");
    expect(run(1, "Faucet is not ready (or not testnet); no request was made.").kind).toBe("refused");
    expect(run(1, "Proof of work was not accepted.").kind).toBe("refused");
    expect(run(1, "This amount requires Turnstile; use the browser or configure an API key. Not retrying.").kind).toBe("refused");
    const limited = run(1, "Faucet returned HTTP 429; not retrying. Retry after 3600s.");
    expect(limited).toMatchObject({ kind: "refused", why: expect.stringContaining("Retry after 3600s") });
  });
});
