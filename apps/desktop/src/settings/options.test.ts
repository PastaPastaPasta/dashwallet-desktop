// Settings ▸ General's rules (QT-020, QT-036, QT-039, QT-075, QT-135…141, IOS-104…106) and the
// display settings' life (prefs.ts, D2, D3, D8, D14): the Swift `OptionsViewModel` and
// `SettingsProviding` tests, ported by parity ID, over the fixture backend.

import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IpcBackend } from "../api/tauri";
import { DEFAULT_PREFS, ui } from "../app/ui";
import { installFixture, type FixtureHandle } from "../fixture";
import { DIGIT_CHOICES, dustThreshold, OptionsText, UNIT_CHOICES } from "./options";
import { loadPrefs, savePrefs } from "./prefs";

let fixture: FixtureHandle;
let b: IpcBackend;
beforeEach(() => {
  vi.stubGlobal("window", globalThis);
  fixture = installFixture({ scenario: "funded" });
  b = new IpcBackend("fixture", () => fixture.now, fixture.locale);
  ui.set({ prefs: DEFAULT_PREFS, theme: null, discreet: false });
});
afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
});

const launch = (theme: "light" | "dark" | null = null) => ({ ...fixture.world.launch, theme });

describe("display settings", () => {
  it("QT020_QT036_QT039_displaySettingsPersistAndClamp", async () => {
    expect(DIGIT_CHOICES).toEqual([2, 3, 4, 5, 6, 7, 8]);
    await savePrefs(b, launch(), { decimalDigits: 8, unit: "milliDash" });
    expect(ui.get().prefs).toMatchObject({ decimalDigits: 8, unit: "milliDash" });
    expect(fixture.world.settings.ui).toMatchObject({ decimalDigits: 8, unit: "milliDash" });
    // dw-app refuses what the file must not hold, and the store keeps the last saved values.
    await expect(savePrefs(b, launch(), { decimalDigits: 9 })).rejects.toMatchObject({ code: "invalid_argument" });
    expect(ui.get().prefs.decimalDigits).toBe(8);
  });

  it("QT139_unitsAreDashQts", () => {
    expect(UNIT_CHOICES.map((u) => u.unit)).toEqual(["dash", "milliDash", "microDash", "duffs"]);
  });

  it("QT137_walletDefaultsAreDashQts", () => {
    // dash-qt's defaults: DASH, two digits, the fee added to the amount; auto lock off (IOS-015).
    expect(DEFAULT_PREFS).toEqual({ theme: null, unit: "dash", decimalDigits: 2, autohideBalance: false, autoLockSecs: null, subtractFeeByDefault: false });
  });

  it("themeAndLanguage", async () => {
    // The saved theme applies unless --theme was given (D3).
    fixture.world.settings.ui = { ...DEFAULT_PREFS, theme: "dark", autohideBalance: true };
    await loadPrefs(b, launch());
    expect(ui.get()).toMatchObject({ theme: "dark", discreet: true });
    await loadPrefs(b, launch("light"));
    expect(ui.get().theme).toBe("light");
    await savePrefs(b, launch("light"), { theme: null });
    expect(ui.get().theme).toBe("light");
    expect(fixture.world.settings.ui.theme).toBeNull();
    await savePrefs(b, launch(), { theme: "dark" });
    expect(ui.get().theme).toBe("dark");
  });

  it("an engine that cannot read the settings leaves the defaults", async () => {
    fixture.fail("ui_settings", "none");
    await loadPrefs(b, launch());
    expect(ui.get().prefs).toEqual(DEFAULT_PREFS);
  });

  it("QT141_resetOptionsConfirmsBacksUpAndQuits", async () => {
    // Ours applies live: no backup copy, no restart (D8); dust protection is the engine's.
    await savePrefs(b, launch(), { unit: "duffs", autoLockSecs: 60, subtractFeeByDefault: true });
    await b.setDustProtection(500);
    await savePrefs(b, launch(), DEFAULT_PREFS);
    expect(fixture.world.settings.ui).toEqual(DEFAULT_PREFS);
    expect(await b.dustProtection()).toBe(500);
    expect(OptionsText.resetTitle).toBe("Confirm options reset");
  });
});

describe("dust protection", () => {
  it("QT075_dustThresholdIsBetweenOneAndAMillionDuffs", () => {
    expect(dustThreshold("0")).toBe(OptionsText.dustThresholdInvalid);
    expect(dustThreshold("1")).toBe(1);
    expect(dustThreshold("1,000,000")).toBe(1_000_000);
    expect(dustThreshold("1000001")).toBe(OptionsText.dustThresholdInvalid);
    expect(dustThreshold("5.5")).toBe(OptionsText.dustThresholdInvalid);
    expect(dustThreshold("")).toBe(OptionsText.dustThresholdInvalid);
  });

  it("QT075_protectionIsSentToTheEngine", async () => {
    expect(await b.dustProtection()).toBe(100);
    await b.setDustProtection(null);
    expect(await b.dustProtection()).toBeNull();
  });
});
