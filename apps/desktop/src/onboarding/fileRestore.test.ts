// Restore from a file during setup: a backup or a Dash Core file instead of a phrase. The vault's
// passphrase comes last, as a phrase restore's does, and nothing is read or stored before Restore.

import { describe, expect, it } from "vitest";
import type { CoreImportReport } from "../api/generated/CoreImportReport";
import type { ImportOptions } from "../api/generated/ImportOptions";
import type { WalletFileInfo } from "../api/generated/WalletFileInfo";
import type { ImportApi } from "../settings/files";
import { OnboardingModel } from "./onboarding";
import { appError, FakeWorld, seeded, status, WALLET_A, WALLET_B } from "./testWorld";

const BACKUP = "/home/user/Documents/Main.dwbackup";
const DAT = "/home/user/Documents/core-sqlite-wallet.dat";
const BACKUP_PASS = "backup pass";
const testnet = { kind: "testnet", dirName: "testnet", unitName: "tDASH" } as const;

class FakeFiles implements ImportApi {
  restores: { path: string; passphrase: string | null; vault: string }[] = [];
  /** Holds `inspectWalletFile` of a path until its gate opens. */
  gates: Record<string, Promise<void>> = {};
  emptyRestore = false;
  dats: { path: string; passphrase: string | null; options: ImportOptions }[] = [];
  infos: Record<string, WalletFileInfo> = {
    [BACKUP]: { kind: "dwBackup", network: testnet, walletCount: 2, createdAt: 1, formatVersion: 1 },
    [DAT]: { kind: "walletDatSqlite", encrypted: true, hasMnemonic: true },
    "/home/user/Documents/empty.dwbackup": { kind: "dwBackup", network: testnet, walletCount: 0, createdAt: 1, formatVersion: 1 },
    "/home/user/Documents/core-dump.txt": { kind: "dumpWallet", network: testnet, hasMnemonic: true, hasHdSeed: true, hasXprv: false, looseKeyCount: 2, scriptCount: 0, labelCount: 3 },
    "/elsewhere/mainnet.dwbackup": { kind: "dwBackup", network: { kind: "mainnet", dirName: "mainnet", unitName: "DASH" }, walletCount: 1, createdAt: 1, formatVersion: 1 },
  };
  constructor(private readonly world: FakeWorld) {}
  inspectWalletFile = async (path: string) => {
    await this.gates[path];
    const info = this.infos[path];
    if (!info) throw appError("compat.file_unreadable");
    return info;
  };
  restoreBackup = async (path: string, passphrase: string | null) => {
    if (this.world.status.state === "locked" || this.world.status.state === "noVault") throw appError("backup.vault_locked");
    if (passphrase !== BACKUP_PASS) throw appError("backup.wrong_passphrase");
    this.restores.push({ path, passphrase, vault: this.world.status.state });
    return this.emptyRestore ? [] : [WALLET_A, WALLET_B];
  };
  importDumpWallet = async (): Promise<CoreImportReport> =>
    ({ walletId: WALLET_B, labelsImported: 3, keysNotImported: 2, scriptsNotImported: 0 }) as CoreImportReport;
  importWalletDat = async (path: string, passphrase: string | null, options: ImportOptions): Promise<CoreImportReport> => {
    this.dats.push({ path, passphrase, options });
    return { walletId: WALLET_B, labelsImported: 1, keysNotImported: 0, scriptsNotImported: 0 } as CoreImportReport;
  };
}

function make(world: FakeWorld, network = "testnet") {
  const files = new FakeFiles(world);
  const model = new OnboardingModel(world, "testnet", { random: seeded(1n), files: { api: files, network } });
  return { model, files };
}

describe("onboarding: restore from a file", () => {
  it("restores a backup on a new install: file, its passphrase, then the vault's, then every wallet", async () => {
    const world = new FakeWorld();
    world.status = status({ state: "noVault", encrypted: false });
    const { model, files } = make(world);
    await model.startFileRestore();
    expect(model.step).toBe("restoreFile");
    await model.file!.inspect(BACKUP);
    expect(model.file!.step).toBe("details");
    expect(model.file!.plan?.passphrase).toBeTruthy();

    await model.restoreFromFile(null, "");
    expect(model.file!.error).toBe("Enter the file's passphrase.");
    expect(model.step).toBe("restoreFile");

    await model.restoreFromFile(BACKUP_PASS, "");
    expect(model.step).toBe("choosePassphrase");
    expect(files.restores).toEqual([]);
    expect(world.creates).toEqual([]);

    await model.submitPassphrase("correct horse battery staple", "correct horse battery staple");
    expect(world.creates).toEqual(["correct horse battery staple"]);
    expect(files.restores).toEqual([{ path: BACKUP, passphrase: BACKUP_PASS, vault: "unlocked" }]);
    expect(model.step).toBe("done");
    expect(model.walletId).toBe(WALLET_A);
    expect(model.restoredCount).toBe(2);
    expect(model.keepsVaultChoice).toBe(false);
  });

  it("a wrong backup passphrase goes back to the file with the vault kept; the next try restores", async () => {
    const world = new FakeWorld();
    world.status = status({ state: "noVault", encrypted: false });
    const { model, files } = make(world);
    await model.startFileRestore();
    await model.file!.inspect(BACKUP);
    await model.restoreFromFile("wrong", "");
    await model.skipEncryption();
    expect(model.step).toBe("restoreFile");
    expect(model.file!.step).toBe("details");
    expect(model.file!.error).toBeTruthy();
    expect(world.creates).toEqual([null]);
    await model.restoreFromFile(BACKUP_PASS, "");
    expect(model.step).toBe("done");
    expect(world.creates).toEqual([null]);
    expect(files.restores).toHaveLength(1);
  });

  it("asks for the unlock first when the vault is locked, and imports a wallet.dat with its name", async () => {
    const world = new FakeWorld();
    world.status = status({ state: "locked", walletsWithSecrets: [WALLET_A] });
    const { model, files } = make(world);
    await model.startFileRestore();
    expect(model.step).toBe("unlockVault");
    await model.unlockVault(world.passphrase);
    expect(model.step).toBe("restoreFile");
    await model.file!.inspect(DAT);
    await model.restoreFromFile("core pass", "Old Core wallet");
    expect(files.dats).toEqual([{ path: DAT, passphrase: "core pass", options: expect.objectContaining({ name: "Old Core wallet" }) }]);
    expect(model.step).toBe("done");
    expect(model.walletId).toBe(WALLET_B);
  });

  it("refuses another network's backup before anything is asked, and Back returns to the path, then Welcome", async () => {
    const world = new FakeWorld();
    world.status = status({ state: "noVault", encrypted: false });
    const { model } = make(world);
    await model.startFileRestore();
    await model.file!.inspect("/elsewhere/mainnet.dwbackup");
    expect(model.file!.plan?.refusal).toBeTruthy();
    await model.restoreFromFile("x", "");
    expect(model.step).toBe("restoreFile");
    model.back();
    expect(model.file!.step).toBe("path");
    model.back();
    expect(model.step).toBe("welcome");
    expect(model.file!.path).toBe("");
  });

  it("is not offered without the file commands", async () => {
    const model = new OnboardingModel(new FakeWorld(), "testnet");
    expect(model.file).toBeNull();
    await model.startFileRestore();
    expect(model.step).toBe("welcome");
  });

  it("drops a late inspection of a file the user left: the next file's details stay its own", async () => {
    const world = new FakeWorld();
    world.status = status({ state: "noVault", encrypted: false });
    const { model, files } = make(world);
    let openA!: () => void;
    files.gates[BACKUP] = new Promise((r) => (openA = r));
    await model.startFileRestore();
    const a = model.file!.inspect(BACKUP);
    model.back();
    await model.startFileRestore();
    await model.file!.inspect(DAT);
    openA();
    await a;
    expect(model.file!.path).toBe(DAT);
    expect(model.file!.info?.kind).toBe("walletDatSqlite");
  });

  it("a vault locked between the file and Restore asks for the unlock, then restores (backup.vault_locked)", async () => {
    const world = new FakeWorld();
    world.status = status({ state: "unlocked", walletsWithSecrets: [WALLET_A] });
    const { model, files } = make(world);
    await model.startFileRestore();
    await model.file!.inspect(BACKUP);
    world.status = { ...world.status, state: "locked" };
    await model.restoreFromFile(BACKUP_PASS, "");
    expect(model.step).toBe("unlockVault");
    await model.unlockVault(world.passphrase);
    expect(model.step).toBe("done");
    expect(files.restores).toHaveLength(1);
  });

  it("refuses an empty backup before anything is created, and one that restores nothing stays on the file", async () => {
    const world = new FakeWorld();
    world.status = status({ state: "noVault", encrypted: false });
    const { model, files } = make(world);
    await model.startFileRestore();
    await model.file!.inspect("/home/user/Documents/empty.dwbackup");
    expect(model.file!.plan?.refusal).toBeTruthy();
    model.back();
    await model.file!.inspect(BACKUP);
    files.emptyRestore = true;
    await model.restoreFromFile(BACKUP_PASS, "");
    await model.skipEncryption();
    expect(model.step).toBe("restoreFile");
    expect(model.file!.step).toBe("path");
    expect(model.file!.error).toBe("This backup holds no wallet.");
    expect(model.walletId).toBeNull();
  });

  it("says what a dumpwallet import left out, and takes a test-family dump on regtest", async () => {
    const world = new FakeWorld();
    world.status = status({ state: "unlocked", walletsWithSecrets: [WALLET_A] });
    const { model } = make(world, "regtest");
    await model.startFileRestore();
    await model.file!.inspect("/home/user/Documents/core-dump.txt");
    expect(model.file!.plan?.refusal).toBeNull();
    await model.restoreFromFile(null, "");
    expect(model.step).toBe("done");
    expect(model.restoredSkipped).toEqual({ keys: 2, scripts: 0 });
  });

  it("backing out of the vault passphrase drops the file's passphrase", async () => {
    const world = new FakeWorld();
    world.status = status({ state: "noVault", encrypted: false });
    const { model, files } = make(world);
    await model.startFileRestore();
    await model.file!.inspect(BACKUP);
    await model.restoreFromFile(BACKUP_PASS, "");
    expect(model.step).toBe("choosePassphrase");
    model.back();
    expect(model.step).toBe("restoreFile");
    await model.skipEncryption();
    expect(files.restores).toEqual([]);
  });
});
