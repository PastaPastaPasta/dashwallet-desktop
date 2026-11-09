// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
// The fixture backend (U-07): an engine-free stand-in for dw-app at the IPC boundary. It
// installs Tauri's mock IPC, so the renderer runs its real code path (the generated command
// wrappers and the `engine://event` listener) against a scenario's `World`.
//
// Never part of a production build: main.tsx reaches this module only through
// `if (__DW_FIXTURES__) await import(...)`, which `vite build` drops, and
// scripts/check-bundle.mjs fails the build if `FIXTURE_SENTINEL` shows up in `dist/`.

import { mockIPC } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";
import { COMMANDS, ENGINE_EVENT, SHELL_EVENT, type AppError, type AppEvent, type CommandName, type ShellEvent } from "../api/generated/commands";
import type { LinkRoute } from "../api/generated/LinkRoute";
import type { ShellCommand } from "../api/generated/ShellCommand";
import type { ThemeName } from "../api/generated/ThemeName";
import { isErrorClass, sampleError, type ErrorClass } from "./errors";
import { landSend, PENDING_HANDLERS, switchedWorld } from "./pending";
import { HANDLERS, registrationRun, type Ctx } from "./handlers";
import { restarted } from "./registration";
import { buildWorld, DEFAULT_SCENARIO, FIXTURE_WALLET_ID, isScenario, type ScenarioName, type World } from "./scenarios";
import { receivePayment } from "./send";
import { FIXTURE_NOW } from "./wallet";

/** Present in every fixture build and in no production build (scripts/check-bundle.mjs). */
export const FIXTURE_SENTINEL = "dw-fixture-backend:U-07";

export interface FixtureOptions {
  scenario: ScenarioName;
  theme?: ThemeName | null;
  /** Commands that reject with a sample of the error class instead of running. */
  failures?: Partial<Record<CommandName, ErrorClass>>;
  /** Every engine command answers after this many ms (loading states). `launch_info` does not
   * wait: dw-app answers it from its launch options, before the window shows anything. */
  delayMs?: number;
  /** Commands whose first call is held from the page's start (`hold=<command>`): a read that is still
   * out when the screen shows. `release(command)` lets it answer. A relaunch holds them again. */
  holds?: CommandName[];
}

export interface FixtureHandle {
  readonly sentinel: string;
  readonly scenario: ScenarioName;
  readonly world: World;
  /** Sends an engine event, as dw-app's bridge would. */
  emit(event: AppEvent): void;
  /** A payment of `duffs` arrives at the wallet's current receiving address (the next one is current
   * from then on): its row, `historyChanged` and `newTransactions`. Returns the address paid. */
  receivePayment(duffs: number, walletId?: string, to?: string): string;
  /** Queues links as dw-app's shell routed them and says so (`linksPending`), as a clicked link
   * or a second launch would. */
  deliverLinks(routes: LinkRoute[]): void;
  /** A native menu or tray command. */
  shellCommand(command: ShellCommand): void;
  /** Makes `command` reject with a sample of `cls` from now on (`null` clears it). */
  fail(command: CommandName, cls: ErrorClass | null): void;
  /** Holds the next call of `command`: it answers from the world as it is when called, but only
   * once the returned function is called (a reply that arrives late). */
  hold(command: CommandName): () => void;
  /** Releases the call a `hold=` URL option held at the start (no-op when none is out). */
  release(command: CommandName): void;
  /** Every command called so far, in order. */
  readonly calls: string[];
  /** Platform takes a held send after all (its row becomes `done`, the credits are debited), with
   * no signal to the app, as the engine settles it between two reads. */
  landPlatformSend(id: string): void;
  /** One step of `draft`'s registration run, where the network would answer (DP1-09);
   * `false` when it has no run. */
  advanceRegistration(draft: string, walletId?: string): boolean;
  /** `draft`'s run fails with `code`: `retryable` keeps it for Continue; a `name.*` code ends it
   * with the identity but no name. */
  failRegistration(draft: string, code: string, retryable: boolean, walletId?: string): boolean;
  /** Quits and relaunches the app (DP1-09): the world as the engine left it on disk, without
   * its grants, leases and runs, and the vault locked. Reloads the page. */
  relaunch(): void;
  /** The renderer's clock (`Backend.now`). A test may move it; the handlers keep FIXTURE_NOW. */
  now: Date;
  readonly locale: string;
}

const notImplemented = (call: string): AppError => ({
  code: "platform.not_implemented",
  message: `the fixture backend has no handler for ${call}`,
  data: { kind: "call", call },
});

/** Where `relaunch` leaves the world for the next page load (one tab's session). */
const RELAUNCH_KEY = "dw-fixture-relaunch";

/** The world `relaunch` left, if this load is one; consumed. */
function relaunchedWorld(): World | null {
  if (typeof sessionStorage === "undefined") return null;
  const saved = sessionStorage.getItem(RELAUNCH_KEY);
  if (saved === null) return null;
  sessionStorage.removeItem(RELAUNCH_KEY);
  return restarted(JSON.parse(saved) as World);
}

export function installFixture(opts: FixtureOptions): FixtureHandle {
  const world = relaunchedWorld() ?? switchedWorld() ?? buildWorld(opts.scenario, opts.theme ?? null);
  const failures = new Map(Object.entries(opts.failures ?? {}) as [CommandName, ErrorClass][]);
  const calls: string[] = [];
  const held = new Map<string, Promise<void>>();
  const startHold = (command: string) => {
    let release = () => {};
    held.set(command, new Promise<void>((r) => (release = r)));
    return () => release();
  };
  const startReleases = new Map<string, () => void>();
  for (const c of opts.holds ?? []) startReleases.set(c, startHold(c));
  const ctx: Ctx = { world, emit: (event) => void emit(ENGINE_EVENT, event) };

  const UNDELAYED = new Set(["launch_info", "ui_settings"]);
  mockIPC(
    async (cmd, payload) => {
      calls.push(cmd);
      // `delay` is a slow engine; the launch reads are dw-app's own, before any engine (U-04 D29).
      if (opts.delayMs && !UNDELAYED.has(cmd)) await new Promise((r) => setTimeout(r, opts.delayMs));
      const cls = failures.get(cmd as CommandName);
      if (cls) throw sampleError(cls);
      const handler = (HANDLERS[cmd as CommandName] ?? PENDING_HANDLERS[cmd]) as ((args: unknown, ctx: Ctx) => unknown) | undefined;
      if (!handler) throw notImplemented(cmd);
      const result = handler(payload ?? {}, ctx);
      const release = held.get(cmd);
      if (release) {
        held.delete(cmd);
        await release;
      }
      return result;
    },
    { shouldMockEvents: true },
  );

  return {
    sentinel: FIXTURE_SENTINEL,
    scenario: opts.scenario,
    world,
    emit: ctx.emit,
    receivePayment: (duffs, walletId = FIXTURE_WALLET_ID, to) => receivePayment(ctx, walletId, duffs, to),
    deliverLinks: (routes) => {
      world.links.push(...routes);
      void emit(SHELL_EVENT, { kind: "linksPending" } satisfies ShellEvent);
    },
    shellCommand: (command) => void emit(SHELL_EVENT, { kind: "command", command } satisfies ShellEvent),
    fail: (command, cls) => (cls ? failures.set(command, cls) : failures.delete(command)),
    hold: startHold,
    release: (command) => void startReleases.get(command)?.(),
    advanceRegistration: (draft, walletId = FIXTURE_WALLET_ID) => registrationRun.advance(ctx, walletId, draft),
    failRegistration: (draft, code, retryable, walletId = FIXTURE_WALLET_ID) => registrationRun.fail(ctx, walletId, draft, code, retryable),
    relaunch: () => {
      sessionStorage.setItem(RELAUNCH_KEY, JSON.stringify(world));
      location.reload();
    },
    calls,
    landPlatformSend: (id) => {
      if (world.pending.platform) landSend(world.pending.platform, id);
    },
    now: FIXTURE_NOW,
    locale: "en-US",
  };
}

/** `?scenario=<name>&theme=light|dark&fail=<command>:<class>&hold=<command>&delay=<ms>`. `fail` and `hold` repeat; an
 * `error-<class>` scenario is `funded` with `list_wallets` failing with that class. Unknown
 * names throw, so a typo in a test URL fails loudly instead of showing the default. */
export function fixtureOptionsFromUrl(search: string): FixtureOptions {
  const p = new URLSearchParams(search);
  const t = p.get("theme");
  const theme: ThemeName | null = t === "light" || t === "dark" ? t : null;
  const failures: Partial<Record<CommandName, ErrorClass>> = {};
  let scenario: ScenarioName = DEFAULT_SCENARIO;
  const name = p.get("scenario");
  if (name?.startsWith("error-")) {
    const cls = name.slice("error-".length);
    if (!isErrorClass(cls)) throw new Error(`unknown error class ${cls}`);
    failures.list_wallets = cls;
  } else if (name !== null) {
    if (!isScenario(name)) throw new Error(`unknown fixture scenario ${name}`);
    scenario = name;
  }
  for (const f of p.getAll("fail")) {
    const [command, cls] = f.split(":");
    if (!Object.hasOwn(COMMANDS, command) || !cls || !isErrorClass(cls)) throw new Error(`bad fail=${f}: expected <command>:<error class>`);
    failures[command as CommandName] = cls;
  }
  const holds: CommandName[] = [];
  for (const c of p.getAll("hold")) {
    if (!Object.hasOwn(COMMANDS, c)) throw new Error(`bad hold=${c}: not a command`);
    holds.push(c as CommandName);
  }
  const delay = Number(p.get("delay") ?? 0);
  return { scenario, theme, failures, holds, delayMs: Number.isFinite(delay) && delay > 0 ? delay : undefined };
}
