// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
// Settings (U-04): dw-app's display settings file, the wallets' load states, backups, the Dash
// Core imports and exports, dust protection and node information, over the fixture's `World`.
// Files are not on disk: `SettingsWorld.files` is what a typed path names, and a backup or an
// export adds its file there, so it can be inspected and read back. Not modelled: the rotation
// writing automatic backups (a scenario lists them), the contents of an export, and a sealed
// backup's crypto (the passphrase that wrapped it is kept in the clear), and the leases of a
// closed or removed wallet, which the engine revokes.

import type { AppError } from "../api/generated/AppError";
import type { BackupFileHeader } from "../api/generated/BackupFileHeader";
import type { BackupSummary } from "../api/generated/BackupSummary";
import type { CoreExportWarningName } from "../api/generated/CoreExportWarningName";
import type { CoreImportReport } from "../api/generated/CoreImportReport";
import type { ImportOptions } from "../api/generated/ImportOptions";
import type { UiSettings } from "../api/generated/UiSettings";
import type { WalletFileInfo } from "../api/generated/WalletFileInfo";
import type { Ctx, Handlers } from "./handlers";
import { emptyWallet, type FixtureGrant, type FixtureWallet, type World } from "./scenarios";
import { walletIdOf } from "./vault";
import { BASE58, FIXTURE_NOW, secs } from "./wallet";

/** dw-app's `settings.rs` defaults and bounds. */
export const DEFAULT_UI_SETTINGS: UiSettings = {
  theme: null,
  unit: "dash",
  decimalDigits: 2,
  autohideBalance: false,
  autoLockSecs: null,
  subtractFeeByDefault: false,
};
const MAX_AUTO_LOCK_SECS = 86_400;
const MAX_KEEP = 10;
const MAX_DUST = 1_000_000;

/** The fixture's data root and the network's backup directory in it. */
export const FIXTURE_DATA_DIR = "/home/user/.local/share/dashwallet";
export const FIXTURE_BACKUP_DIR = `${FIXTURE_DATA_DIR}/testnet/backups`;
/** Files the Wallets tab's imports can name (D19: a typed path). */
export const FIXTURE_FILES = {
  dumpwallet: "/home/user/Documents/dash-core-dumpwallet.txt",
  walletDat: "/home/user/Documents/core-sqlite-wallet.dat",
  walletDatBdb: "/home/user/Documents/core-berkeley-wallet.dat",
  psbt: "/home/user/Documents/unsigned-payment.psbt",
  unknown: "/home/user/Documents/shopping-list.txt",
  mainnetDump: "/home/user/Documents/mainnet-dumpwallet.txt",
};
/** The passphrase of FIXTURE_FILES.walletDat (Dash Core's `encryptwallet`). */
export const CORE_WALLET_PASSPHRASE = "core-wallet-pass";

/** A file a path names: what `inspect_wallet_file` detects, and what reading it gives. */
export interface FixtureFile {
  info: WalletFileInfo;
  /** A backup's or an encrypted wallet.dat's passphrase. */
  passphrase?: string;
  /** A backup's wallets and their phrases (`null`: watch-only). */
  wallets?: { walletId: string; name: string; phrase: string | null }[];
  automatic?: boolean;
  /** What a Dash Core import reports for it. */
  report?: Pick<CoreImportReport, "labelsImported" | "keysNotImported" | "scriptsNotImported">;
}

export interface SettingsWorld {
  /** `<data root>/settings.json` (U-04 D2). */
  ui: UiSettings;
  /** Registered wallets `unload_wallet` closed: still in `World.wallets`, but every
   * wallet-scoped command answers `wallet_not_found` for them (`isClosed`). */
  closed: string[];
  /** Wallets left out of the load-on-startup list. */
  notAtStartup: string[];
  backups: BackupSummary[];
  keep: number;
  /** Dust protection in duffs; null: off (QT-075). */
  dust: number | null;
  files: Record<string, FixtureFile>;
  /** Wallets whose phrase Dash Core cannot rebuild (`core_mnemonic_compatibility`). */
  coreIncompatible: string[];
}

const dumpInfo = (mainnet: boolean): WalletFileInfo => ({
  kind: "dumpWallet",
  network: mainnet ? { kind: "mainnet", dirName: "mainnet", unitName: "DASH" } : { kind: "testnet", dirName: "testnet", unitName: "tDASH" },
  hasMnemonic: true,
  hasHdSeed: true,
  hasXprv: false,
  looseKeyCount: 2,
  scriptCount: 0,
  labelCount: 3,
});

export function newSettingsWorld(): SettingsWorld {
  return {
    ui: { ...DEFAULT_UI_SETTINGS },
    closed: [],
    notAtStartup: [],
    backups: [],
    keep: 5,
    dust: 100,
    coreIncompatible: [],
    files: {
      [FIXTURE_FILES.dumpwallet]: { info: dumpInfo(false), report: { labelsImported: 3, keysNotImported: 2, scriptsNotImported: 0 } },
      [FIXTURE_FILES.mainnetDump]: { info: dumpInfo(true) },
      [FIXTURE_FILES.walletDat]: {
        info: { kind: "walletDatSqlite", encrypted: true, hasMnemonic: true },
        passphrase: CORE_WALLET_PASSPHRASE,
        report: { labelsImported: 1, keysNotImported: 0, scriptsNotImported: 1 },
      },
      [FIXTURE_FILES.walletDatBdb]: { info: { kind: "walletDatBdb", encrypted: false } },
      [FIXTURE_FILES.psbt]: { info: { kind: "psbt" } },
      [FIXTURE_FILES.unknown]: { info: { kind: "unknown" } },
    },
  };
}

/** Two automatic backups of the first wallet, newest first (Backups tab). */
export function withAutomaticBackups(s: SettingsWorld, walletId: string, phrase: string, passphrase: string): SettingsWorld {
  for (const [i, minutes] of [0, 1440].entries()) {
    const createdAt = secs(FIXTURE_NOW) - 3600 - minutes * 60;
    const path = `${FIXTURE_BACKUP_DIR}/Main-auto-${i + 1}.dwbackup`;
    s.backups.push({ path, walletId, createdAt, sizeBytes: 3_187 + i, automatic: true });
    s.files[path] = backupFile([{ walletId, name: "Main", phrase }], passphrase, createdAt, true);
  }
  return s;
}

const fail = (code: AppError["code"], message: string, data: AppError["data"] = null): AppError => ({ code, message, data });

function backupFile(wallets: NonNullable<FixtureFile["wallets"]>, passphrase: string, createdAt: number, automatic: boolean): FixtureFile {
  return {
    info: { kind: "dwBackup", network: { kind: "testnet", dirName: "testnet", unitName: "tDASH" }, walletCount: wallets.length, createdAt, formatVersion: 1 },
    passphrase,
    wallets,
    automatic,
  };
}

/** 1-64 characters after trimming, as `rename_wallet` and the imports check a name. */
function checkName(name: string | null): string | null {
  if (name === null) return null;
  const t = name.trim();
  const chars = [...t].length; // code points, as dw-app counts them
  if (chars < 1 || chars > 64) throw fail("invalid_argument", "a wallet name is 1 to 64 characters");
  return t;
}

/** A key the engine would derive: base58 digits drawn from `seed` (not a real key). */
function fakeKey(prefix: string, seed: string): string {
  let h = 0x2f6b9d1;
  return (
    prefix +
    Array.from({ length: 107 }, (_, i) => {
      h = Math.imul(h ^ seed.charCodeAt(i % seed.length), 0x01000193) >>> 0;
      return BASE58[h % 58];
    }).join("")
  );
}

export interface SettingsDeps {
  wallet(w: World, walletId: string): FixtureWallet;
  /** An unused grant of the wallet, of one of `kinds`, its key usable. */
  grant(w: World, id: string, walletId: string, kinds: ("wipe" | "revealSecret")[]): FixtureGrant | null;
  useGrant(w: World, id: string): void;
}

export function settingsHandlers({ wallet, grant, useGrant }: SettingsDeps): Handlers {
  const network = (w: World) => w.launch.network.dirName;
  /** No data key: what the vault answers a call that needs it. */
  const needKey = (w: World) => {
    if (w.lock.state === "noVault") throw fail("vault.no_vault", "no vault");
    if (w.lock.state === "locked") throw fail("vault.locked", "vault locked");
    if (w.lock.state === "unlockedMixingOnly") throw fail("vault.mixing_only", "vault unlocked for mixing only");
  };
  /** Registers a wallet an import or a restore made, its secrets in the vault. */
  const register = (ctx: Ctx, walletId: string, name: string | null, phrase: string | null) => {
    const { world } = ctx;
    if (world.wallets.some((x) => x.summary.walletId === walletId)) throw fail("wallet_already_exists", "wallet already exists");
    const w = emptyWallet(walletId, name ?? `Wallet ${world.wallets.length + 1}`);
    if (phrase === null) w.summary = { ...w.summary, watchOnly: true, hasMnemonic: false };
    else {
      world.phrases[walletId] = phrase;
      world.lock = { ...world.lock, walletsWithSecrets: [...new Set([...world.lock.walletsWithSecrets, walletId])].sort() };
    }
    world.wallets.push(w);
    ctx.emit({ kind: "walletCreated", network: network(world), walletId });
    return walletId;
  };
  const file = (w: World, path: string, code: AppError["code"]) => {
    const f = w.settings.files[path];
    if (!f) throw fail(code, `${path}: no such file`);
    return f;
  };
  /** A Dash Core import of `path`: the wallet it registers, with a phrase only the fixture knows. */
  const coreImport = (ctx: Ctx, path: string, options: ImportOptions, f: FixtureFile | null): CoreImportReport => {
    needKey(ctx.world);
    const name = checkName(options.name);
    const walletId = register(ctx, walletIdOf(`core-import ${path}`), name, `core import of ${path}`);
    return { walletId, labelsImported: 0, keysNotImported: 0, scriptsNotImported: 0, ...f?.report, coreCompatSeed: true };
  };
  const warnings = (w: World, walletId: string): CoreExportWarningName[] =>
    w.settings.coreIncompatible.includes(walletId) ? ["mnemonicNotCoreCompatible"] : [];

  return {
    ui_settings: (_, { world }) => world.settings.ui,
    set_ui_settings: ({ settings }, { world }) => {
      if (settings.decimalDigits < 2 || settings.decimalDigits > 8) throw fail("invalid_argument", "decimal digits must be 2 to 8");
      if (settings.autoLockSecs !== null && (settings.autoLockSecs < 1 || settings.autoLockSecs > MAX_AUTO_LOCK_SECS))
        throw fail("invalid_argument", "auto lock must be 1 s to 24 hours");
      world.settings.ui = { ...settings };
      return world.settings.ui;
    },

    // Wallets (QT-101, IOS-110)
    wallet_load_states: (_, { world }) =>
      world.wallets.map(({ summary: s }) => ({
        walletId: s.walletId,
        name: s.name,
        loaded: !world.settings.closed.includes(s.walletId),
        loadOnStartup: !world.settings.notAtStartup.includes(s.walletId),
        watchOnly: s.watchOnly,
      })),
    load_wallet: ({ walletId }, ctx) => {
      const s = ctx.world.settings;
      if (!ctx.world.wallets.some((x) => x.summary.walletId === walletId)) throw fail("wallet_not_found", `wallet not found: ${walletId}`);
      if (s.closed.includes(walletId)) {
        s.closed = s.closed.filter((id) => id !== walletId);
        ctx.emit({ kind: "walletLoadChanged", network: network(ctx.world), walletId, loaded: true });
      }
      return null;
    },
    unload_wallet: ({ walletId }, ctx) => {
      const s = ctx.world.settings;
      if (s.closed.includes(walletId)) return null;
      wallet(ctx.world, walletId);
      s.closed = [...s.closed, walletId];
      ctx.emit({ kind: "walletLoadChanged", network: network(ctx.world), walletId, loaded: false });
      return null;
    },
    set_load_on_startup: ({ walletId, loadOnStartup }, { world }) => {
      if (!world.wallets.some((x) => x.summary.walletId === walletId)) throw fail("wallet_not_found", `wallet not found: ${walletId}`);
      const rest = world.settings.notAtStartup.filter((id) => id !== walletId);
      world.settings.notAtStartup = loadOnStartup ? rest : [...rest, walletId];
      return null;
    },
    rename_wallet: ({ walletId, name }, { world }) => {
      const w = wallet(world, walletId);
      w.summary = { ...w.summary, name: checkName(name)! };
      return null;
    },
    remove_wallet: ({ walletId, grantId }, ctx) => {
      const { world } = ctx;
      wallet(world, walletId);
      if (!grant(world, grantId, walletId, ["wipe"])) throw fail("vault.grant_invalid", "grant invalid");
      useGrant(world, grantId);
      world.wallets = world.wallets.filter((x) => x.summary.walletId !== walletId);
      delete world.phrases[walletId];
      world.lock = { ...world.lock, walletsWithSecrets: world.lock.walletsWithSecrets.filter((id) => id !== walletId) };
      world.settings.closed = world.settings.closed.filter((id) => id !== walletId);
      world.settings.notAtStartup = world.settings.notAtStartup.filter((id) => id !== walletId);
      ctx.emit({ kind: "walletRemoved", network: network(world), walletId });
      return null;
    },
    import_watch_only: ({ xpub, options }, ctx) => {
      const x = xpub.trim();
      if (!/^tpub[1-9A-HJ-NP-Za-km-z]{100,}$/.test(x)) throw fail("invalid_xpub", "not a testnet account xpub");
      if (options.lookahead !== null && (options.lookahead < 1 || options.lookahead > 1000)) throw fail("invalid_argument", "lookahead must be 1 to 1000");
      return register(ctx, walletIdOf(`xpub ${x}`), checkName(options.name), null);
    },
    account_xpub: ({ walletId, account }, { world }) => {
      wallet(world, walletId);
      return { account, derivationPath: `m/44'/1'/${account}'`, xpub: fakeKey("tpub", `${walletId}/${account}`) };
    },

    // Backups (QT-110, QT-116)
    backup_wallet: ({ walletId, destPath, backupPassphrase }, { world }) => {
      const w = wallet(world, walletId);
      if (w.summary.watchOnly) throw fail("not_implemented", "a watch-only wallet has nothing to back up");
      let passphrase: string;
      if (world.lock.encrypted) {
        if (backupPassphrase !== null) throw fail("invalid_argument", "an encrypted vault's backup uses its passphrase");
        if (world.lock.state !== "unlocked") throw fail("backup.vault_locked", "the vault is locked");
        passphrase = world.passphrase;
      } else {
        if (backupPassphrase === null) throw fail("backup.passphrase_required", "an unencrypted vault's backup needs a passphrase");
        if (backupPassphrase === "") throw fail("invalid_argument", "the backup passphrase is empty");
        passphrase = backupPassphrase;
      }
      if (world.settings.files[destPath]) throw fail("backup.destination_unwritable", `${destPath} exists`);
      const createdAt = secs(FIXTURE_NOW);
      world.settings.files[destPath] = backupFile([{ walletId, name: w.summary.name, phrase: world.phrases[walletId] ?? null }], passphrase, createdAt, false);
      return { path: destPath, walletId, createdAt, sizeBytes: 3_204, automatic: false };
    },
    restore_backup: ({ path, passphrase }, ctx) => {
      const { world } = ctx;
      const f = file(world, path, "io");
      if (f.info.kind !== "dwBackup" || !f.wallets) throw fail("backup.corrupt", "not a backup");
      if (f.info.network?.dirName !== network(world)) throw fail("backup.network_mismatch", "a backup of another network");
      needKey(world);
      if (passphrase === null) throw fail("backup.passphrase_required", "the backup needs its passphrase");
      if (passphrase !== f.passphrase) throw fail("backup.wrong_passphrase", "wrong backup passphrase");
      const known = f.wallets.find((b) => world.wallets.some((x) => x.summary.walletId === b.walletId));
      if (known) throw fail("backup.already_exists", "the wallet already exists", { kind: "wallet", walletId: known.walletId });
      return f.wallets.map((b) => register(ctx, b.walletId, b.name, b.phrase));
    },
    automatic_backups: ({ walletId }, { world }) =>
      world.settings.backups.filter((b) => b.automatic && (walletId === null || b.walletId === walletId)).toSorted((a, b) => b.createdAt - a.createdAt),
    backup_directory: () => FIXTURE_BACKUP_DIR,
    backup_policy: (_, { world }) => ({ keep: world.settings.keep, directory: FIXTURE_BACKUP_DIR }),
    set_backup_policy: ({ keep }, { world }) => {
      if (!Number.isInteger(keep) || keep < 0 || keep > MAX_KEEP) throw fail("invalid_argument", "keep 0 to 10 backups");
      const s = world.settings;
      s.keep = keep;
      // Deletes the oldest automatic backups of each wallet beyond `keep`.
      const kept = new Map<string, number>();
      s.backups = s.backups
        .toSorted((a, b) => b.createdAt - a.createdAt)
        .filter((b) => {
          if (!b.automatic) return true;
          const n = (kept.get(b.walletId) ?? 0) + 1;
          kept.set(b.walletId, n);
          if (n <= keep) return true;
          delete s.files[b.path];
          return false;
        });
      return { keep, directory: FIXTURE_BACKUP_DIR };
    },
    read_backup_header: ({ path }, { world }) => {
      const f = file(world, path, "io");
      if (f.info.kind !== "dwBackup" || !f.wallets) throw fail("backup.corrupt", "not a backup");
      const header: BackupFileHeader = {
        format: "dwbackup",
        formatVersion: f.info.formatVersion,
        network: f.info.network?.dirName ?? "unknown",
        createdAt: f.info.createdAt,
        walletIds: f.wallets.map((b) => b.walletId),
        automatic: f.automatic ?? false,
        appVersion: world.launch.appVersion,
      };
      return header;
    },

    // Dash Core imports and exports (QT-106 to QT-109)
    inspect_wallet_file: ({ path }, { world }) => file(world, path, "compat.file_unreadable").info,
    import_dump_wallet: ({ path, options }, ctx) => {
      const f = file(ctx.world, path, "compat.file_unreadable");
      if (f.info.kind !== "dumpWallet") throw fail("compat.unsupported_format", "not a dumpwallet file");
      if (f.info.network && f.info.network.dirName !== network(ctx.world))
        throw fail("compat.network_mismatch", "keys of another network", { kind: "keysNetwork", mainnet: f.info.network.kind === "mainnet" });
      if (!f.info.hasMnemonic && !f.info.hasHdSeed && !f.info.hasXprv) throw fail("compat.no_hd_chain", "no HD chain");
      return coreImport(ctx, path, options, f);
    },
    import_wallet_dat: ({ path, walletPassphrase, options }, ctx) => {
      const f = file(ctx.world, path, "compat.file_unreadable");
      if (f.info.kind === "walletDatBdb") throw fail("not_implemented", "Berkeley DB wallets cannot be read");
      if (f.info.kind !== "walletDatSqlite") throw fail("compat.unsupported_format", "not a wallet.dat");
      if (f.info.encrypted) {
        if (walletPassphrase === null) throw fail("compat.passphrase_required", "the wallet.dat is encrypted");
        if (walletPassphrase !== f.passphrase) throw fail("compat.wrong_passphrase", "wrong wallet.dat passphrase");
      }
      return coreImport(ctx, path, options, f);
    },
    import_key_material: ({ material, options }, ctx) => {
      const ok =
        material.kind === "hdSeed"
          ? /^([0-9a-f]{2}){16,64}$/i.test(material.seedHex.trim())
          : material.kind === "xprv"
            ? /^tprv[1-9A-HJ-NP-Za-km-z]{100,}$/.test(material.xprv.trim())
            : (() => {
                try {
                  const v: unknown = JSON.parse(material.json);
                  return Array.isArray(v) || (typeof v === "object" && v !== null && Array.isArray((v as { descriptors?: unknown }).descriptors));
                } catch {
                  return false;
                }
              })();
      if (!ok) throw fail("compat.invalid_key_material", `not a valid ${material.kind}`);
      const seed = material.kind === "hdSeed" ? material.seedHex : material.kind === "xprv" ? material.xprv : material.json;
      return coreImport(ctx, `${material.kind} ${seed.trim().toLowerCase()}`, options, null);
    },
    core_mnemonic_compatibility: ({ walletId }, { world }) => {
      const w = wallet(world, walletId);
      if (w.summary.watchOnly) throw fail("compat.watch_only", "a watch-only wallet has no phrase");
      needKey(world);
      const found = warnings(world, walletId);
      return { coreCompatible: found.length === 0, warnings: found };
    },
    export_for_core: ({ walletId, format, destPath, grantId }, { world }) => {
      const w = wallet(world, walletId);
      if (w.summary.watchOnly) throw fail("compat.watch_only", "a watch-only wallet has no keys to export");
      if (world.settings.files[destPath]) throw fail("compat.destination_unwritable", `${destPath} exists`);
      if (!grant(world, grantId, walletId, ["revealSecret"])) throw fail("vault.grant_invalid", "grant invalid");
      useGrant(world, grantId);
      world.settings.files[destPath] = format === "dumpWallet" ? { info: dumpInfo(false) } : { info: { kind: "unknown" } };
      return { path: destPath, format, keyCount: 62, warnings: warnings(world, walletId) };
    },

    // Options (QT-075, QT-138)
    dust_protection: (_, { world }) => world.settings.dust,
    set_dust_protection: ({ threshold }, { world }) => {
      if (threshold !== null && (!Number.isInteger(threshold) || threshold < 1 || threshold > MAX_DUST))
        throw fail("invalid_argument", "the dust threshold must be 1 to 1,000,000 duffs");
      world.settings.dust = threshold;
      return null;
    },
    node_info: (_, { world }) => ({
      userAgent: `/DashWallet:${world.launch.appVersion}/`,
      dataDir: FIXTURE_DATA_DIR,
      startupTime: secs(FIXTURE_NOW) - 5400,
      network: world.launch.network,
      connectionsIn: 0,
      connectionsOut: 8,
      localAddresses: [],
      tipHeight: world.sync.tipHeight,
      tipTime: world.sync.tipTime,
      tipHash: null,
      bestChainlock: null,
      masternodes: null,
      evonodes: null,
      mempoolTxCount: null,
      mempoolUsageBytes: null,
    }),
  };
}

/** A closed wallet answers `wallet_not_found` to every wallet-scoped call (QT-101). */
export const isClosed = (w: World, walletId: string) => w.settings.closed.includes(walletId);
