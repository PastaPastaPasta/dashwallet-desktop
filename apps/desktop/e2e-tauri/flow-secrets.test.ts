// flow-secrets.ts with the real WebDriver against a stand-in WebDriver server and a stand-in
// `secret` (H-04 O4a): a suite flow's passphrase is in the store and its name in the manifest
// before the flow's first screenshot, and a screenshot before that is refused.

import { createServer, type Server } from "node:http";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const FLOWS = ["dashpay-status", "contacts", "registration", "add-contact", "bell-accept", "pay-contact", "profile-write"];

describe.skipIf(process.platform === "win32")("a flow's own secrets (H-04 O4a)", () => {
  let t: string;
  let store: string;
  let manifest: string;
  let server: Server;
  let base: string;
  /** The manifest as it stood when each screenshot request reached the server. */
  let atCapture: string[];
  const saved = { ...process.env };

  beforeEach(async () => {
    t = mkdtempSync(join(tmpdir(), "dw-flow-secrets-"));
    store = join(t, "store");
    mkdirSync(store);
    mkdirSync(join(t, "bin"));
    writeFileSync(
      join(t, "bin/secret"),
      `#!/bin/sh
[ "\${FAKE_SECRET_FAIL:-0}" = 1 ] && { echo "secret: the store is unreachable" >&2; exit 1; }
case "$1" in
  set) cat >"${store}/$2" ;;
  get) cat "${store}/$2" ;;
  *) exit 3 ;;
esac
`,
    );
    chmodSync(join(t, "bin/secret"), 0o755);
    manifest = join(t, "secrets.manifest");
    Object.assign(process.env, {
      PATH: `${join(t, "bin")}:${saved.PATH}`,
      DW_APP: "/bin/true",
      DWCLI: "/bin/true",
      DW_SECRET_MANIFEST: manifest,
      DW_SECRET_RUN_PREFIX: "dwd-qa-run-x-dashpay-status",
    });
    atCapture = [];
    server = createServer((_req, res) => {
      atCapture.push(existsSync(manifest) ? readFileSync(manifest, "utf8") : "");
      res.setHeader("content-type", "application/json");
      res.end(JSON.stringify({ value: Buffer.from("png").toString("base64") }));
    });
    await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
    base = `http://127.0.0.1:${(server.address() as { port: number }).port}`;
    vi.resetModules();
  });
  afterEach(async () => {
    await new Promise((r) => server.close(r));
    process.env = { ...saved };
    rmSync(t, { recursive: true, force: true });
  });

  it("puts the flow's passphrase in the store and its name in the manifest before the first capture", async () => {
    const { WebDriver } = await import("./driver.ts");
    const { newPassphrase } = await import("./flow-secrets.ts");
    const wd = new WebDriver(base, t);
    wd.session = "s1";
    await expect(wd.screenshot("too-early")).rejects.toThrow(/before the flow registered its secrets/);
    expect(atCapture).toEqual([]);
    expect(existsSync(join(t, "too-early.png"))).toBe(false);

    const passphrase = newPassphrase();
    const entry = { name: "dwd-qa-run-x-dashpay-status-passphrase" };
    expect(readFileSync(manifest, "utf8")).toBe(`${JSON.stringify(entry)}\n`);
    expect(readFileSync(join(store, entry.name), "utf8")).toBe(passphrase);
    await wd.screenshot("1-home");
    expect(atCapture).toEqual([`${JSON.stringify(entry)}\n`]);
    expect(readFileSync(manifest, "utf8")).not.toContain(passphrase);
  });

  it("stops the flow when the store refuses its passphrase", async () => {
    const { newPassphrase } = await import("./flow-secrets.ts");
    process.env.FAKE_SECRET_FAIL = "1";
    expect(() => newPassphrase()).toThrow(/could not put passphrase in the secret store/);
    expect(existsSync(manifest)).toBe(false);
  });

  it("is how each of the seven flows makes the secrets it types", () => {
    for (const flow of FLOWS) {
      const src = readFileSync(join(import.meta.dirname, `${flow}.ts`), "utf8");
      expect(src, flow).toMatch(/from "\.\/flow-secrets\.ts"/);
      expect(src, flow).not.toMatch(/=\s*randomBytes\(\d+\)\.toString\("base64url"\)/);
      expect(src, flow).toMatch(/newPassphrase\(\)|keepSecretAt\("E-passphrase"/);
    }
  });
});
