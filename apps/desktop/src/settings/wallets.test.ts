// Settings ▸ Wallets' view models (QT-101, QT-114, IOS-110, IOS-111): the Swift
// `WalletManagementViewModel` tests, ported by parity ID, over the fixture backend.

import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IpcBackend } from "../api/tauri";
import { installFixture, type FixtureHandle } from "../fixture";
import { FIXTURE_PASSPHRASE as PASS, FIXTURE_WALLET_ID as A, SECOND_WALLET_ID as B, type ScenarioName } from "../fixture/scenarios";
import { WRONG_PASSPHRASE } from "../onboarding/model";
import { RemoveWalletModel, walletName, WalletsModel, WalletsText, WatchOnlyModel } from "./wallets";

let fixture: FixtureHandle;
let b: IpcBackend;
function install(scenario: ScenarioName) {
  fixture = installFixture({ scenario });
  b = new IpcBackend("fixture", () => fixture.now, fixture.locale);
}
beforeEach(() => {
  vi.stubGlobal("window", globalThis);
  install("two-wallets");
});
afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
});

describe("wallets list", () => {
  it("QT101_closeKeepsDataAndLeavesLoadOnStartup", async () => {
    const model = new WalletsModel(b);
    await model.load();
    expect(model.rows.map((r) => [r.name, r.loaded, r.loadOnStartup])).toEqual([
      ["Main", true, true],
      ["Savings", true, true],
    ]);
    expect(await model.close(B)).toBe(true);
    expect(model.rows[1]).toMatchObject({ walletId: B, loaded: false, loadOnStartup: false, hasMnemonic: null });
    expect((await b.listWallets()).map((w) => w.walletId)).toEqual([A]);
    // Its data stays: opening it again brings it back, on the startup list too.
    await model.open(B);
    expect(model.rows[1]).toMatchObject({ loaded: true, loadOnStartup: true, hasMnemonic: true });
  });

  it("QT101_theLastOpenWalletCannotBeClosed", async () => {
    const model = new WalletsModel(b);
    await model.load();
    await model.close(B);
    expect(model.canClose(model.rows[0])).toBe(false);
    expect(await model.close(A)).toBe(false);
    expect(model.error).toBe(WalletsText.openAnotherFirst);
    expect(fixture.calls.filter((c) => c === "unload_wallet")).toHaveLength(1);
  });

  it("QT101_theStartupListIsSetAlone", async () => {
    const model = new WalletsModel(b);
    await model.load();
    await model.setStartup(B, false);
    expect(model.rows[1]).toMatchObject({ loaded: true, loadOnStartup: false });
  });

  it("IOS110_listFallsBackToLoadedWalletsWhileLoadStatesAreNotImplemented", async () => {
    fixture.fail("wallet_load_states", "call");
    const model = new WalletsModel(b);
    await model.load();
    expect(model.fallback).toBe(true);
    expect(model.rows.map((r) => r.name)).toEqual(["Main", "Savings"]);
    expect(model.canClose(model.rows[0])).toBe(false);
  });

  it("IOS110_renameKeepsTheEnginesRule", async () => {
    const model = new WalletsModel(b);
    await model.load();
    expect(await model.rename(A, "   ")).toBe(false);
    expect(model.error).toBe(WalletsText.nameRule);
    expect(await model.rename(A, "  Everyday ")).toBe(true);
    expect(model.rows[0].name).toBe("Everyday");
    expect(walletName("x".repeat(64))).toBe("x".repeat(64));
    expect(walletName("x".repeat(65))).toBeNull();
  });
});

describe("remove wallet", () => {
  it("IOS110_removeNeedsAWipeGrant", async () => {
    const model = new RemoveWalletModel(b, B);
    await model.load();
    await model.remove("wrong");
    expect(model.step).toBe("confirm");
    expect(model.message).toContain(WRONG_PASSPHRASE);
    expect(fixture.calls).not.toContain("remove_wallet");
    await model.remove(PASS);
    expect(model.step).toBe("done");
    expect((await b.walletLoadStates()).map((w) => w.walletId)).toEqual([A]);
    expect(fixture.world.lock.walletsWithSecrets).toEqual([A]);
  });

  it("IOS110_anUnencryptedVaultRemovesWithoutAPassphrase", async () => {
    install("unencrypted");
    const model = new RemoveWalletModel(b, A);
    await model.load();
    expect(model.encrypted).toBe(false);
    await model.remove("");
    expect(model.step).toBe("done");
    expect(fixture.world.wallets).toEqual([]);
  });
});

describe("watch-only and the account xpub", () => {
  it("QT114_watchOnlyWalletFromAnXpub", async () => {
    const xpub = (await b.accountXpub(A, 0)).xpub;
    const model = new WatchOnlyModel(b);
    await model.add("", "");
    expect(model.error).toBe("Paste an extended public key.");
    await model.add("xpub-garbage", "");
    expect(model.error).toBe("This is not an extended public key (tpub or xpub) of this network.");
    await model.add(`  ${xpub}\n`, " Cold storage ");
    expect(model.added).toMatchObject({ name: "Cold storage" });
    expect((await b.walletLoadStates()).find((w) => w.walletId === model.added!.walletId)).toMatchObject({ name: "Cold storage", watchOnly: true });
  });

  it("IOS111_xpubWithQRCode", async () => {
    const x = await b.accountXpub(A, 0);
    expect(x).toMatchObject({ account: 0, derivationPath: "m/44'/1'/0'" });
    expect(x.xpub.startsWith("tpub")).toBe(true);
    // The sheet's QR encodes exactly the key.
    expect((await b.qrMatrix(x.xpub)).size).toBeGreaterThan(0);
  });
});
