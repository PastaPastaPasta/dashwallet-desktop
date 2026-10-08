// `--selftest` (DASHPAY §2.1 G-01): render the four gate screens in light and dark, record the
// box of every `data-testid` element and the U3(a) style checks, and hand the report to dw-app,
// which writes it and exits. In a browser (Playwright) the same routine is `window.__dwSelftest`.
// Under Tauri the report also carries the U5 IPC timings and the U6 security probes.

import type { QueryClient } from "@tanstack/react-query";
import type { Backend } from "../api/backend";
import { FIXTURE_PASSPHRASE } from "../api/fixture";
import { tauriTools } from "../api/tauri";
import { milestones, whenMarked } from "./startup";
import { runStyleChecks, type CheckResult, type Screen } from "./styleChecks";
import { ui } from "./ui";

export interface Box {
  id: string;
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface ScreenRecord {
  boxes: Box[];
  checks: CheckResult[];
}

export interface Report {
  userAgent: string;
  backend: Backend["kind"];
  devicePixelRatio: number;
  viewport: { width: number; height: number };
  interFontLoaded: boolean;
  textProbe: Record<string, number>;
  /** `<theme>/<screen>` → boxes and style checks. */
  screens: Record<string, ScreenRecord>;
  checksFailed: number;
  /** Checks of elements the data did not produce (e.g. no pending transaction, so no chip). */
  checksNotApplicable: number;
  checksTotal: number;
  /** U1: what the screens show, to compare with `dwcli` on the same datadir. */
  shown?: {
    heroAmount: string | null;
    rows: { txid: string; recordIndex: number; amount: string }[];
  };
  /** ms since `main`: the first window frame, and the first Overview frame with data. */
  shellPaintMs?: number;
  firstPaintMs?: number;
  /** Milestones in ms since the webview's navigation start, plus the navigation timing entry. */
  startup?: { milestones: Record<string, number>; navigation: Record<string, number> };
  ipc?: Record<string, Stats>;
  security?: Record<string, string>;
}

interface Stats {
  n: number;
  p50: number;
  p95: number;
  max: number;
}

const frame = () => new Promise<void>((r) => requestAnimationFrame(() => r()));
const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

/** Waits until queries are idle, fonts and images are loaded, finite animations and
 * transitions have finished and two frames have painted. */
async function settle(client: QueryClient) {
  // Busy until React has started and finished every query the screen needs: a freshly mounted
  // screen may not have begun fetching yet, so wait for two quiet checks in a row (≤ 60 s).
  const busy = () => client.isFetching() > 0 || client.isMutating() > 0 || !!document.querySelector(".loading-state, [data-testid='splash']");
  for (let i = 0, quiet = 0; i < 1200 && quiet < 2; i++) {
    await sleep(50);
    quiet = busy() ? 0 : quiet + 1;
  }
  await document.fonts.ready;
  await frame();
  await Promise.all(
    document
      .getAnimations()
      .filter((a) => a.effect?.getComputedTiming().iterations !== Infinity)
      .map((a) => a.finished.catch(() => undefined)),
  );
  await Promise.all(
    [...document.images].filter((img) => !img.complete).map((img) => new Promise((r) => img.addEventListener("load", r, { once: true }))),
  );
  await frame();
  await frame();
}

const round = (n: number) => Math.round(n * 100) / 100;

/** Widths of reference strings with and without the font features the screens use, so text
 * metric differences between engines can be attributed (U3c). */
function textProbe(): Record<string, number> {
  const cases: Record<string, string> = {
    amount700: "font: 700 34px Inter",
    amount700tnum: "font: 700 34px Inter; font-variant-numeric: tabular-nums",
    amount400: "font: 400 34px Inter",
    caps700: "font: 700 11px Inter",
    caps700ls: "font: 700 11px Inter; letter-spacing: 1.2px",
    body400: "font: 400 15px Inter",
  };
  const text: Record<string, string> = { amount: "30.39", caps: "TESTNET", body: "Search address, transaction id or label" };
  const host = document.createElement("div");
  host.style.cssText = "position:absolute;left:-9999px;top:0;white-space:nowrap";
  document.body.append(host);
  const out: Record<string, number> = {};
  for (const [name, css] of Object.entries(cases)) {
    const span = document.createElement("span");
    span.style.cssText = css;
    span.textContent = text[name.replace(/\d.*$/, "")];
    host.append(span);
    out[name] = round(span.getBoundingClientRect().width);
  }
  host.remove();
  return out;
}

function boxes(): Box[] {
  return [...document.querySelectorAll<HTMLElement>("[data-testid]")].map((el) => {
    const r = el.getBoundingClientRect();
    return { id: el.dataset.testid!, x: round(r.x), y: round(r.y), w: round(r.width), h: round(r.height) };
  });
}

function stats(samples: number[]): Stats {
  const s = [...samples].sort((a, b) => a - b);
  const q = (p: number) => round(s[Math.min(s.length - 1, Math.ceil(p * s.length) - 1)]);
  return { n: s.length, p50: q(0.5), p95: q(0.95), max: round(s[s.length - 1]) };
}

async function time(n: number, f: () => Promise<unknown>): Promise<Stats> {
  for (let i = 0; i < 20; i++) await f();
  const samples: number[] = [];
  for (let i = 0; i < n; i++) {
    const t = performance.now();
    await f();
    samples.push(performance.now() - t);
  }
  return stats(samples);
}

/** Called after each screen is recorded (the Tauri harness screenshots it). */
type OnScreen = (name: string) => Promise<void>;

/** Renders the gate screens in both appearances and records them. */
export async function collectReport(backend: Backend, client: QueryClient, onScreen?: OnScreen): Promise<Report> {
  const before = ui.get();
  const screens: Record<string, ScreenRecord> = {};
  let shown: Report["shown"];
  const record = async (theme: "light" | "dark", screen: Screen) => {
    screens[`${theme}/${screen}`] = { boxes: boxes(), checks: runStyleChecks(screen, theme) };
    await onScreen?.(`${theme}/${screen}`);
  };
  for (const theme of ["light", "dark"] as const) {
    ui.set({ theme, page: "overview", detail: null, discreet: false });
    await settle(client);
    await record(theme, "overview");
    const heroAmount = document.querySelector('[data-testid="hero-amount"] .amount-number')?.textContent ?? null;
    ui.set({ page: "transactions" });
    await settle(client);
    shown ??= {
      heroAmount,
      rows: [...document.querySelectorAll<HTMLElement>("[data-txid]")].slice(0, 200).map((el) => ({
        txid: el.dataset.txid!,
        recordIndex: Number(el.dataset.testid!.split("-").pop()),
        amount: el.querySelector(".tx-row-amount .amount-number")?.textContent ?? "",
      })),
    };
    await record(theme, "transactions");
    // The detail sheet never opens behind the lock screen (a real vault launched locked).
    const first = document.querySelector("[data-testid='lock-screen']") ? null : document.querySelector<HTMLElement>("[data-txid]");
    if (first) {
      ui.set({ detail: first.dataset.txid! });
      await settle(client);
      await record(theme, "txDetail");
      ui.set({ detail: null });
    }
    // Only the fixture can be locked and unlocked without the user's passphrase.
    if (backend.kind === "fixture") {
      await backend.lock();
      await settle(client);
      await record(theme, "unlock");
      await backend.unlock(FIXTURE_PASSPHRASE);
      await settle(client);
    }
  }
  ui.set({ ...before, detail: null });
  await settle(client);
  const all = Object.values(screens).flatMap((s) => s.checks);
  return {
    userAgent: navigator.userAgent,
    backend: backend.kind,
    devicePixelRatio: window.devicePixelRatio,
    viewport: { width: window.innerWidth, height: window.innerHeight },
    interFontLoaded: document.fonts.check('15px "Inter"'),
    textProbe: textProbe(),
    screens,
    shown,
    checksFailed: all.filter((c) => c.pass === false).length,
    checksNotApplicable: all.filter((c) => c.pass === null).length,
    checksTotal: all.length,
  };
}

/** `window.__dwSelftest()` for Playwright and manual runs. */
export function exposeSelftest(backend: Backend, client: QueryClient) {
  (window as unknown as { __dwSelftest: () => Promise<Report> }).__dwSelftest = () => collectReport(backend, client);
}

async function refused(f: () => Promise<unknown>): Promise<string> {
  try {
    const v = await f();
    return `NOT REFUSED: ${JSON.stringify(v)}`;
  } catch (e) {
    return `refused: ${e instanceof Error ? e.message : String(e)}`;
  }
}

/** Under Tauri: the report plus IPC timings and security probes, then dw-app writes it and exits. */
export async function runTauriSelftest(backend: Backend, client: QueryClient, pauseMs: number) {
  const shellPaintMs = await whenMarked("shell");
  const firstPaintMs = await whenMarked("first-paint");
  const nav = performance.getEntriesByType("navigation")[0] as PerformanceNavigationTiming | undefined;
  const navigation: Record<string, number> = nav
    ? { responseEnd: round(nav.responseEnd), domInteractive: round(nav.domInteractive), domContentLoaded: round(nav.domContentLoadedEventEnd), load: round(nav.loadEventEnd) }
    : {};
  await sleep(300);
  const report = await collectReport(backend, client, async (name) => {
    if (pauseMs <= 0) return;
    await tauriTools.selftestCheckpoint(name);
    await sleep(pauseMs);
  });
  report.shellPaintMs = shellPaintMs;
  report.firstPaintMs = firstPaintMs;
  report.startup = { milestones: Object.fromEntries(Object.entries(milestones).map(([k, v]) => [k, round(v)])), navigation };

  report.ipc = { ipcBenchRows200: await time(200, () => tauriTools.ipcBenchRows(200)) };
  if (backend.kind === "tauri") {
    const wallet = (await backend.listWallets())[0];
    if (wallet) {
      const request = { categories: [], text: null, sort: "newestFirst" as const, cursor: null, limit: 200 };
      report.ipc.historyPage200 = await time(200, () => backend.historyPage(wallet.walletId, request));
      // Same engine work, one-row payload: separates the engine's query cost from IPC and rendering.
      report.ipc.historyPage1 = await time(50, () => backend.historyPage(wallet.walletId, { ...request, limit: 1 }));
    }
  }

  report.security = {
    unlistedCommand: await refused(() => tauriTools.probeUnlisted()),
    unknownPluginCommand: await refused(() => tauriTools.invokeUnknownPlugin()),
    remoteFetch: await refused(() => fetch("https://example.com/", { cache: "no-store" })),
  };
  // Navigation probe: if the guard failed, the page would leave and the report below would never
  // arrive, so the first write keeps everything measured so far.
  await tauriTools.selftestReport({ ...report, final: false });
  const href = location.href;
  const a = document.createElement("a");
  a.href = "https://example.com/";
  document.body.append(a);
  a.click();
  await sleep(1500);
  report.security.navigation = location.href === href ? "refused: still on the app origin" : `NOT REFUSED: ${location.href}`;
  a.remove();
  await tauriTools.selftestReport({ ...report, final: true });
}
