// Settings ▸ Backups' view models (QT-078, QT-106…110, QT-116): the Swift
// `WalletManagementViewModel` file tests, ported by parity ID, over the fixture backend; and the
// generated file names (D19).

import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IpcBackend } from "../api/tauri";
import { installFixture, type FixtureHandle } from "../fixture";
import { FIXTURE_PASSPHRASE as PASS, FIXTURE_WALLET_ID as A, type ScenarioName } from "../fixture/scenarios";
import { CORE_WALLET_PASSPHRASE, FIXTURE_BACKUP_DIR, FIXTURE_FILES } from "../fixture/settings";
import { WRONG_PASSPHRASE } from "../onboarding/model";
import { PassphraseText } from "../onboarding/passphrase";
import { BackupModel, deletedByKeep, ExportModel, fileName, fileStem, FilesText, ImportFileModel, ImportKeysModel, importPlan, inDirectory, keepAsks, keyMaterial } from "./files";

const AT = new Date(2026, 9, 6, 9, 5);

let fixture: FixtureHandle;
let b: IpcBackend;
function install(scenario: ScenarioName) {
  fixture = installFixture({ scenario });
  b = new IpcBackend("fixture", () => fixture.now, fixture.locale);
}
beforeEach(() => {
  vi.stubGlobal("window", globalThis);
  install("funded");
});
afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
});

describe("file names (D19)", () => {
  it("names a file after its wallet and the second, in the folder's own separator", () => {
    expect(fileName("Main", AT, "backup")).toBe("Main-2026-10-06-09-05-00.dwbackup");
    expect(fileName("Mon portefeuille é/../x", AT, "dumpWallet")).toBe("Mon-portefeuille-e-x-2026-10-06-09-05-00-dumpwallet.txt");
    expect(fileName("💰", AT, "importDescriptorsJson")).toBe("wallet-2026-10-06-09-05-00-descriptors.json");
    expect(fileStem("<script>")).toBe("script");
    expect(inDirectory("/home/u/backups", "a.dwbackup")).toBe("/home/u/backups/a.dwbackup");
    expect(inDirectory("C:\\Users\\u\\backups\\", "a.dwbackup")).toBe("C:\\Users\\u\\backups\\a.dwbackup");
  });
});

describe("back up", () => {
  it("QT110_backupOfAnEncryptedVaultUsesItsPassphrase", async () => {
    const model = new BackupModel(b, () => AT);
    await model.backup(A, "Main", null, null);
    expect(model.done).toMatchObject({ path: `${FIXTURE_BACKUP_DIR}/Main-2026-10-06-09-05-00.dwbackup`, walletId: A, automatic: false });
    // The same second again: the engine never writes over a file.
    await model.backup(A, "Main", null, null);
    expect(model.error).toMatch(/a file with that name exists/);
  });

  it("QT110_backupOfAnUnencryptedVaultNeedsAPassphrase", async () => {
    install("unencrypted");
    const model = new BackupModel(b, () => AT);
    await model.backup(A, "Main", "secret", "secert");
    expect(model.error).toBe(PassphraseText.mismatch);
    expect(fixture.calls).not.toContain("backup_wallet");
    await model.backup(A, "Main", "secret", "secret");
    expect(model.done?.path).toBe(`${FIXTURE_BACKUP_DIR}/Main-2026-10-06-09-05-00.dwbackup`);
    expect(fixture.world.settings.files[model.done!.path].passphrase).toBe("secret");
  });

  it("QT116_automaticBackupsAndFolder", async () => {
    expect(await b.backupDirectory()).toBe(FIXTURE_BACKUP_DIR);
    expect((await b.automaticBackups(null)).map((x) => x.automatic)).toEqual([true, true]);
    expect(await b.setBackupPolicy(0)).toEqual({ keep: 0, directory: FIXTURE_BACKUP_DIR });
    expect(await b.automaticBackups(null)).toEqual([]);
  });
});

describe("import a wallet file (D21)", () => {
  it("QT110_restoringABackupAsksForItsPassphrase", async () => {
    const [auto] = await b.automaticBackups(A);
    // Restoring needs the wallet gone: a backup of a wallet already here is refused.
    const model = new ImportFileModel(b, "testnet");
    await model.inspect(auto.path);
    expect(model.plan).toMatchObject({ kind: "dwBackup", refusal: null, passphrase: "Backup passphrase", named: false });
    await model.run(null, "");
    expect(model.error).toBe("Enter the backup passphrase.");
    await model.run("wrong", "");
    expect(model.error).toBe("The passphrase does not open this backup.");
    await model.run(PASS, "");
    expect(model.error).toBe("A wallet in this backup is already in Dash Wallet.");
    const grant = await b.authorize({ kind: "wipe" }, A, { kind: "passphrase", passphrase: PASS });
    await b.removeWallet(A, grant.id);
    await model.run(PASS, "");
    expect(model.step).toBe("done");
    expect(model.done).toEqual({ walletIds: [A], lines: [FilesText.restored(1)] });
  });

  it("QT107_dumpwalletImportReportsKeysNotImported", async () => {
    const model = new ImportFileModel(b, "testnet");
    await model.inspect(` ${FIXTURE_FILES.dumpwallet} `);
    expect(model.plan).toMatchObject({ kind: "dumpWallet", named: true, passphrase: null });
    await model.run(null, "From Core");
    expect(model.done?.lines).toEqual([FilesText.imported(3), FilesText.keysNotImported(2)]);
    expect((await b.listWallets()).at(-1)?.name).toBe("From Core");
  });

  it("QT107_aFileOfAnotherNetworkIsRefusedBeforeAnyImport", async () => {
    const model = new ImportFileModel(b, "testnet");
    await model.inspect(FIXTURE_FILES.mainnetDump);
    expect(model.plan?.refusal).toBe(FilesText.otherNetwork("mainnet"));
    await model.run(null, "");
    expect(fixture.calls).not.toContain("import_dump_wallet");
  });

  it("QT106_encryptedWalletDatAsksForItsPassphrase", async () => {
    const model = new ImportFileModel(b, "testnet");
    await model.inspect(FIXTURE_FILES.walletDat);
    expect(model.plan).toMatchObject({ kind: "walletDatSqlite", passphrase: "wallet.dat passphrase", named: true });
    await model.run("nope", "");
    expect(model.error).toBe("The passphrase does not open this wallet.dat.");
    await model.run(CORE_WALLET_PASSPHRASE, "");
    expect(model.done?.lines).toEqual([FilesText.imported(1), FilesText.scriptsNotImported(1)]);
  });

  it("QT106_berkeleyDBSaysWhatToDoInstead", async () => {
    const model = new ImportFileModel(b, "testnet");
    await model.inspect(FIXTURE_FILES.walletDatBdb);
    expect(model.plan?.refusal).toBe(FilesText.berkeleyDBUnavailable);
  });

  it("QT078_psbtFileIsHandedToThePSBTDialog", async () => {
    const model = new ImportFileModel(b, "testnet");
    await model.inspect(FIXTURE_FILES.psbt);
    expect(model.plan?.refusal).toBe(FilesText.psbtFile);
    await model.inspect(FIXTURE_FILES.unknown);
    // `inspect` runs from the path step only.
    model.back();
    await model.inspect(FIXTURE_FILES.unknown);
    expect(model.plan?.refusal).toBe(FilesText.unknownFile);
  });

  it("a path that cannot be read stays on the path step", async () => {
    const model = new ImportFileModel(b, "testnet");
    await model.inspect("   ");
    expect(model.error).toBe("Enter the file's full path.");
    await model.inspect("/nowhere/wallet.dat");
    expect(model.step).toBe("path");
    expect(model.error).toBe("The file could not be read. Check the path.");
  });

  it("plans a backup of an unknown network as a refusal", () => {
    expect(importPlan({ kind: "dwBackup", network: null, walletCount: 2, createdAt: 0, formatVersion: 9 }, "testnet")).toMatchObject({
      refusal: FilesText.otherNetwork("a network this version does not know"),
      summary: "A Dash Wallet backup of 2 wallets.",
    });
  });
});

describe("import keys", () => {
  it("QT108_hdSeedHexGoesIntoAZeroingBuffer", async () => {
    // The renderer checks the shape; the engine wraps it in `Zeroizing` (KeyMaterialInput).
    expect(keyMaterial("hdSeed", "abc")).toBe(FilesText.invalidSeedHex);
    expect(keyMaterial("hdSeed", ` ${"0F".repeat(32)} `)).toEqual({ kind: "hdSeed", seedHex: "0F".repeat(32) });
    expect(keyMaterial("xprv", "xpub123")).toBe(FilesText.invalidXprv);
    expect(keyMaterial("descriptors", "[{")).toBe(FilesText.invalidDescriptors);
    const model = new ImportKeysModel(b);
    await model.run("hdSeed", "zz", "");
    expect(model.error).toBe(FilesText.invalidSeedHex);
    expect(fixture.calls).not.toContain("import_key_material");
    await model.run("hdSeed", "0f".repeat(32), "Seeded");
    expect(model.done?.lines).toEqual([FilesText.imported(0)]);
  });

  it("QT108_theEnginesRefusalIsShownByCode", async () => {
    const model = new ImportKeysModel(b);
    await model.run("descriptors", '{"not":"descriptors"}', "");
    expect(model.error).toBe("The key material is not valid.");
  });
});

describe("backups to keep (D30)", () => {
  const backup = (walletId: string, n: number) => ({ path: `/b/${walletId}-${n}.dwbackup`, walletId, createdAt: 100 - n, automatic: true, sizeBytes: 1 });
  // Newest first, as automaticBackups lists them: A has 3, B has 1.
  const list = [backup("A", 1), backup("B", 1), backup("A", 2), backup("A", 3)];

  it("QT110_aLowerCountSaysHowManyListedBackupsItDeletes", () => {
    expect(deletedByKeep(list, 0)).toBe(4);
    expect(deletedByKeep(list, 1)).toBe(2);
    expect(deletedByKeep(list, 2)).toBe(1);
    expect(deletedByKeep(list, 3)).toBe(0);
    expect(deletedByKeep([], 0)).toBe(0);
    expect(deletedByKeep(undefined, 0)).toBeNull();
  });

  it("QT110_everyLowerCountAsksEvenWhenTheListShowsNothingToDelete", () => {
    // D42: by this list keeping 3 deletes nothing, but a backup written since may go.
    expect(deletedByKeep(list, 3)).toBe(0);
    expect(keepAsks(5, 3)).toBe(true);
    expect(keepAsks(5, 0)).toBe(true);
    expect(keepAsks(5, 5)).toBe(false);
    expect(keepAsks(5, 8)).toBe(false);
  });

  it("QT110_theQuestionNamesWhatGoesAndWhatTheListMayMiss", () => {
    expect(FilesText.keepDeletes(0, 4, "/b")).toBe(
      "Turning automatic backups off deletes every automatic backup in /b now, and no new ones are made. Of the backups listed here, 4 go, and any written since the list was read.",
    );
    expect(FilesText.keepDeletes(1, 1, "/b")).toBe(
      "Keeping 1 for each wallet deletes its older automatic backups in /b now: each wallet keeps its newest 1. Of the backups listed here, 1 goes, and any written since the list was read.",
    );
    expect(FilesText.keepDeletes(3, 0, "/b")).toMatch(/None of the backups listed here goes, only any written since the list was read\.$/);
    expect(FilesText.keepDeletes(3, null, "/b")).toMatch(/keeps its newest 3\.$/);
  });
});

describe("export for Dash Core", () => {
  it("QT109_exportForCoreNeedsARevealGrantAndReportsWarnings", async () => {
    fixture.world.settings.coreIncompatible = [A];
    const model = new ExportModel(b, () => AT, A, "Main");
    await model.load();
    expect(model.warnings).toEqual(["mnemonicNotCoreCompatible"]);
    await model.export("dumpWallet", "wrong");
    expect(model.message).toContain(WRONG_PASSPHRASE);
    expect(fixture.calls).not.toContain("export_for_core");
    await model.export("dumpWallet", PASS);
    expect(model.step).toBe("done");
    expect(model.done).toMatchObject({ path: `${FIXTURE_BACKUP_DIR}/Main-2026-10-06-09-05-00-dumpwallet.txt`, keyCount: 62, warnings: ["mnemonicNotCoreCompatible"] });
  });

  it("QT109_aClosedSheetWritesNoKeys", async () => {
    const model = new ExportModel(b, () => AT, A, "Main");
    await model.load();
    const release = fixture.hold("authorize");
    const exporting = model.export("dumpWallet", PASS);
    await vi.waitFor(() => expect(fixture.calls).toContain("authorize"));
    expect(model.busy).toBe(true);
    model.cancel();
    release();
    await exporting;
    expect(fixture.calls).not.toContain("export_for_core");
    expect(fixture.calls).toContain("revoke_grant");
  });

  it("QT109_aSheetMountedAgainStillExports", async () => {
    const model = new ExportModel(b, () => AT, A, "Main");
    await model.load();
    // StrictMode runs the sheet's cleanup once and mounts the same model again (D37).
    model.cancel();
    await model.export("dumpWallet", PASS);
    expect(model.step).toBe("done");
    expect(fixture.calls).toContain("export_for_core");
  });

  it("QT109_aLockedVaultStillOffersTheExport", async () => {
    install("locked");
    const model = new ExportModel(b, () => AT, A, "Main");
    await model.load();
    expect(model.step).toBe("confirm");
    expect(model.warnings).toEqual([]);
    // A passphrase grant carries its own key, so a locked vault exports too.
    await model.export("importDescriptorsJson", PASS);
    expect(model.done?.path).toBe(`${FIXTURE_BACKUP_DIR}/Main-2026-10-06-09-05-00-descriptors.json`);
  });
});
