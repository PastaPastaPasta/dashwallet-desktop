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
import type { NetworkKind } from "../api/generated/NetworkKind";
import type { UpdateAvailability } from "../api/generated/UpdateAvailability";
import type { UpdatePhase } from "../api/generated/UpdatePhase";
import { NOT_PRIVATE_REASONS, type NotPrivateReason } from "../api/storeOpen";
import { startingPhase, UPDATE_FAULTS, type UpdateFault } from "./updates";
import {
  isErrorClass,
  notPrivateFailure,
  RESTORE_FAILURE_NAMES,
  restoreFailure,
  sampleError,
  STORE_SCREEN_NAMES,
  STORE_SCREENS,
  UPGRADE_NOTICE_NAMES,
  UPGRADE_NOTICES,
  type ErrorClass,
  type StoreScreenName,
  type UpgradeNoticeName,
} from "./errors";
import { landSend, PENDING_HANDLERS, switchedWorld } from "./pending";
import { HANDLERS, registrationRun, type Ctx } from "./handlers";
import { restarted } from "./registration";
import { buildWorld, DEFAULT_SCENARIO, FIXTURE_WALLET_ID, isScenario, type ScenarioName, type World } from "./scenarios";
import { receivePayment, type PaymentSource } from "./send";
import { FIXTURE_NOW } from "./wallet";
import { ui } from "../app/ui";
import { isLanguage } from "../i18n";
import { FIXTURE_DATA_DIR, isLanguageTag } from "./settings";

/** Present in every fixture build and in no production build (scripts/check-bundle.mjs). */
export const FIXTURE_SENTINEL = "dw-fixture-backend:U-07";

export interface FixtureOptions {
  scenario: ScenarioName;
  theme?: ThemeName | null;
  network?: NetworkKind;
  /** Commands that reject with a sample of the error class instead of running. */
  failures?: Partial<Record<CommandName, ErrorClass>>;
  /** Every engine command answers after this many ms (loading states). `launch_info` does not
   * wait: dw-app answers it from its launch options, before the window shows anything. */
  delayMs?: number;
  /** Commands whose first call is held from the page's start (`hold=<command>`): a read that is still
   * out when the screen shows. `release(command)` lets it answer. A relaunch holds them again. */
  holds?: CommandName[];
  /** The saved language (`lang=<tag>`, MP-14): a shipped one or a pseudo-locale (`en-XA` long,
   * `ar-XB` right to left). Applied on every load, a relaunch's included. */
  language?: string;
  /** `savedLang=<tag>`: any well-formed tag as a hand-edited or older settings file may hold it
   * (`EN`, `pt-BR`), shipped or not. Seeds a fresh world only, so a relaunch keeps what was saved. */
  savedLanguage?: string;
  /** This copy's update availability (`updates=<availability>`, PK-03); with `update=<version>`, the
   * version the stable feed serves, `updatePhase=<kind>` the phase it starts in and
   * `updateFault=<fault>` what goes wrong next. */
  updates?: UpdateAvailability;
  updateServes?: string;
  updatePhase?: UpdatePhase["kind"];
  updateFault?: UpdateFault;
  /** `openRefused=<screen>`: `open_network` refuses the wallet data with one of `screen_for`'s screens
   * (a fresh world only). */
  openRefused?: StoreScreenName;
  /** `notPrivate=<reason>`: `open_network` refuses the data root as one others can change (a fresh world only). */
  notPrivate?: NotPrivateReason;
  /** `storageInUse=retry|restart`: `open_network` finds the data still held, once or until a restart
   * (a fresh world only). */
  storageInUse?: "retry" | "restart";
  /** `openRestore=<state>:<cause>`: `open_network` stops at a restore the user asked for (a fresh world
   * only). */
  openRestore?: string;
  /** `openRetry=yes`: `open_network` stops on the machine while it upgrades the store (a fresh world only). */
  openRetry?: boolean;
  /** `upgradeNotice=<name>`: `open_network` carries the store upgrade's notice (a fresh world only). */
  upgradeNotice?: UpgradeNoticeName;
  /** `restoreAvailable=yes|no`: whether a verified snapshot is there to restore (a fresh world only). */
  restoreAvailable?: boolean;
  /** `restored=yes`: the network's wallet data was restored and RESTORED is still set (a fresh world only). */
  restored?: boolean;
}

export interface FixtureHandle {
  readonly sentinel: string;
  readonly scenario: ScenarioName;
  readonly world: World;
  /** Sends an engine event, as dw-app's bridge would. */
  emit(event: AppEvent): void;
  /** A payment of `duffs` arrives at the wallet's current receiving address (the next one is current
   * from then on): its row, `historyChanged` and `newTransactions`, learned live or by the catch-up
   * scan (`source`). Returns the address paid. */
  receivePayment(duffs: number, walletId?: string, to?: string, source?: PaymentSource): string;
  /** Queues links as dw-app's shell routed them and says so (`linksPending`), as a clicked link
   * or a second launch would. */
  deliverLinks(routes: LinkRoute[]): void;
  /** A native menu or tray command. */
  shellCommand(command: ShellCommand): void;
  /** The app's language as Settings ▸ General sets it, with no navigation: what is open stays open. */
  setLanguage(tag: string | null): void;
  /** Makes `command` reject with a sample of `cls` from now on (`null` clears it). */
  fail(command: CommandName, cls: ErrorClass | null): void;
  /** Holds the next call of `command`: it answers from the world as it is when called, a result
   * or an error, but only once the returned function is called (a reply that arrives late). */
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

const UPDATE_AVAILABILITIES: UpdateAvailability[] = ["available", "notInThisBuild", "flatpak", "packageManager", "notInstalled", "moveToApplications"];
const UPDATE_PHASES: UpdatePhase["kind"][] = ["idle", "checking", "upToDate", "found", "downloaded", "downloading", "installing", "installed", "restartRequired"];

function oneOf<T extends string>(p: URLSearchParams, name: string, allowed: readonly T[]): T | undefined {
  const v = p.get(name);
  if (v !== null && !allowed.includes(v as T)) throw new Error(`unknown ${name}=${v}`);
  return (v ?? undefined) as T | undefined;
}

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
  const carried = relaunchedWorld() ?? switchedWorld();
  const world = carried ?? buildWorld(opts.scenario, opts.theme ?? null);
  if (!carried && opts.savedLanguage) world.settings.ui = { ...world.settings.ui, language: opts.savedLanguage };
  if (opts.language) world.settings.ui = { ...world.settings.ui, language: opts.language };
  if (opts.updates) world.updates.availability = opts.updates;
  if (opts.updateServes) world.updates.serves = opts.updateServes;
  if (opts.updatePhase) world.updates.phase = startingPhase(opts.updatePhase, world.updates.serves ?? "0.0.0");
  if (opts.updateFault) world.updates.fault = opts.updateFault;
  if (opts.network) {
    world.launch.network = { kind: opts.network, dirName: opts.network, unitName: opts.network === "mainnet" ? "DASH" : "tDASH" };
    world.pending.networkChoice = { chosen: opts.network === "testnet" ? "mainnet" : "testnet", saved: true };
    world.networkForced = true;
  }
  const dataDir = `${FIXTURE_DATA_DIR}/${world.launch.network.dirName}`;
  if (!carried && opts.openRefused) world.openRefusal = STORE_SCREENS[opts.openRefused](dataDir);
  if (!carried && opts.notPrivate) world.openNotPrivate = notPrivateFailure(opts.notPrivate, FIXTURE_DATA_DIR);
  if (!carried && opts.storageInUse) world.storageHeld = { retryable: opts.storageInUse === "retry" };
  if (!carried && opts.openRestore) world.openRestore = restoreFailure(opts.openRestore, dataDir);
  if (!carried && opts.openRetry) world.openRetry = true;
  if (!carried && opts.upgradeNotice) world.upgradeNotice = UPGRADE_NOTICES[opts.upgradeNotice];
  if (!carried && opts.restoreAvailable !== undefined) world.restoreAvailable = opts.restoreAvailable;
  if (!carried && opts.restored) world.restored = true;
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
  const carry = () => sessionStorage.setItem(RELAUNCH_KEY, JSON.stringify(world));
  const relaunch = () => {
    carry();
    location.reload();
  };
  const ctx: Ctx = { world, emit: (event) => void emit(ENGINE_EVENT, event), relaunch, carry };

  const UNDELAYED = new Set(["launch_info", "ui_settings"]);
  mockIPC(
    async (cmd, payload) => {
      calls.push(cmd);
      // `delay` is a slow engine; the launch reads are dw-app's own, before any engine (U-04 D29).
      if (opts.delayMs && !UNDELAYED.has(cmd)) await new Promise((r) => setTimeout(r, opts.delayMs));
      const cls = failures.get(cmd as CommandName);
      // A held call that is set to fail fails late, as it answers.
      if (cls && !held.has(cmd)) throw sampleError(cls);
      const handler = (HANDLERS[cmd as CommandName] ?? PENDING_HANDLERS[cmd]) as ((args: unknown, ctx: Ctx) => unknown) | undefined;
      if (!handler) throw notImplemented(cmd);
      let answer: { result: unknown } | { error: unknown };
      try {
        if (cls) throw sampleError(cls);
        answer = { result: handler(payload ?? {}, ctx) };
      } catch (error) {
        answer = { error };
      }
      const release = held.get(cmd);
      if (release) {
        held.delete(cmd);
        await release;
      }
      if ("error" in answer) throw answer.error;
      return answer.result;
    },
    { shouldMockEvents: true },
  );

  return {
    sentinel: FIXTURE_SENTINEL,
    scenario: opts.scenario,
    world,
    emit: ctx.emit,
    receivePayment: (duffs, walletId = FIXTURE_WALLET_ID, to, source) => receivePayment(ctx, walletId, duffs, to, source),
    deliverLinks: (routes) => {
      world.links.push(...routes);
      void emit(SHELL_EVENT, { kind: "linksPending" } satisfies ShellEvent);
    },
    shellCommand: (command) => void emit(SHELL_EVENT, { kind: "command", command } satisfies ShellEvent),
    setLanguage: (language) => ui.set({ prefs: { ...ui.get().prefs, language } }),
    fail: (command, cls) => (cls ? failures.set(command, cls) : failures.delete(command)),
    hold: startHold,
    release: (command) => void startReleases.get(command)?.(),
    advanceRegistration: (draft, walletId = FIXTURE_WALLET_ID) => registrationRun.advance(ctx, walletId, draft),
    failRegistration: (draft, code, retryable, walletId = FIXTURE_WALLET_ID) => registrationRun.fail(ctx, walletId, draft, code, retryable),
    relaunch,
    calls,
    landPlatformSend: (id) => {
      if (world.pending.platform) landSend(world.pending.platform, id);
    },
    now: FIXTURE_NOW,
    locale: "en-US",
  };
}

/** `?scenario=<name>&theme=light|dark&network=mainnet|testnet|regtest&lang=<tag>&savedLang=<tag>&updates=<availability>&update=<version>&updatePhase=<kind>&updateFault=<fault>&fail=<command>:<class>&hold=<command>&delay=<ms>&openRefused=<screen>&notPrivate=<reason>&storageInUse=retry|restart&openRestore=<state>:<cause>&restoreAvailable=yes|no&restored=yes`. `fail` and `hold` repeat; an
 * `error-<class>` scenario is `funded` with `list_wallets` failing with that class. Unknown
 * names throw, so a typo in a test URL fails loudly instead of showing the default. */
export function fixtureOptionsFromUrl(search: string): FixtureOptions {
  const p = new URLSearchParams(search);
  const t = p.get("theme");
  const theme: ThemeName | null = t === "light" || t === "dark" ? t : null;
  const n = p.get("network");
  const network: NetworkKind | undefined = n === "mainnet" || n === "testnet" || n === "regtest" ? n : undefined;
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
  const saved = p.get("savedLang");
  if (saved !== null && !isLanguageTag(saved)) throw new Error(`bad savedLang=${saved}: not a language tag`);
  if (saved !== null && p.has("lang")) throw new Error("give lang= or savedLang=, not both");
  const savedLanguage = saved ?? undefined;
  const language = p.get("lang") ?? undefined;
  if (language !== undefined && !isLanguage(language)) throw new Error(`unknown language ${language}`);
  const updates = oneOf(p, "updates", UPDATE_AVAILABILITIES);
  const updateServes = p.get("update") ?? undefined;
  const updatePhase = oneOf(p, "updatePhase", UPDATE_PHASES);
  const updateFault = oneOf(p, "updateFault", UPDATE_FAULTS);
  const delay = Number(p.get("delay") ?? 0);
  const openRefused = oneOf(p, "openRefused", STORE_SCREEN_NAMES);
  const notPrivate = oneOf(p, "notPrivate", NOT_PRIVATE_REASONS);
  const storageInUse = oneOf(p, "storageInUse", ["retry", "restart"] as const);
  const openRestore = oneOf(p, "openRestore", RESTORE_FAILURE_NAMES);
  const available = oneOf(p, "restoreAvailable", ["yes", "no"] as const);
  const openRetry = oneOf(p, "openRetry", ["yes"] as const) === "yes" || undefined;
  const upgradeNotice = oneOf(p, "upgradeNotice", UPGRADE_NOTICE_NAMES);
  const restored = oneOf(p, "restored", ["yes"] as const) === "yes" || undefined;
  return {
    scenario,
    theme,
    network,
    failures,
    holds,
    language,
    savedLanguage,
    updates,
    updateServes,
    updatePhase,
    updateFault,
    delayMs: Number.isFinite(delay) && delay > 0 ? delay : undefined,
    openRefused,
    notPrivate,
    storageInUse,
    openRestore,
    openRetry,
    upgradeNotice,
    restoreAvailable: available === undefined ? undefined : available === "yes",
    restored,
  };

}
