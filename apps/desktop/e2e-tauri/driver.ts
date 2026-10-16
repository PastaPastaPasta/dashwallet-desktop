// What the tauri-driver runs share (smoke.ts, regtest-send.ts, onboarding.ts): settings from the environment,
// polling, a W3C WebDriver client over fetch, tauri-driver itself and dwcli. Node 24 runs these
// files as they are (type stripping); no other dependency.

import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { closeSync, openSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

export const env = (name: string, fallback?: string): string => {
  const v = process.env[name] ?? fallback;
  if (!v) throw new Error(`set ${name}`);
  return v;
};

export const APP = resolve(env("DW_APP"));
export const DWCLI = resolve(env("DWCLI"));
export const DRIVER = env("TAURI_DRIVER", "tauri-driver");
export const TIMEOUT_MS = Number(env("DW_E2E_TIMEOUT_MS", "120000"));
/** Regtest needs a DAPI address; with --no-platform nothing connects to it. */
export const CHAIN = ["--network", "regtest", "--no-platform", "--dapi", "https://127.0.0.1:1"];

/** Calls `probe` every 250 ms until it returns a value, for at most `ms`. */
export async function poll<T>(ms: number, what: string, probe: () => Promise<T | undefined>): Promise<T> {
  const until = Date.now() + ms;
  while (Date.now() < until) {
    const v = await probe();
    if (v !== undefined) return v;
    await new Promise((r) => setTimeout(r, 250));
  }
  throw new Error(`timed out after ${ms} ms waiting for ${what}`);
}

export const tid = (id: string) => `[data-testid="${id}"]`;

// ------------------------------------------------------------------ W3C WebDriver client

// The W3C web element identifier.
const ELEMENT = "element-6066-11e4-a52e-4f735466cecf";

export class WebDriver {
  base: string;
  out: string;
  session = "";

  constructor(base: string, out: string) {
    this.base = base;
    this.out = out;
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

  waitFor(css: string, what = css, ms = TIMEOUT_MS): Promise<string> {
    return poll(ms, what, async () => {
      const el = await this.find(css);
      return el && (await this.call<boolean>("GET", this.s(`/element/${el}/displayed`))) ? el : undefined;
    });
  }

  async findAll(css: string): Promise<string[]> {
    const found = await this.call<Record<string, string>[]>("POST", this.s("/elements"), { using: "css selector", value: css });
    return found.map((f) => f[ELEMENT]!);
  }

  async waitGone(css: string, what = css) {
    await poll(TIMEOUT_MS, `${what} to go`, async () => ((await this.find(css)) ? undefined : true));
  }

  text = (el: string) => this.call<string>("GET", this.s(`/element/${el}/text`));
  click = (el: string) => this.call<null>("POST", this.s(`/element/${el}/click`), {});
  type = (el: string, text: string) => this.call<null>("POST", this.s(`/element/${el}/value`), { text });
  clear = (el: string) => this.call<null>("POST", this.s(`/element/${el}/clear`), {});
  press = async (css: string) => void (await this.click(await this.waitFor(css)));
  fill = async (css: string, text: string) => void (await this.type(await this.waitFor(css), text));
  script = <T>(script: string, args: unknown[] = []) => this.call<T>("POST", this.s("/execute/sync"), { script, args });
  /** `script` gets a callback as its last argument and answers through it. */
  scriptAsync = <T>(script: string, args: unknown[] = []) => this.call<T>("POST", this.s("/execute/async"), { script, args });

  async screenshot(name: string) {
    const png = await this.call<string>("GET", this.s("/screenshot"));
    writeFileSync(join(this.out, `${name}.png`), Buffer.from(png, "base64"));
  }

  end = () => this.call<null>("DELETE", this.s(""));
}

// ------------------------------------------------------------------ tauri-driver and dwcli

/** Starts tauri-driver on `port` (and `port + 1` for WebKitWebDriver), logging to `out`, in its
 * own process group so `stopDriver` ends the driver, WebKitWebDriver and the app. */
/** Starts tauri-driver and waits until it listens; `spawned` gets the child at once, so a signal
 * while it starts can still stop it. */
export async function startDriver(port: number, out: string, spawned?: (driver: ChildProcess) => void): Promise<{ driver: ChildProcess; base: string }> {
  // A file, not a pipe: the app logs through the driver, and `spawnSync` (dwcli) blocks the event
  // loop that would drain a pipe, so a full one would stop the app.
  const log = openSync(join(out, "tauri-driver.log"), "w");
  const driver = spawn(DRIVER, ["--port", String(port), "--native-port", String(port + 1)], { stdio: ["ignore", log, log], detached: true });
  closeSync(log);
  spawned?.(driver);
  const base = `http://127.0.0.1:${port}`;
  await poll(30_000, "tauri-driver to listen", () =>
    fetch(`${base}/status`).then(
      (r) => (r.ok ? true : undefined),
      () => undefined,
    ),
  );
  return { driver, base };
}

/** Ends the driver's process group: tauri-driver, WebKitWebDriver and the app. Gone already is fine. */
export function stopDriver(driver: ChildProcess | undefined) {
  try {
    if (driver?.pid) process.kill(-driver.pid, "SIGTERM");
  } catch {
    // ESRCH
  }
}

/** Settles once every process of the driver's group has exited, so the log they write is
 * complete (or after `ms`): a leak scan of OUT must not race the app's last lines. */
export async function driverGone(driver: ChildProcess | undefined, ms = 30_000) {
  const pid = driver?.pid;
  if (!pid) return;
  await poll(ms, "the driver's processes to exit", async () => {
    try {
      process.kill(-pid, 0);
      return undefined;
    } catch {
      return true;
    }
  }).catch(() => undefined);
}

export function dwcli(dataRoot: string, passFile: string, ...args: string[]): string {
  const r = spawnSync(DWCLI, ["--datadir", dataRoot, ...CHAIN, "--passphrase-file", passFile, ...args], { encoding: "utf8" });
  if (r.status !== 0) throw new Error(`dwcli ${args.join(" ")} failed (${r.status}): ${r.stderr}`);
  return r.stdout;
}

/** The leak scan: it skips symlinks (BL-162). */
export { leaks } from "./scratch.ts";
