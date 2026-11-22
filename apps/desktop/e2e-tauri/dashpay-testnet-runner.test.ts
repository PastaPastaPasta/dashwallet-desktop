// scripts/e2e-tauri-dashpay-testnet.sh run offline (H-04 O4a): a copy of the runner and the node
// files it calls, with stand-in flow scripts, binaries and `secret`, so the guards are exercised
// without a build, a display or testnet.

import { spawnSync } from "node:child_process";
import { chmodSync, cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readlinkSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

const APP = join(import.meta.dirname, "..");
const COPIED = ["scripts/e2e-tauri-dashpay-testnet.sh", "e2e-tauri/testnet-suite.ts", "e2e-tauri/artifact-scan.ts", "e2e-tauri/scratch.ts", "src/onboarding/bip39English.ts"];

const script = (path: string, body: string) => {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, `#!/usr/bin/env bash\nset -eu\n${body}\n`);
  chmodSync(path, 0o755);
};

describe.skipIf(process.platform !== "linux")("the DashPay testnet runner (H-04 O4a)", () => {
  let t: string;
  let app: string;
  let fixture: string;
  let store: string;
  let env: Record<string, string>;

  beforeEach(() => {
    t = mkdtempSync(join(tmpdir(), "dw-suite-runner-"));
    app = join(t, "repo/apps/desktop");
    for (const f of COPIED) {
      mkdirSync(dirname(join(app, f)), { recursive: true });
      cpSync(join(APP, f), join(app, f));
    }
    // Stand-in flows: registration writes E's phrase and passphrase as plain files, as
    // registration.ts does; profile-write counts its edits.
    script(
      join(app, "scripts/e2e-tauri-registration.sh"),
      `printf 'e phrase words\\n' >"$DW_TESTNET_FIXTURE/E.mnemonic"
printf 'e-pass\\n' >"$DW_REG_STATE/passphrase"
printf '{"walletId": "abcdef0123", "state": "done"}\\n' >"$DW_TESTNET_FIXTURE/DP1-09.json"
echo "registration: ok"`,
    );
    script(join(app, "scripts/e2e-tauri-profile-write.sh"), `echo edit >>"${t}/profile-writes"; echo "profile-write: ok"`);
    script(join(app, "scripts/e2e-tauri-dashpay-status.sh"), `echo "dashpay-status: ok"`);
    for (const bin of ["dash-wallet", "dwcli"]) script(join(t, "target/debug", bin), "exit 0");
    // The store: `secret adopt` moves the file in and leaves a link, or fails with FAKE_SECRET_FAIL=1.
    store = join(t, "store");
    mkdirSync(store);
    script(
      join(t, "bin/secret"),
      `[ "\${FAKE_SECRET_FAIL:-0}" = 1 ] && { echo "secret: the store is unreachable" >&2; exit 1; }
[ "$1" = adopt ] || exit 3
name=$3; path=$6
mv "$path" "${store}/$name"
[ "\${FAKE_SECRET_NOLINK:-0}" = 1 ] || ln -s "${store}/$name" "$path"`,
    );
    fixture = join(t, "home/fixture");
    mkdirSync(fixture, { recursive: true });
    const rows = Object.fromEntries(
      ["A", "B", "C", "D"].map((w) => [w, { wallet_id: `${w}0`, identity_id: `id${w}`, username: `dwo4a${w.toLowerCase()}2222`, display_name: `QA ${w}`, birth_height: 100, contacts: w === "A" ? ["B", "C"] : [] }]),
    );
    writeFileSync(join(fixture, "fixture.json"), JSON.stringify(rows));
    for (const [name, value] of [
      ["A.mnemonic", "a phrase words"],
      ["passphrase", "fixture-pass"],
    ]) {
      writeFileSync(join(store, name), `${value}\n`);
      symlinkSync(join(store, name), join(fixture, name));
    }
    env = {
      PATH: `${join(t, "bin")}:${process.env.PATH}`,
      HOME: join(t, "home"),
      TMPDIR: t,
      DISPLAY: ":0",
      DW_E2E_SKIP_BUILD: "1",
      CARGO_TARGET_DIR: join(t, "target"),
      DW_SECRET_STORE: store,
      DW_TESTNET_FIXTURE: fixture,
      DW_SUITE_OUT: join(t, "results"),
    };
  });
  afterEach(() => rmSync(t, { recursive: true, force: true }));

  const runner = (extra: Record<string, string> = {}, cwd = t) =>
    spawnSync("bash", [join(app, "scripts/e2e-tauri-dashpay-testnet.sh")], { cwd, env: { ...env, ...extra }, encoding: "utf8", timeout: 60_000 });
  const latest = () => join(t, "results", readlinkSync(join(t, "results/latest")));

  it("never empties a directory it did not create: each run gets a new one", () => {
    mkdirSync(join(t, "results"));
    writeFileSync(join(t, "results/keep.txt"), "mine");
    const r = runner({ DW_SUITE_FLOWS: "dashpay-status" });
    expect(r.status, r.stderr).toBe(0);
    expect(readFileSync(join(t, "results/keep.txt"), "utf8")).toBe("mine");
    expect(readFileSync(join(latest(), "summary.md"), "utf8")).toContain("| dashpay-status | PASS |");
  });

  it("refuses an output directory in a git work tree unless it is git-ignored", () => {
    const repo = join(t, "repo");
    spawnSync("git", ["init", "-q", repo]);
    writeFileSync(join(repo, "README"), "checkout");
    writeFileSync(join(repo, ".gitignore"), "results/\n");
    for (const out of [repo, join(app, "scripts"), join(repo, "fresh/out")]) {
      const r = runner({ DW_SUITE_OUT: out, DW_SUITE_FLOWS: "dashpay-status" });
      expect(r.status, out).toBe(2);
      expect(r.stderr).toMatch(/git work tree and is not git-ignored/);
    }
    expect(readFileSync(join(repo, "README"), "utf8")).toBe("checkout");
    expect(existsSync(join(app, "scripts/e2e-tauri-dashpay-testnet.sh"))).toBe(true);
    expect(existsSync(join(repo, "fresh"))).toBe(false);
    expect(runner({ DW_SUITE_OUT: ".", DW_SUITE_FLOWS: "dashpay-status" }, repo).status).toBe(2);
    // A git that cannot answer is not "outside a work tree".
    const broken = runner({ DW_SUITE_OUT: join(repo, "fresh/out"), DW_SUITE_FLOWS: "dashpay-status", GIT_DIR: join(t, "nowhere") });
    expect(broken.status).toBe(2);
    expect(readFileSync(join(repo, "README"), "utf8")).toBe("checkout");
    const r = runner({ DW_SUITE_OUT: join(repo, "results"), DW_SUITE_FLOWS: "dashpay-status" });
    expect(r.status, r.stderr).toBe(0);
  });

  it("fails the run when wallet E's secrets cannot be moved into the store", () => {
    const r = runner({ DW_SUITE_FLOWS: "registration dashpay-status", FAKE_SECRET_FAIL: "1" });
    expect(r.status).not.toBe(0);
    const md = readFileSync(join(latest(), "summary.md"), "utf8");
    expect(md).toContain("| secret-store | FAIL |");
    expect(md).not.toContain("dashpay-status");
  });

  it("fails the run when the store takes E's phrase but leaves no link", () => {
    const r = runner({ DW_SUITE_FLOWS: "registration", FAKE_SECRET_NOLINK: "1" });
    expect(r.status).not.toBe(0);
    expect(readFileSync(join(latest(), "summary.md"), "utf8")).toContain("| secret-store | FAIL |");
  });

  it("stops before any flow while a phrase or passphrase is a plain file", () => {
    writeFileSync(join(fixture, "E.mnemonic"), "e phrase words\n");
    const r = runner({ DW_SUITE_FLOWS: "profile-write" });
    expect(r.status).toBe(2);
    expect(r.stderr).toMatch(/E\.mnemonic is not a link into the secret store/);
    expect(existsSync(join(t, "profile-writes"))).toBe(false);
  });

  it("moves E's secrets into the store, and the next run passes the check", () => {
    expect(runner({ DW_SUITE_FLOWS: "registration" }).status).toBe(0);
    expect(readlinkSync(join(fixture, "E.mnemonic"))).toBe(join(store, "dwd-qa-testnet-fixture-E-abcdef01-mnemonic"));
    expect(readlinkSync(join(`${fixture}.reg-state`, "passphrase"))).toBe(join(store, "dwd-qa-testnet-fixture-E-abcdef01-reg-passphrase"));
    const again = runner({ DW_SUITE_FLOWS: "registration" });
    expect(again.status, again.stderr).toBe(0);
    expect(readFileSync(join(latest(), "summary.md"), "utf8")).toContain("| registration | SKIP |");
  });

  it("writes the profile once per fixture unless DW_PROFILE_WRITE_AGAIN=1", () => {
    const writes = () => readFileSync(join(t, "profile-writes"), "utf8").trim().split("\n").length;
    expect(runner({ DW_SUITE_FLOWS: "profile-write" }).status).toBe(0);
    expect(writes()).toBe(1);
    expect(runner({ DW_SUITE_FLOWS: "profile-write" }).status).toBe(0);
    expect(writes()).toBe(1);
    expect(readFileSync(join(latest(), "summary.md"), "utf8")).toContain("| profile-write | SKIP |");
    expect(runner({ DW_SUITE_FLOWS: "profile-write", DW_PROFILE_WRITE_AGAIN: "1" }).status).toBe(0);
    expect(writes()).toBe(2);
    // A read-only run (no edit) is never skipped.
    expect(runner({ DW_SUITE_FLOWS: "profile-write", DW_PROFILE_WRITE_AT: "never" }).status).toBe(0);
    expect(writes()).toBe(3);
  });

  it("records no profile edit when profile-write fails", () => {
    script(join(app, "scripts/e2e-tauri-profile-write.sh"), `echo "profile-write: FAIL: no edit"; exit 1`);
    expect(runner({ DW_SUITE_FLOWS: "profile-write" }).status).toBe(1);
    expect(existsSync(join(fixture, "BL-162.json"))).toBe(false);
  });
});
