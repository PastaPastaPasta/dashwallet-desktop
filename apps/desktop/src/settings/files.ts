// Settings ▸ Backups (QT-106…110, QT-116): backups, the Dash Core imports and the export for
// Dash Core, without a file chooser (D19): what is written goes into the network's backup folder
// under a generated name, what is read comes from a typed path. Import a wallet file goes on by
// what the file is (D21). Passphrases and key material stay in the call that uses them.

import { hasCode, type Backend } from "../api/backend";
import type { BackupSummary } from "../api/generated/BackupSummary";
import type { CoreExportFormatName } from "../api/generated/CoreExportFormatName";
import type { CoreExportReport } from "../api/generated/CoreExportReport";
import type { CoreExportWarningName } from "../api/generated/CoreExportWarningName";
import type { CoreImportReport } from "../api/generated/CoreImportReport";
import type { ImportOptions } from "../api/generated/ImportOptions";
import type { KeyMaterialInput } from "../api/generated/KeyMaterialInput";
import type { WalletFileInfo } from "../api/generated/WalletFileInfo";
import { LockModel } from "../onboarding/lock";
import { failureText, Model } from "../onboarding/model";
import { passphraseProblem } from "../onboarding/passphrase";
import { t } from "../i18n";
import { vaultCredential } from "./security";
import { optionalName, WalletsText } from "./wallets";

export const FilesText = {
  backedUp: (path: string) => `The backup was written to ${path}.`,
  backupPassphraseHint: "This wallet has no passphrase: choose one to protect the backup. You need it to restore the backup.",
  backupUnlocked: "The backup is protected with your wallet passphrase.",
  /** QT-078. */
  psbtFile: "This is a partially signed transaction. Open it in Tools ▸ PSBT.",
  unknownFile: "DashPay Desktop does not recognise this file. It reads DashPay Desktop backups, dumpwallet files and wallet.dat files.",
  otherNetwork: (name: string) => `This file is for ${name}, not for the network this window uses.`,
  emptyBackup: "This backup holds no wallet.",
  restored: (n: number) => (n === 1 ? "The wallet was restored." : `${n} wallets were restored.`),
  /** QT-107. */
  imported: (labels: number) => (labels === 1 ? "The wallet was imported with 1 label." : `The wallet was imported with ${labels} labels.`),
  keysNotImported: (n: number) =>
    n === 1
      ? "1 key outside the HD wallet was not imported. Move its coins to this wallet from Dash Core."
      : `${n} keys outside the HD wallet were not imported. Move their coins to this wallet from Dash Core.`,
  scriptsNotImported: (n: number) => (n === 1 ? "1 script was not imported." : `${n} scripts were not imported.`),
  /** QT-108. */
  invalidSeedHex: "An HD seed is 32 to 128 hexadecimal digits.",
  invalidXprv: "This is not an extended private key (tprv or xprv) of this network.",
  invalidDescriptors: "Paste the whole output of listdescriptors true.",
  /** QT-109. */
  exported: (keys: number, path: string) => `${keys} keys were written to ${path}. Keep this file safe: it holds your private keys.`,
  exportWarning: "This file holds your private keys. Anyone who gets it can take your Dash.",
  /** D30: a lower count deletes files at once. */
  keepTitle: "Delete automatic backups?",
  /** Only what the engine's prune guarantees (D46): each wallet keeps its newest `keep` as they
   * are when it prunes. A backup written since the list was read counts among those, so a listed
   * one can go that the list says stays: `shown`, the count by the list, is an estimate. */
  keepDeletes: (keep: number, shown: number | null, dir: string) => {
    const what =
      keep === 0
        ? `Turning automatic backups off deletes every automatic backup in ${dir} now, and no new ones are made.`
        : `Keeping ${keep} for each wallet deletes all but each wallet's newest ${keep === 1 ? "automatic backup" : `${keep} automatic backups`} in ${dir} now. Which ones go is decided then: a backup written since this list was read counts among the newest, so backups shown here can go too.`;
    return shown === null ? what : `${what} The list shown counts ${shown === 0 ? "none" : shown} of them; there can be more.`;
  },
  keepConfirm: "Delete backups",
  warnings: {
    mnemonicNotCoreCompatible:
      "Dash Core cannot rebuild this wallet from its recovery phrase. The exported keys still work, but keep this wallet's phrase for DashPay Desktop.",
    coinJoinAccountNotScannedByLegacyCore: "Dash Core before v20 does not scan the CoinJoin account: mixed coins may not show there.",
  } satisfies Record<CoreExportWarningName, string>,
};

// ---------------------------------------------------------------------------- file names (D19)

const pad = (n: number) => String(n).padStart(2, "0");

/** Letters, digits, `-` and `_`; anything else becomes `-`. "wallet" when nothing is left. */
export function fileStem(name: string): string {
  const stem = name
    .normalize("NFKD")
    .replace(/\p{M}/gu, "")
    .replace(/[^A-Za-z0-9_-]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return stem || "wallet";
}

export const SUFFIX = { backup: ".dwbackup", dumpWallet: "-dumpwallet.txt", importDescriptorsJson: "-descriptors.json" } as const;

/** `<name>-YYYY-MM-DD-HH-MM-SS<suffix>` in local time. */
export function fileName(walletName: string, at: Date, kind: keyof typeof SUFFIX): string {
  const time = `${at.getFullYear()}-${pad(at.getMonth() + 1)}-${pad(at.getDate())}-${pad(at.getHours())}-${pad(at.getMinutes())}-${pad(at.getSeconds())}`;
  return `${fileStem(walletName)}-${time}${SUFFIX[kind]}`;
}

/** `dir` and `file` with the directory's own separator (a Windows folder has `\`). */
export function inDirectory(dir: string, file: string): string {
  const sep = dir.includes("\\") && !dir.includes("/") ? "\\" : "/";
  return dir.endsWith(sep) ? dir + file : dir + sep + file;
}

// ------------------------------------------------------------------------------- back up (QT-110)

type BackupApi = Pick<Backend, "backupWallet" | "backupDirectory">;

/** Back up a wallet: on an encrypted vault the file takes its passphrase, on an unencrypted one a
 * passphrase chosen here, typed twice. */
export class BackupModel extends Model {
  working = false;
  error: string | null = null;
  done: BackupSummary | null = null;

  constructor(
    private readonly api: BackupApi,
    private readonly now: () => Date,
  ) {
    super();
  }

  edited() {
    this.error = null;
    this.changed();
  }

  /** `passphrase` and `confirmation` are `null` on an encrypted vault. */
  async backup(walletId: string, name: string, passphrase: string | null, confirmation: string | null) {
    if (this.working) return;
    if (passphrase !== null) {
      const problem = passphraseProblem(passphrase, confirmation ?? "");
      if (problem) return this.#fail(problem);
    }
    this.working = true;
    this.error = null;
    this.done = null;
    this.changed();
    try {
      const dest = inDirectory(await this.api.backupDirectory(), fileName(name, this.now(), "backup"));
      this.done = await this.api.backupWallet(walletId, dest, passphrase);
    } catch (e) {
      this.error = failureText(e);
    }
    this.working = false;
    this.changed();
  }

  #fail(message: string) {
    this.error = message;
    this.changed();
  }
}

// ---------------------------------------------------------------------- import a wallet file (D21)

export type ImportStep = "path" | "inspecting" | "details" | "working" | "done";

/** What a detected file asks for before its import, or why it cannot be imported. */
export interface ImportPlan {
  kind: WalletFileInfo["kind"];
  /** Why this file stops here (a PSBT, unknown, another network). */
  refusal: string | null;
  /** The passphrase field's label, when the file needs one. */
  passphrase: string | null;
  /** The import takes a wallet name (a restore keeps the backup's names). */
  named: boolean;
  /** One line about the file. */
  summary: string;
}

export function importPlan(info: WalletFileInfo, network: string): ImportPlan {
  const base = { kind: info.kind, refusal: null, passphrase: null, named: false };
  const other = (n: { dirName: string } | null) => (n !== null && n.dirName !== network ? FilesText.otherNetwork(n.dirName) : null);
  // A dumpwallet's keys tell only mainnet from the test networks (dw-engine compat.rs `check_key_network`):
  // the engine reports every test-family file as testnet, which regtest and devnet import too.
  const otherFamily = (n: { dirName: string } | null) => (n !== null && (n.dirName === "mainnet") !== (network === "mainnet") ? FilesText.otherNetwork(n.dirName) : null);
  switch (info.kind) {
    case "dwBackup":
      return {
        ...base,
        refusal:
          info.network === null
            ? FilesText.otherNetwork("a network this version does not know")
            : (other(info.network) ?? (info.walletCount === 0 ? FilesText.emptyBackup : null)),
        passphrase: "Backup passphrase",
        summary: info.walletCount === 1 ? "A DashPay Desktop backup of 1 wallet." : `A DashPay Desktop backup of ${info.walletCount} wallets.`,
      };
    case "dumpWallet":
      return { ...base, refusal: otherFamily(info.network), named: true, summary: "A Dash Core dumpwallet file." };
    case "walletDatSqlite":
    case "walletDatBdb": {
      const encrypted = info.kind === "walletDatSqlite" ? info.encrypted : info.encrypted === true;
      return {
        ...base,
        passphrase: encrypted ? "wallet.dat passphrase" : null,
        named: true,
        summary: encrypted ? "An encrypted Dash Core wallet.dat." : "A Dash Core wallet.dat.",
      };
    }
    case "psbt":
      return { ...base, refusal: FilesText.psbtFile, summary: "A partially signed transaction." };
    case "unknown":
      return { ...base, refusal: FilesText.unknownFile, summary: "An unknown file." };
  }
}

/** What an import or a restore did, as the sheet reports it. */
export interface ImportDone {
  walletIds: string[];
  lines: string[];
  /** A Dash Core import's report: what it left out. */
  report: CoreImportReport | null;
}

export function importLines(report: CoreImportReport): string[] {
  const lines = [FilesText.imported(report.labelsImported)];
  if (report.keysNotImported > 0) lines.push(FilesText.keysNotImported(report.keysNotImported));
  if (report.scriptsNotImported > 0) lines.push(FilesText.scriptsNotImported(report.scriptsNotImported));
  return lines;
}

export type ImportApi = Pick<Backend, "inspectWalletFile" | "restoreBackup" | "importDumpWallet" | "importWalletDat">;

const options = (name: string | null): ImportOptions => ({ name, birthHeight: null, coreCompat: false, lookahead: null });

export class ImportFileModel extends Model {
  step: ImportStep = "path";
  path = "";
  plan: ImportPlan | null = null;
  /** What `inspect` detected. */
  info: WalletFileInfo | null = null;
  error: string | null = null;
  /** The last run's refusal as the engine gave it (its code), `null` after a success. */
  failure: unknown = null;
  done: ImportDone | null = null;

  constructor(
    private readonly api: ImportApi,
    private readonly network: string,
  ) {
    super();
  }

  /** Bumped whenever the selection is dropped (Back, reset): a reply for an older one is ignored. */
  generation = 0;

  /** A new path is being chosen: the last one's problem no longer applies. */
  clearError() {
    if (this.step !== "path" || this.error === null) return;
    this.error = null;
    this.changed();
  }

  async inspect(path: string) {
    if (this.step !== "path") return;
    const generation = this.generation;
    const p = path.trim();
    if (!p) return this.#fail("Enter the file's full path.");
    this.path = p;
    this.error = null;
    this.#go("inspecting");
    try {
      const info = await this.api.inspectWalletFile(p);
      if (generation !== this.generation) return;
      this.info = info;
      this.plan = importPlan(info, this.network);
      this.#go("details");
    } catch (e) {
      if (generation !== this.generation) return;
      this.error = failureText(e);
      this.#go("path");
    }
  }

  back() {
    this.generation++;
    this.plan = null;
    this.info = null;
    this.error = null;
    this.#go("path");
  }

  /** Why `run` would refuse these inputs before reading the file, or `null`. */
  problem(passphrase: string | null, nameText: string): string | null {
    const plan = this.plan;
    if (!plan || plan.refusal) return plan?.refusal ?? null;
    if (plan.passphrase && !passphrase) return `Enter the ${plan.passphrase.toLowerCase()}.`;
    if (plan.named && optionalName(nameText) === undefined) return WalletsText.nameRule;
    return null;
  }

  /** A field changed: the last refusal no longer applies. */
  edited() {
    if (this.error === null) return;
    this.error = null;
    this.changed();
  }

  /** Shows a refusal found before `run` (onboarding checks first, then makes the vault). */
  refuse(message: string) {
    this.#fail(message);
  }

  /** Back to an empty path, as when the sheet opens. */
  reset() {
    this.generation++;
    this.path = "";
    this.plan = null;
    this.info = null;
    this.error = null;
    this.failure = null;
    this.done = null;
    this.#go("path");
  }

  /** Imports the inspected file; `passphrase` when the plan asks for one, else `null`. */
  async run(passphrase: string | null, nameText: string) {
    const plan = this.plan;
    if (this.step !== "details" || !plan || plan.refusal) return;
    const problem = this.problem(passphrase, nameText);
    if (problem) return this.#fail(problem);
    const name = plan.named ? (optionalName(nameText) ?? null) : null;
    this.error = null;
    this.failure = null;
    this.#go("working");
    try {
      if (plan.kind === "dwBackup") {
        const ids = await this.api.restoreBackup(this.path, passphrase);
        this.done = { walletIds: ids, lines: [FilesText.restored(ids.length)], report: null };
      } else {
        const report =
          plan.kind === "dumpWallet"
            ? await this.api.importDumpWallet(this.path, options(name))
            : await this.api.importWalletDat(this.path, plan.passphrase ? passphrase : null, options(name));
        this.done = { walletIds: [report.walletId], lines: importLines(report), report };
      }
      this.#go("done");
    } catch (e) {
      this.failure = e;
      this.error = failureText(e);
      this.#go("details");
    }
  }

  #go(step: ImportStep) {
    this.step = step;
    this.changed();
  }

  #fail(message: string) {
    this.error = message;
    this.changed();
  }
}

// --------------------------------------------------------------------------- import keys (QT-108)

export type KeyKind = KeyMaterialInput["kind"];

export const KEY_KINDS: { kind: KeyKind; label: string }[] = [
  { kind: "hdSeed", label: "HD seed (hex)" },
  { kind: "xprv", label: "Extended private key" },
  { kind: "descriptors", label: "listdescriptors true" },
];

/** The material, or why it is not one; checked before it reaches the engine. */
export function keyMaterial(kind: KeyKind, text: string): KeyMaterialInput | string {
  const t = text.trim();
  switch (kind) {
    case "hdSeed":
      return /^([0-9a-fA-F]{2}){16,64}$/.test(t) ? { kind, seedHex: t } : FilesText.invalidSeedHex;
    case "xprv":
      return /^[tx]prv[1-9A-HJ-NP-Za-km-z]+$/.test(t) ? { kind, xprv: t } : FilesText.invalidXprv;
    case "descriptors":
      try {
        JSON.parse(t);
        return { kind, json: t };
      } catch {
        return FilesText.invalidDescriptors;
      }
  }
}

export class ImportKeysModel extends Model {
  working = false;
  error: string | null = null;
  done: ImportDone | null = null;

  constructor(private readonly api: Pick<Backend, "importKeyMaterial">) {
    super();
  }

  async run(kind: KeyKind, text: string, nameText: string) {
    if (this.working) return;
    const material = keyMaterial(kind, text);
    if (typeof material === "string") return this.#fail(material);
    const name = optionalName(nameText);
    if (name === undefined) return this.#fail(WalletsText.nameRule);
    this.working = true;
    this.error = null;
    this.changed();
    try {
      const report = await this.api.importKeyMaterial(material, options(name));
      this.done = { walletIds: [report.walletId], lines: importLines(report), report };
    } catch (e) {
      this.error = failureText(e);
    }
    this.working = false;
    this.changed();
  }

  #fail(message: string) {
    this.error = message;
    this.changed();
  }
}

// ------------------------------------------------------------- backups to keep (QT-110, D30)

/** Whether choosing `keep` asks first: any count lower than `current` may delete files at once,
 * whatever the list shows. The engine writes automatic backups in the background without telling
 * the renderer, so a list that shows nothing to delete may be out of date (D42). */
export function keepAsks(current: number, keep: number): boolean {
  return keep < current;
}

/** How many of `backups` (newest first, as `automaticBackups` lists them) `keep` deletes: the
 * engine keeps each wallet's newest `keep` (`set_backup_policy` prunes). `null`: no list. */
export function deletedByKeep(backups: BackupSummary[] | undefined, keep: number): number | null {
  if (!backups) return null;
  const seen = new Map<string, number>();
  let deleted = 0;
  for (const b of backups) {
    const n = (seen.get(b.walletId) ?? 0) + 1;
    seen.set(b.walletId, n);
    if (n > keep) deleted++;
  }
  return deleted;
}

// ------------------------------------------------------------------- export for Dash Core (QT-109)

type ExportApi = Pick<Backend, "lockStatus" | "unlock" | "unlockMixingOnly" | "coreMnemonicCompatibility" | "authorize" | "exportForCore" | "backupDirectory" | "revokeGrant">;

/** The wallet's compatibility first (its warnings show before the passphrase), then a
 * `revealSecret` grant and the export into the backup folder. */
export class ExportModel extends Model {
  step: "loading" | "confirm" | "working" | "done" = "loading";
  encrypted = true;
  warnings: CoreExportWarningName[] = [];
  error: string | null = null;
  done: CoreExportReport | null = null;
  readonly lock: LockModel;

  constructor(
    private readonly api: ExportApi,
    private readonly now: () => Date,
    readonly walletId: string,
    private readonly name: string,
  ) {
    super();
    this.lock = new LockModel(api);
  }

  get message(): string | null {
    return this.error ?? this.lock.message;
  }

  async load() {
    try {
      this.encrypted = (await this.api.lockStatus()).encrypted;
      if (this.encrypted) await this.lock.load();
      this.warnings = (await this.api.coreMnemonicCompatibility(this.walletId)).warnings;
    } catch (e) {
      // A locked vault cannot check the phrase: the export still asks for the passphrase.
      if (!hasCode(e, "vault.locked") && !hasCode(e, "vault.mixing_only")) this.error = failureText(e);
    }
    this.step = "confirm";
    this.changed();
  }

  /** The passphrase is being checked or the file written: the sheet stays open (D31). */
  get busy(): boolean {
    return this.#authorizing || this.lock.working || this.step === "working";
  }

  #authorizing = false;
  // Not a flag: StrictMode cancels and mounts the same model again (D37).
  #epoch = 0;

  /** The sheet is gone: an authorization still running writes nothing. */
  cancel() {
    this.#epoch++;
  }

  async export(format: CoreExportFormatName, passphrase: string) {
    if (this.step !== "confirm" || this.busy) return;
    let grant: string | null = null;
    const authorize = async (p: string) => {
      grant = (await this.api.authorize({ kind: "revealSecret" }, this.walletId, vaultCredential(this.encrypted, p))).id;
    };
    this.error = null;
    const epoch = this.#epoch;
    this.#authorizing = true;
    this.changed();
    let ok: boolean;
    try {
      ok = this.encrypted
        ? await this.lock.attempt(passphrase, authorize)
        : await authorize("").then(
            () => true,
            (e) => {
              this.error = failureText(e);
              return false;
            },
          );
    } finally {
      this.#authorizing = false;
    }
    if (!ok || !grant) return this.changed();
    // The sheet went away while the passphrase was checked: no key is written (D31).
    if (epoch !== this.#epoch) return void this.api.revokeGrant(grant).catch(() => {});
    this.step = "working";
    this.changed();
    try {
      const dest = inDirectory(await this.api.backupDirectory(), fileName(this.name, this.now(), format));
      this.done = await this.api.exportForCore(this.walletId, format, dest, grant);
      this.step = "done";
    } catch (e) {
      this.api.revokeGrant(grant).catch(() => {});
      this.error = failureText(e);
      this.step = "confirm";
    }
    this.changed();
  }
}

/** Show Folder's refusal: no folder yet, the folder not opening, or anything else. */
export function folderErrorText(e: unknown): string {
  if (hasCode(e, "app.backup_folder_missing")) return t("settings.backups.noFolderYet");
  if (hasCode(e, "app.folder_open")) return t("settings.backups.showFolderFailed");
  return failureText(e);
}
