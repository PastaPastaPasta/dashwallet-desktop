// Settings ▸ About ▸ Updates (PK-03) over the fixture backend: off by default, the unavailable
// states, and a found update's install and restart, each on the user's click.

import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { UpdatePhase } from "../api/generated/UpdatePhase";
import { IpcBackend } from "../api/tauri";
import { DEFAULT_PREFS } from "../app/ui";
import { installFixture, type FixtureHandle } from "../fixture";
import { isOffered } from "../fixture/updates";
import { automaticCheck, installClosesApp, unavailableText, updateFailureText, UpdatesModel, updatesView } from "./updates";

let fixture: FixtureHandle;
let b: IpcBackend;
function install(updates?: Parameters<typeof installFixture>[0]["updates"], updateServes?: string) {
  fixture = installFixture({ scenario: "funded", updates, updateServes });
  b = new IpcBackend("fixture", () => fixture.now, fixture.locale);
}
const feedReads = () => fixture.world.updates.requests;
const updateCalls = () => fixture.calls.filter((c) => /update/.test(c));
const ON = { checkUpdatesAutomatically: true };
const WINDOWS = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36";
const LINUX = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15";

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

  it("a Mac app on the disk image asks to be moved to Applications", async () => {
    install("moveToApplications", "0.3.0");
    const model = new UpdatesModel(b);
    await model.load();
    expect(model.view).toEqual({ kind: "unavailable", reason: "moveToApplications" });
    expect(unavailableText("moveToApplications")).toMatch(/Applications folder/);
    expect(await automaticCheck(model, ON)).toBeNull();
    expect(feedReads()).toBe(0);
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
    fixture.world.updates.fault = "signature";
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
    fixture.world.updates.fault = "offline";
    expect(await model.check()).toBeNull();
    expect(model.problem).toMatch(/could not reach the update server/);
    expect(model.view).toEqual({ kind: "found", version: "0.3.0" });
  });

  it("a Latest release without a feed says no update information is published", async () => {
    install("available", "0.3.0");
    fixture.world.updates.fault = "unpublished";
    const model = new UpdatesModel(b);
    await model.load();
    expect(await model.check()).toBeNull();
    expect(model.problem).toMatch(/No update information has been published/);
    expect(model.view).toEqual({ kind: "idle" });
  });

  it("on Windows, Install and Restart wait while a payment, mixing, a Platform action or start-up runs", async () => {
    vi.stubGlobal("navigator", { userAgent: WINDOWS });
    for (const reason of ["startup", "payment", "mixing", "platform"] as const) {
      install("available", "0.3.0");
      const model = new UpdatesModel(b);
      await model.check();
      fixture.world.updates.fault = reason;
      await model.install();
      expect(model.problem).toBe(updateFailureText({ code: "app.update_busy", message: "", data: { kind: "updateBusy", reason } }));
      expect(model.view).toEqual({ kind: "found", version: "0.3.0" });
    }
    expect(updateFailureText({ code: "app.update_busy", message: "", data: { kind: "updateBusy", reason: "mixing" } })).toMatch(/Stop mixing/);
  });

  it("a busy refusal without a reason says only that DashPay Desktop is busy", () => {
    expect(updateFailureText({ code: "app.update_busy", message: "", data: null })).toBe("DashPay Desktop is busy. Try again when it has finished.");
  });

  it("on macOS and Linux, Install runs while mixing, and only Restart waits", async () => {
    vi.stubGlobal("navigator", { userAgent: LINUX });
    install("available", "0.3.0");
    const model = new UpdatesModel(b);
    await model.check();
    fixture.world.updates.fault = "mixing";
    await model.install();
    expect(model.problem).toBeNull();
    expect(model.view).toEqual({ kind: "installed", version: "0.3.0" });
    await model.restart();
    expect(model.problem).toMatch(/Stop mixing/);
    expect(fixture.world.updates.restarts).toBe(0);
  });

  it("a download kept while something ran installs without downloading again", async () => {
    vi.stubGlobal("navigator", { userAgent: WINDOWS });
    fixture = installFixture({ scenario: "funded", updates: "available", updateServes: "0.3.0", updatePhase: "downloaded", updateFault: "payment" });
    b = new IpcBackend("fixture", () => fixture.now, fixture.locale);
    const model = new UpdatesModel(b);
    await model.load();
    expect(model.view).toEqual({ kind: "downloaded", version: "0.3.0" });
    await model.install();
    expect(model.problem).toMatch(/payment is being sent/);
    expect(model.view).toEqual({ kind: "downloaded", version: "0.3.0" });
    fixture.world.updates.fault = null;
    await model.install();
    expect(model.view).toEqual({ kind: "installed", version: "0.3.0" });
    expect(feedReads()).toBe(0);
  });

  it("a failed install after the app closed the wallet asks for a restart, which works", async () => {
    install("available", "0.3.0");
    const model = new UpdatesModel(b);
    await model.check();
    fixture.world.updates.fault = "closed";
    await model.install();
    expect(model.problem).toBeNull();
    expect(model.view).toEqual({ kind: "restartRequired", version: "0.3.0" });
    fixture.world.updates.fault = "payment";
    await model.restart();
    expect(fixture.world.updates.restarts).toBe(1);
  });

  it("nothing newer, or only a pre-release, is up to date", async () => {
    for (const serves of [null, "0.1.0", "0.0.9", "0.3.0-rc.1"]) {
      install("available", serves ?? undefined);
      const model = new UpdatesModel(b);
      expect(await model.check()).toBeNull();
      expect(model.view).toEqual({ kind: "upToDate" });
    }
  });
});

describe("the card's states", () => {
  it("shows the phase once this copy can update itself", () => {
    const phase: UpdatePhase = { kind: "downloading", version: "0.3.0", received: 512, total: 1024 };
    expect(updatesView({ availability: "available", phase })).toEqual(phase);
    expect(updatesView({ availability: "flatpak", phase })).toEqual({ kind: "unavailable", reason: "flatpak" });
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
  });
});
