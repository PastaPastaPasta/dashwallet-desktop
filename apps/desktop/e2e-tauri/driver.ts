// What the tauri-driver runs share (smoke.ts, regtest-send.ts): settings from the environment,
// polling, a W3C WebDriver client over fetch, tauri-driver itself and dwcli. Node 24 runs these
// files as they are (type stripping); no other dependency.

import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { createWriteStream, writeFileSync } from "node:fs";
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
    writeFileSync(join(this.out, `${name}.png`), Buffer.from(png, "base64"));
  }

  end = () => this.call<null>("DELETE", this.s(""));
}

// ------------------------------------------------------------------ tauri-driver and dwcli

/** Starts tauri-driver on `port` (and `port + 1` for WebKitWebDriver), logging to `out`, in its
 * own process group so `stopDriver` ends the driver, WebKitWebDriver and the app. */
export async function startDriver(port: number, out: string): Promise<{ driver: ChildProcess; base: string }> {
  const log = createWriteStream(join(out, "tauri-driver.log"));
  const driver = spawn(DRIVER, ["--port", String(port), "--native-port", String(port + 1)], { stdio: ["ignore", "pipe", "pipe"], detached: true });
  driver.stdout!.pipe(log);
  driver.stderr!.pipe(log);
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

export function dwcli(dataRoot: string, passFile: string, ...args: string[]): string {
  const r = spawnSync(DWCLI, ["--datadir", dataRoot, ...CHAIN, "--passphrase-file", passFile, ...args], { encoding: "utf8" });
  if (r.status !== 0) throw new Error(`dwcli ${args.join(" ")} failed (${r.status}): ${r.stderr}`);
  return r.stdout;
}
