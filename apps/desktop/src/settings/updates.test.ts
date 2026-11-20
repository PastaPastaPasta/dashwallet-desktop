// Settings ▸ About ▸ Updates (PK-03) over the fixture backend: off by default, the unavailable
// states (a build without the key, Flatpak, a package), and a found update's install and restart,
// each on the user's click.

import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { UpdatePhase } from "../api/generated/UpdatePhase";
import { IpcBackend } from "../api/tauri";
import { DEFAULT_PREFS } from "../app/ui";
import { installFixture, type FixtureHandle } from "../fixture";
import { isOffered } from "../fixture/updates";
import { automaticCheck, installClosesApp, unavailableText, UpdatesModel, updatesView } from "./updates";

let fixture: FixtureHandle;
let b: IpcBackend;
function install(updates?: Parameters<typeof installFixture>[0]["updates"], updateServes?: string) {
  fixture = installFixture({ scenario: "funded", updates, updateServes });
  b = new IpcBackend("fixture", () => fixture.now, fixture.locale);
}
const feedReads = () => fixture.world.updates.requests;
const updateCalls = () => fixture.calls.filter((c) => /update/.test(c) && c !== "set_ui_settings" && c !== "ui_settings");
const ON = { checkUpdatesAutomatically: true };

beforeEach(() => {
  vi.stubGlobal("window", globalThis);
  install();
});
afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
});

describe("off by default", () => {
  it("starts with the automatic check off, in the app and in dw-app's settings", async () => {
    expect(DEFAULT_PREFS.checkUpdatesAutomatically).toBe(false);
    expect((await b.uiSettings()).checkUpdatesAutomatically).toBe(false);
  });

  it("makes no call and reads no feed while the automatic check is off", async () => {
    install("available", "0.3.0");
    const model = new UpdatesModel(b);
    expect(await automaticCheck(model, DEFAULT_PREFS)).toBeNull();
    expect(updateCalls()).toEqual([]);
    expect(feedReads()).toBe(0);
  });

  it("keeps the choice once turned on", async () => {
    const saved = await b.setUiSettings({ ...(await b.uiSettings()), ...ON });
    expect(saved.checkUpdatesAutomatically).toBe(true);
    expect((await b.uiSettings()).checkUpdatesAutomatically).toBe(true);
  });
});

describe("unavailable", () => {
  it("a build without the updater key says so and reads no feed, even when turned on", async () => {
    const model = new UpdatesModel(b);
    await model.load();
    expect(model.view).toEqual({ kind: "unavailable", reason: "notInThisBuild" });
    expect(unavailableText("notInThisBuild")).toMatch(/does not update itself/);
    expect(await automaticCheck(model, ON)).toBeNull();
    expect(feedReads()).toBe(0);
    await expect(b.checkForUpdate()).rejects.toMatchObject({ code: "app.updates_unavailable" });
    expect(feedReads()).toBe(0);
  });

  it("a Flatpak points at Flatpak and reads no feed", async () => {
    install("flatpak", "0.3.0");
    const model = new UpdatesModel(b);
    await model.load();
    expect(model.view).toEqual({ kind: "unavailable", reason: "flatpak" });
    expect(unavailableText("flatpak")).toMatch(/Flatpak/);
    expect(await automaticCheck(model, ON)).toBeNull();
    expect(feedReads()).toBe(0);
  });

  it("a package install points at the releases page", async () => {
    install("packageManager", "0.3.0");
    const model = new UpdatesModel(b);
    await model.load();
    expect(model.view).toEqual({ kind: "unavailable", reason: "packageManager" });
    expect(unavailableText("packageManager")).toMatch(/releases page/);
  });
});

describe("available", () => {
  it("turned on, checks and says a newer version is news once", async () => {
    install("available", "0.3.0");
    const model = new UpdatesModel(b);
    expect(await automaticCheck(model, ON)).toBe("0.3.0");
    expect(model.view).toEqual({ kind: "found", version: "0.3.0" });
    expect(feedReads()).toBe(1);
    expect(await automaticCheck(model, ON)).toBeNull();
    expect(feedReads()).toBe(2);
    // Shown only: nothing installed, nothing restarted.
    expect(fixture.world.updates.phase).toEqual({ kind: "found", version: "0.3.0" });
    expect(fixture.calls).not.toContain("install_update");
    expect(fixture.calls).not.toContain("restart_to_update");
  });

  it("off, checks only when the user presses Check for updates", async () => {
    install("available", "0.3.0");
    const model = new UpdatesModel(b);
    await model.load();
    expect(model.view).toEqual({ kind: "idle" });
    expect(feedReads()).toBe(0);
    expect(await model.check()).toBe("0.3.0");
    expect(feedReads()).toBe(1);
  });

  it("installs on Install and restarts on Restart", async () => {
    install("available", "0.3.0");
    const model = new UpdatesModel(b);
    await model.check();
    await model.openNotes();
    expect(fixture.world.updates.notesOpened).toEqual(["0.3.0"]);
    await model.restart();
    expect(fixture.calls).not.toContain("restart_to_update");
    await model.install();
    expect(model.problem).toBeNull();
    expect(model.view).toEqual({ kind: "installed", version: "0.3.0" });
    expect(fixture.world.updates.restarts).toBe(0);
    // Installed: Check for updates reads nothing more.
    await model.check();
    expect(feedReads()).toBe(1);
    await model.restart();
    expect(fixture.world.updates.restarts).toBe(1);
  });

  it("a refused signature installs nothing and keeps the offer", async () => {
    install("available", "0.3.0");
    fixture.world.updates.badSignature = true;
    const model = new UpdatesModel(b);
    await model.check();
    await model.install();
    expect(model.problem).toMatch(/signature check/);
    expect(model.view).toEqual({ kind: "found", version: "0.3.0" });
  });

  it("an unreachable feed says so and keeps what was known", async () => {
    install("available", "0.3.0");
    const model = new UpdatesModel(b);
    await model.check();
    fixture.world.updates.checkFails = true;
    expect(await model.check()).toBeNull();
    expect(model.problem).toMatch(/could not reach the update server/);
    expect(model.view).toEqual({ kind: "found", version: "0.3.0" });
  });

  it("nothing newer, or only a pre-release, is up to date", async () => {
    for (const serves of [null, "0.2.0", "0.1.9", "0.3.0-rc.1"]) {
      install("available", serves ?? undefined);
      const model = new UpdatesModel(b);
      expect(await model.check()).toBeNull();
      expect(model.view).toEqual({ kind: "upToDate" });
    }
  });
});

describe("the card's states", () => {
  it("shows the download's progress, then the install", () => {
    const status = (phase: UpdatePhase) => updatesView({ availability: "available", currentVersion: "0.2.0", phase });
    expect(status({ kind: "downloading", version: "0.3.0", received: 0, total: null })).toEqual({ kind: "installing", version: "0.3.0", percent: null });
    expect(status({ kind: "downloading", version: "0.3.0", received: 512, total: 1024 })).toEqual({ kind: "installing", version: "0.3.0", percent: 50 });
    expect(status({ kind: "installing", version: "0.3.0" })).toEqual({ kind: "installing", version: "0.3.0", percent: 100 });
    expect(updatesView(null)).toEqual({ kind: "loading" });
  });

  it("on Windows, Install says it closes the app", () => {
    expect(installClosesApp("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")).toBe(true);
    expect(installClosesApp("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15")).toBe(false);
    expect(installClosesApp("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15")).toBe(false);
  });

  it("the fixture offers what dw-app offers", () => {
    expect(isOffered("0.2.0", "0.2.1")).toBe(true);
    expect(isOffered("0.2.0", "0.2.0")).toBe(false);
    expect(isOffered("0.2.0", "0.1.9")).toBe(false);
    expect(isOffered("0.2.0", "0.3.0-rc.1")).toBe(false);
    expect(isOffered("0.3.0-rc.1", "0.3.0")).toBe(true);
  });
});
