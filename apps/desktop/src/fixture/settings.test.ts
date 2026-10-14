// The fixture's settings engine (settings.ts, U-04) and the vault's credential commands
// (handlers.ts), through the generated commands as Settings calls them.

import { clearMocks } from "@tauri-apps/api/mocks";
import { listen } from "@tauri-apps/api/event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as cmd from "../api/generated/commands";
import type { AppEvent } from "../api/generated/commands";
import { installFixture, type FixtureHandle } from "./index";
import { FIXTURE_PASSPHRASE, FIXTURE_WALLET_ID as W, SECOND_WALLET_ID as W2, type ScenarioName } from "./scenarios";
import { CORE_WALLET_PASSPHRASE, DEFAULT_UI_SETTINGS, FIXTURE_BACKUP_DIR, FIXTURE_FILES } from "./settings";

const PASS = { kind: "passphrase", passphrase: FIXTURE_PASSPHRASE } as const;
const OPTIONS = { name: null, birthHeight: null, coreCompat: false, lookahead: null };

let fixture: FixtureHandle;
const install = (scenario: ScenarioName) => (fixture = installFixture({ scenario }));
beforeEach(() => {
  vi.stubGlobal("window", globalThis);
  install("funded");
});
afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
});

async function events() {
  const seen: AppEvent[] = [];
  await listen<AppEvent>(cmd.ENGINE_EVENT, (e) => void seen.push(e.payload));
  return seen;
}

describe("fixture settings engine", () => {
  it("keeps the display settings with dw-app's bounds", async () => {
    expect(await cmd.uiSettings()).toEqual(DEFAULT_UI_SETTINGS);
    const s = { ...DEFAULT_UI_SETTINGS, unit: "milliDash", decimalDigits: 8, autoLockSecs: 300 } as const;
    expect(await cmd.setUiSettings(s)).toEqual(s);
    expect(await cmd.uiSettings()).toEqual(s);
    for (const bad of [{ decimalDigits: 1 }, { decimalDigits: 9 }, { autoLockSecs: 0 }, { autoLockSecs: 86_401 }])
      await expect(cmd.setUiSettings({ ...s, ...bad })).rejects.toMatchObject({ code: "invalid_argument" });
    expect(await cmd.uiSettings()).toEqual(s);
  });

  it("closes a wallet without forgetting it and opens it again", async () => {
    install("two-wallets");
    const seen = await events();
    await cmd.unloadWallet(W2);
    expect((await cmd.listWallets()).map((w) => w.walletId)).toEqual([W]);
    await expect(cmd.balance(W2)).rejects.toMatchObject({ code: "wallet_not_found" });
    await cmd.setLoadOnStartup(W2, false);
    expect(await cmd.walletLoadStates()).toEqual([
      { walletId: W, name: "Main", loaded: true, loadOnStartup: true, watchOnly: false },
      { walletId: W2, name: "Savings", loaded: false, loadOnStartup: false, watchOnly: false },
    ]);
    await cmd.loadWallet(W2);
    await cmd.loadWallet(W2);
    expect((await cmd.listWallets()).length).toBe(2);
    expect(seen.map((e) => e.kind === "walletLoadChanged" && e.loaded)).toEqual([false, true]);
  });

  it("removes a wallet only with its wipe grant", async () => {
    install("two-wallets");
    const reveal = await cmd.authorize({ kind: "revealSecret" }, W2, PASS);
    await expect(cmd.removeWallet(W2, reveal.id)).rejects.toMatchObject({ code: "vault.grant_invalid" });
    const wipeOfOther = await cmd.authorize({ kind: "wipe" }, W, PASS);
    await expect(cmd.removeWallet(W2, wipeOfOther.id)).rejects.toMatchObject({ code: "vault.grant_invalid" });
    const wipe = await cmd.authorize({ kind: "wipe" }, W2, PASS);
    await cmd.removeWallet(W2, wipe.id);
    expect((await cmd.walletLoadStates()).map((w) => w.walletId)).toEqual([W]);
    expect((await cmd.lockStatus()).walletsWithSecrets).toEqual([W]);
    await expect(cmd.removeWallet(W2, wipe.id)).rejects.toMatchObject({ code: "wallet_not_found" });
  });

  it("renames within 1 to 64 characters", async () => {
    await cmd.renameWallet(W, "  Everyday  ");
    expect((await cmd.walletInfo(W)).name).toBe("Everyday");
    await expect(cmd.renameWallet(W, "   ")).rejects.toMatchObject({ code: "invalid_argument" });
    await expect(cmd.renameWallet(W, "x".repeat(65))).rejects.toMatchObject({ code: "invalid_argument" });
  });

  it("adds a watch-only wallet from an account xpub", async () => {
    const { xpub, derivationPath } = await cmd.accountXpub(W, 0);
    expect(derivationPath).toBe("m/44'/1'/0'");
    await expect(cmd.importWatchOnly("xpub-not-a-key", { name: null, birthHeight: null, lookahead: null })).rejects.toMatchObject({ code: "invalid_xpub" });
    const id = await cmd.importWatchOnly(xpub, { name: "Cold", birthHeight: null, lookahead: null });
    expect(await cmd.walletInfo(id)).toMatchObject({ name: "Cold", watchOnly: true, hasMnemonic: false });
    await expect(cmd.importWatchOnly(xpub, { name: null, birthHeight: null, lookahead: null })).rejects.toMatchObject({ code: "wallet_already_exists" });
  });

  it("backs up with the vault's passphrase and restores with it", async () => {
    const dest = `${FIXTURE_BACKUP_DIR}/Main-manual.dwbackup`;
    await expect(cmd.backupWallet(W, dest, "extra")).rejects.toMatchObject({ code: "invalid_argument" });
    const summary = await cmd.backupWallet(W, dest, null);
    expect(summary).toMatchObject({ path: dest, walletId: W, automatic: false });
    await expect(cmd.backupWallet(W, dest, null)).rejects.toMatchObject({ code: "backup.destination_unwritable" });
    expect(await cmd.readBackupHeader(dest)).toMatchObject({ network: "testnet", walletIds: [W], automatic: false });
    expect(await cmd.inspectWalletFile(dest)).toMatchObject({ kind: "dwBackup", walletCount: 1 });
    await expect(cmd.restoreBackup(dest, "wrong")).rejects.toMatchObject({ code: "backup.wrong_passphrase" });
    await expect(cmd.restoreBackup(dest, FIXTURE_PASSPHRASE)).rejects.toMatchObject({ code: "backup.already_exists", data: { kind: "wallet", walletId: W } });
    const wipe = await cmd.authorize({ kind: "wipe" }, W, PASS);
    await cmd.removeWallet(W, wipe.id);
    expect(await cmd.restoreBackup(dest, FIXTURE_PASSPHRASE)).toEqual([W]);
    expect((await cmd.lockStatus()).walletsWithSecrets).toEqual([W]);
  });

  it("asks an unencrypted vault for a backup passphrase", async () => {
    install("unencrypted");
    const dest = `${FIXTURE_BACKUP_DIR}/Main.dwbackup`;
    await expect(cmd.backupWallet(W, dest, null)).rejects.toMatchObject({ code: "backup.passphrase_required" });
    await expect(cmd.backupWallet(W, dest, "")).rejects.toMatchObject({ code: "invalid_argument" });
    await cmd.backupWallet(W, dest, "backup-pass");
    expect(fixture.world.settings.files[dest].passphrase).toBe("backup-pass");
  });

  it("lists automatic backups newest first and trims them to the policy", async () => {
    const all = await cmd.automaticBackups(null);
    expect(all.length).toBe(2);
    expect(all[0].createdAt).toBeGreaterThan(all[1].createdAt);
    expect(await cmd.automaticBackups(W2)).toEqual([]);
    expect(await cmd.backupPolicy()).toEqual({ keep: 5, directory: FIXTURE_BACKUP_DIR });
    await expect(cmd.setBackupPolicy(11)).rejects.toMatchObject({ code: "invalid_argument" });
    expect(await cmd.setBackupPolicy(1)).toEqual({ keep: 1, directory: FIXTURE_BACKUP_DIR });
    expect(await cmd.automaticBackups(W)).toEqual([all[0]]);
    expect(fixture.world.settings.files[all[1].path]).toBeUndefined();
    expect(await cmd.backupDirectory()).toBe(FIXTURE_BACKUP_DIR);
  });

  it("detects files by content and imports Dash Core wallets", async () => {
    expect((await cmd.inspectWalletFile(FIXTURE_FILES.psbt)).kind).toBe("psbt");
    expect((await cmd.inspectWalletFile(FIXTURE_FILES.unknown)).kind).toBe("unknown");
    await expect(cmd.inspectWalletFile("/nowhere")).rejects.toMatchObject({ code: "compat.file_unreadable" });
    const dump = await cmd.importDumpWallet(FIXTURE_FILES.dumpwallet, { ...OPTIONS, name: "From Core" });
    expect(dump).toMatchObject({ labelsImported: 3, keysNotImported: 2, coreCompatSeed: true });
    expect(await cmd.walletInfo(dump.walletId)).toMatchObject({ name: "From Core", watchOnly: false });
    await expect(cmd.importDumpWallet(FIXTURE_FILES.mainnetDump, OPTIONS)).rejects.toMatchObject({
      code: "compat.network_mismatch",
      data: { kind: "keysNetwork", mainnet: true },
    });
    await expect(cmd.importWalletDat(FIXTURE_FILES.walletDatBdb, null, OPTIONS)).rejects.toMatchObject({ code: "not_implemented" });
    await expect(cmd.importWalletDat(FIXTURE_FILES.walletDat, null, OPTIONS)).rejects.toMatchObject({ code: "compat.passphrase_required" });
    await expect(cmd.importWalletDat(FIXTURE_FILES.walletDat, "nope", OPTIONS)).rejects.toMatchObject({ code: "compat.wrong_passphrase" });
    expect(await cmd.importWalletDat(FIXTURE_FILES.walletDat, CORE_WALLET_PASSPHRASE, OPTIONS)).toMatchObject({ scriptsNotImported: 1 });
  });

  it("imports key material only when it is well formed", async () => {
    await expect(cmd.importKeyMaterial({ kind: "hdSeed", seedHex: "abc" }, OPTIONS)).rejects.toMatchObject({ code: "compat.invalid_key_material" });
    await expect(cmd.importKeyMaterial({ kind: "descriptors", json: "{" }, OPTIONS)).rejects.toMatchObject({ code: "compat.invalid_key_material" });
    const seed = await cmd.importKeyMaterial({ kind: "hdSeed", seedHex: "0f".repeat(32) }, OPTIONS);
    expect((await cmd.walletInfo(seed.walletId)).watchOnly).toBe(false);
    await cmd.lock();
    await expect(cmd.importKeyMaterial({ kind: "hdSeed", seedHex: "1e".repeat(32) }, OPTIONS)).rejects.toMatchObject({ code: "vault.locked" });
  });

  it("exports for Dash Core with a reveal grant and reports the phrase warning", async () => {
    const dest = `${FIXTURE_BACKUP_DIR}/Main-dumpwallet.txt`;
    const wipe = await cmd.authorize({ kind: "wipe" }, W, PASS);
    await expect(cmd.exportForCore(W, "dumpWallet", dest, wipe.id)).rejects.toMatchObject({ code: "vault.grant_invalid" });
    fixture.world.settings.coreIncompatible = [W];
    expect(await cmd.coreMnemonicCompatibility(W)).toEqual({ coreCompatible: false, warnings: ["mnemonicNotCoreCompatible"] });
    const grant = await cmd.authorize({ kind: "revealSecret" }, W, PASS);
    expect(await cmd.exportForCore(W, "dumpWallet", dest, grant.id)).toMatchObject({ path: dest, keyCount: 62, warnings: ["mnemonicNotCoreCompatible"] });
    await expect(cmd.exportForCore(W, "dumpWallet", dest, (await cmd.authorize({ kind: "revealSecret" }, W, PASS)).id)).rejects.toMatchObject({
      code: "compat.destination_unwritable",
    });
  });

  it("keeps dust protection between 1 and 1,000,000 duffs or off", async () => {
    expect(await cmd.dustProtection()).toBe(100);
    await cmd.setDustProtection(null);
    expect(await cmd.dustProtection()).toBeNull();
    await expect(cmd.setDustProtection(0)).rejects.toMatchObject({ code: "invalid_argument" });
    await expect(cmd.setDustProtection(1_000_001)).rejects.toMatchObject({ code: "invalid_argument" });
  });
});

describe("fixture vault credential", () => {
  it("encrypts an unencrypted vault with a changeCredential grant and leaves it locked", async () => {
    install("unencrypted");
    await expect(cmd.encryptVault("new-pass", "nope")).rejects.toMatchObject({ code: "vault.grant_invalid" });
    const g = await cmd.authorize({ kind: "changeCredential" }, null, { kind: "unencrypted" });
    expect(await cmd.encryptVault("new-pass", g.id)).toMatchObject({ state: "locked", encrypted: true });
    expect((await cmd.unlock("new-pass")).state).toBe("unlocked");
    const again = await cmd.authorize({ kind: "changeCredential" }, null, { kind: "passphrase", passphrase: "new-pass" });
    await expect(cmd.encryptVault("x", again.id)).rejects.toMatchObject({ code: "vault.already_encrypted" });
  });

  it("changes the passphrase; a wrong old one counts and changes nothing", async () => {
    await expect(cmd.changePassphrase("wrong", "next")).rejects.toMatchObject({ code: "vault.wrong_passphrase", data: { failedAttempts: 1 } });
    expect(fixture.world.passphrase).toBe(FIXTURE_PASSPHRASE);
    expect(await cmd.changePassphrase(FIXTURE_PASSPHRASE, "next")).toMatchObject({ state: "unlocked", failedAttempts: 0 });
    await cmd.lock();
    await expect(cmd.unlock(FIXTURE_PASSPHRASE)).rejects.toMatchObject({ code: "vault.wrong_passphrase" });
    expect((await cmd.unlock("next")).state).toBe("unlocked");
  });

  it("turns payment authentication off only with a changeCredential grant", async () => {
    const reveal = await cmd.authorize({ kind: "revealSecret" }, W, PASS);
    await expect(cmd.setPaymentAuthentication(false, reveal.id)).rejects.toMatchObject({ code: "vault.grant_invalid" });
    const g = await cmd.authorize({ kind: "changeCredential" }, null, PASS);
    await cmd.setPaymentAuthentication(false, g.id);
    expect(await cmd.paymentAuthentication()).toBe(false);
    install("unencrypted");
    const u = await cmd.authorize({ kind: "changeCredential" }, null, { kind: "unencrypted" });
    await expect(cmd.setPaymentAuthentication(false, u.id)).rejects.toMatchObject({ code: "vault.not_encrypted" });
  });

  it("destroys the vault only once it holds no secrets, with its passphrase", async () => {
    await expect(cmd.destroyVault(PASS)).rejects.toMatchObject({ code: "vault.not_empty" });
    const wipe = await cmd.authorize({ kind: "wipe" }, W, PASS);
    await cmd.removeWallet(W, wipe.id);
    await expect(cmd.destroyVault({ kind: "unencrypted" })).rejects.toMatchObject({ code: "vault.credential_required" });
    await expect(cmd.destroyVault({ kind: "passphrase", passphrase: "wrong" })).rejects.toMatchObject({ code: "vault.wrong_passphrase" });
    expect(await cmd.destroyVault(PASS)).toMatchObject({ state: "noVault", encrypted: false, walletsWithSecrets: [] });
    expect((await cmd.destroyVault(PASS)).state).toBe("noVault");
  });
});
