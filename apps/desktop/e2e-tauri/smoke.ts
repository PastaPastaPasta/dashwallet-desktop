// End-to-end smoke of the real app on the real engine (U-07): tauri-driver drives the
// production renderer in WebKitGTK through WebKitWebDriver. No fixture is involved: a fresh
// regtest data root gets an encrypted vault and a wallet from dwcli, and the app opens it.
//
// Flow: lock screen → wrong passphrase refused → unlock → Overview (zero balance, empty history)
// → Transactions → lock again. Screenshots and the app's log go to $DW_E2E_OUT.
//
// Run on Linux under a private X display (CI: xvfb-run; agentbox: `agent-gui start N`), never
// a developer's display:
//   DW_APP=…/dash-wallet DWCLI=…/dwcli TAURI_DRIVER=…/tauri-driver node e2e-tauri/smoke.ts
// `scripts/e2e-tauri.sh` builds the binaries and runs it. Node 24 runs this file as is (type
// stripping); it uses W3C WebDriver over fetch and no other dependency.

import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { createWriteStream, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const env = (name: string, fallback?: string): string => {
  const v = process.env[name] ?? fallback;
  if (!v) throw new Error(`set ${name}`);
  return v;
};

const APP = resolve(env("DW_APP"));
const DWCLI = resolve(env("DWCLI"));
const DRIVER = env("TAURI_DRIVER", "tauri-driver");
const OUT = resolve(env("DW_E2E_OUT", "test-results/tauri-e2e"));
const PORT = Number(env("DW_E2E_PORT", "4744"));
const PASSPHRASE = "e2e smoke passphrase";
// Regtest needs a DAPI address; with --no-platform nothing connects to it.
const CHAIN = ["--network", "regtest", "--no-platform", "--dapi", "https://127.0.0.1:1"];
const TIMEOUT_MS = Number(env("DW_E2E_TIMEOUT_MS", "120000"));

/** Calls `probe` every 250 ms until it returns a value, for at most `ms`. */
async function poll<T>(ms: number, what: string, probe: () => Promise<T | undefined>): Promise<T> {
  const until = Date.now() + ms;
  while (Date.now() < until) {
    const v = await probe();
    if (v !== undefined) return v;
    await new Promise((r) => setTimeout(r, 250));
  }
  throw new Error(`timed out after ${ms} ms waiting for ${what}`);
}

// ------------------------------------------------------------------ W3C WebDriver client

// The W3C web element identifier.
const ELEMENT = "element-6066-11e4-a52e-4f735466cecf";

class WebDriver {
  base: string;
  session = "";

  constructor(base: string) {
    this.base = base;
  }

  async call<T>(method: string, path: string, body?: unknown): Promise<T> {
    const res = await fetch(`${this.base}${path}`, {
      method,
      headers: { "content-type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const json = (await res.json()) as { value: T & { error?: string; message?: string } };
    if (!res.ok || (json.value && typeof json.value === "object" && "error" in json.value && json.value.error))
      throw new Error(`${method} ${path}: ${json.value?.error}: ${json.value?.message}`);
    return json.value;
  }

  async start(capabilities: unknown) {
    const v = await this.call<{ sessionId: string }>("POST", "/session", { capabilities });
    this.session = v.sessionId;
  }

  s = (path: string) => `/session/${this.session}${path}`;

  async find(css: string): Promise<string | null> {
    const found = await this.call<Record<string, string>[]>("POST", this.s("/elements"), { using: "css selector", value: css });
    return found[0]?.[ELEMENT] ?? null;
  }

  waitFor(css: string, what = css): Promise<string> {
    return poll(TIMEOUT_MS, what, async () => {
      const el = await this.find(css);
      return el && (await this.call<boolean>("GET", this.s(`/element/${el}/displayed`))) ? el : undefined;
    });
  }

  async waitGone(css: string, what = css) {
    await poll(TIMEOUT_MS, `${what} to go`, async () => ((await this.find(css)) ? undefined : true));
  }

  text = (el: string) => this.call<string>("GET", this.s(`/element/${el}/text`));
  click = (el: string) => this.call<null>("POST", this.s(`/element/${el}/click`), {});
  type = (el: string, text: string) => this.call<null>("POST", this.s(`/element/${el}/value`), { text });
  press = async (css: string) => void (await this.click(await this.waitFor(css)));
  fill = async (css: string, text: string) => void (await this.type(await this.waitFor(css), text));
  script = <T>(script: string) => this.call<T>("POST", this.s("/execute/sync"), { script, args: [] });

  async screenshot(name: string) {
    const png = await this.call<string>("GET", this.s("/screenshot"));
    writeFileSync(join(OUT, `${name}.png`), Buffer.from(png, "base64"));
  }

  end = () => this.call<null>("DELETE", this.s(""));
}

// ------------------------------------------------------------------ steps

const tid = (id: string) => `[data-testid="${id}"]`;
let driver: ChildProcess | undefined;
/** Holds the vault and its passphrase file: out of OUT, which CI uploads; removed at the end. */
let work: string | undefined;

function dwcli(dataRoot: string, passFile: string, ...args: string[]): string {
  const r = spawnSync(DWCLI, ["--datadir", dataRoot, ...CHAIN, "--passphrase-file", passFile, ...args], { encoding: "utf8" });
  if (r.status !== 0) throw new Error(`dwcli ${args.join(" ")} failed (${r.status}): ${r.stderr}`);
  return r.stdout;
}

const waitForDriver = (base: string) =>
  poll(30_000, "tauri-driver to listen", () =>
    fetch(`${base}/status`).then(
      (r) => (r.ok ? true : undefined),
      () => undefined,
    ),
  );

async function main() {
  mkdirSync(OUT, { recursive: true });
  work = mkdtempSync(join(tmpdir(), "dw-e2e-")); // mode 0700
  const dataRoot = join(work, "data");
  const passFile = join(work, "passphrase");
  writeFileSync(passFile, `${PASSPHRASE}\n`, { mode: 0o600 });

  dwcli(dataRoot, passFile, "init-vault");
  const walletId = /wallet_id (\S+)/.exec(dwcli(dataRoot, passFile, "create"))?.[1];
  if (!walletId) throw new Error("dwcli create printed no wallet id");
  console.log(`smoke: regtest data root ${dataRoot}, wallet ${walletId.slice(0, 16)}…`);

  const log = createWriteStream(join(OUT, "tauri-driver.log"));
  // Its own process group: the cleanup below ends the driver, WebKitWebDriver and the app.
  driver = spawn(DRIVER, ["--port", String(PORT), "--native-port", String(PORT + 1)], { stdio: ["ignore", "pipe", "pipe"], detached: true });
  driver.stdout!.pipe(log);
  driver.stderr!.pipe(log);
  const base = `http://127.0.0.1:${PORT}`;
  await waitForDriver(base);

  const wd = new WebDriver(base);
  await wd.start({ alwaysMatch: { "tauri:options": { application: APP, args: ["--datadir", dataRoot, ...CHAIN, "--theme", "light"] } } });
  const steps: string[] = [];
  const step = (s: string) => {
    steps.push(s);
    console.log(`smoke: ${s}`);
  };
  try {
    // The engine opened the vault locked; the production renderer has no fixture.
    const field = await wd.waitFor(tid("lock-field"), "the lock screen");
    if (await wd.script<boolean>("return !!window.__dwFixture")) throw new Error("the app runs a fixture");
    await wd.screenshot("1-lock-screen");
    step("lock screen on the real vault");

    await wd.type(field, "not the passphrase");
    await wd.press(tid("lock-submit"));
    const error = await wd.waitFor("#lock-error:not(:empty)", "the wrong-passphrase message");
    if (!(await wd.text(error)).includes("incorrect")) throw new Error(`unexpected lock error: ${await wd.text(error)}`);
    step("wrong passphrase refused by the vault");

    await wd.fill(tid("lock-field"), PASSPHRASE);
    await wd.press(tid("lock-submit"));
    // The shell is rendered under the lock screen, so wait for the lock screen to go first.
    await wd.waitGone(tid("lock-screen"), "the lock screen");
    await wd.waitFor(tid("hero"), "the Overview after unlock");
    await wd.waitFor(tid("history-empty"), "the empty history of a new wallet");
    await wd.screenshot("2-overview");
    step("unlocked: Overview of the new wallet, history empty");

    await wd.press(tid("sidebar-transactions"));
    await wd.waitFor(tid("tx-empty"), "the Transactions page");
    await wd.screenshot("3-transactions");
    step("Transactions page");

    await wd.press(tid("toolbar-lock"));
    await wd.waitFor(tid("lock-field"), "the lock screen after locking");
    await wd.screenshot("4-locked-again");
    step("locked again");
  } catch (e) {
    await wd.screenshot("failure").catch(() => undefined);
    throw e;
  } finally {
    await wd.end().catch(() => undefined);
  }
  writeFileSync(join(OUT, "result.json"), JSON.stringify({ ok: true, steps }, null, 2));
  console.log(`smoke: PASS (${steps.length} steps); screenshots in ${OUT}`);
}

main()
  .catch((e) => {
    console.error(`smoke: FAIL: ${e instanceof Error ? e.message : String(e)}`);
    process.exitCode = 1;
  })
  .finally(() => {
    // The driver's process group: tauri-driver, WebKitWebDriver and the app. Gone already is fine.
    try {
      if (driver?.pid) process.kill(-driver.pid, "SIGTERM");
    } catch {
      // ESRCH
    }
    try {
      if (work) rmSync(work, { recursive: true, force: true, maxRetries: 3 });
    } catch (e) {
      console.warn(`smoke: could not remove ${work}: ${e instanceof Error ? e.message : String(e)}`);
    }
  });
