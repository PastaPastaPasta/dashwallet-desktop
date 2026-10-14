// The W3C WebDriver client and the tauri-driver plumbing that the T1 scripts share (smoke.ts,
// U-07; onboarding.ts, U-02). WebDriver over fetch, no other dependency: Node 24 runs the
// scripts as they are (type stripping).

import { spawn, type ChildProcess } from "node:child_process";
import { createWriteStream, writeFileSync } from "node:fs";
import { join } from "node:path";

export const env = (name: string, fallback?: string): string => {
  const v = process.env[name] ?? fallback;
  if (!v) throw new Error(`set ${name}`);
  return v;
};

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

// The W3C web element identifier.
const ELEMENT = "element-6066-11e4-a52e-4f735466cecf";

export class WebDriver {
  base: string;
  out: string;
  timeoutMs: number;
  session = "";

  constructor(base: string, out: string, timeoutMs: number) {
    this.base = base;
    this.out = out;
    this.timeoutMs = timeoutMs;
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
    return poll(this.timeoutMs, what, async () => {
      const el = await this.find(css);
      return el && (await this.call<boolean>("GET", this.s(`/element/${el}/displayed`))) ? el : undefined;
    });
  }

  async findAll(css: string): Promise<string[]> {
    const found = await this.call<Record<string, string>[]>("POST", this.s("/elements"), { using: "css selector", value: css });
    return found.map((f) => f[ELEMENT]!);
  }

  async waitGone(css: string, what = css) {
    await poll(this.timeoutMs, `${what} to go`, async () => ((await this.find(css)) ? undefined : true));
  }

  text = (el: string) => this.call<string>("GET", this.s(`/element/${el}/text`));
  click = (el: string) => this.call<null>("POST", this.s(`/element/${el}/click`), {});
  type = (el: string, text: string) => this.call<null>("POST", this.s(`/element/${el}/value`), { text });
  clear = (el: string) => this.call<null>("POST", this.s(`/element/${el}/clear`), {});
  press = async (css: string) => void (await this.click(await this.waitFor(css)));
  fill = async (css: string, text: string) => void (await this.type(await this.waitFor(css), text));
  script = <T>(script: string, args: unknown[] = []) => this.call<T>("POST", this.s("/execute/sync"), { script, args });

  async screenshot(name: string) {
    const png = await this.call<string>("GET", this.s("/screenshot"));
    writeFileSync(join(this.out, `${name}.png`), Buffer.from(png, "base64"));
  }

  end = () => this.call<null>("DELETE", this.s(""));
}

/** Starts tauri-driver on `port` (and `port + 1` for WebKitWebDriver) in its own process group,
 * so `stopDriver` ends the driver, WebKitWebDriver and the app; its output goes to `log`.
 * `closed` settles once every process holding the output is gone and `log` is on disk (or after
 * 30 s). */
export async function startDriver(
  driverPath: string,
  port: number,
  log: string,
): Promise<{ driver: ChildProcess; base: string; closed: Promise<void> }> {
  const out = createWriteStream(log);
  const driver = spawn(driverPath, ["--port", String(port), "--native-port", String(port + 1)], { stdio: ["ignore", "pipe", "pipe"], detached: true });
  // Two sources: neither may end the file before the other is done.
  driver.stdout!.pipe(out, { end: false });
  driver.stderr!.pipe(out, { end: false });
  const flushed = new Promise<void>((resolve) => driver.on("close", () => out.end(() => resolve())));
  const closed = Promise.race([flushed, new Promise<void>((resolve) => setTimeout(resolve, 30_000).unref())]);
  const base = `http://127.0.0.1:${port}`;
  await poll(30_000, "tauri-driver to listen", () =>
    fetch(`${base}/status`).then(
      (r) => (r.ok ? true : undefined),
      () => undefined,
    ),
  );
  return { driver, base, closed };
}

/** Ends the driver's process group: tauri-driver, WebKitWebDriver and the app. Gone already is fine. */
export function stopDriver(driver: ChildProcess | undefined) {
  try {
    if (driver?.pid) process.kill(-driver.pid, "SIGTERM");
  } catch {
    // ESRCH
  }
}
