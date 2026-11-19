// The fixture's update check (updates.ts, PK-03a) through the generated commands: off by default
// with no request, one request per check while on, nothing inside Flatpak.

import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as cmd from "../api/generated/commands";
import { installFixture, type FixtureHandle } from "./index";
import type { ScenarioName } from "./scenarios";

let fixture: FixtureHandle;
const install = (scenario: ScenarioName) => (fixture = installFixture({ scenario }));
beforeEach(() => vi.stubGlobal("window", globalThis));
afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
});

describe("fixture update check", () => {
  it("is off by default and makes no request", async () => {
    install("funded");
    expect(await cmd.updateStatus()).toEqual({ supported: true, enabled: false, state: { kind: "off" }, checkedAt: null });
    await expect(cmd.checkForUpdate()).rejects.toMatchObject({ code: "updates.off" });
    await expect(cmd.openReleaseNotes()).rejects.toMatchObject({ code: "app.release_notes_open" });
    expect(fixture.world.updates.requests).toBe(0);
  });

  it("checks once per call when on, and Off forgets the result", async () => {
    install("update-off");
    expect((await cmd.setUpdateCheck(true)).state).toEqual({ kind: "notChecked" });
    expect(fixture.world.updates.requests).toBe(0);
    const v = await cmd.checkForUpdate();
    expect(v.state).toMatchObject({ kind: "available", release: { version: "0.9.0" } });
    expect(v.checkedAt).not.toBeNull();
    expect(await cmd.openReleaseNotes()).toBeNull();
    expect(fixture.world.updates.opened).toEqual(["0.9.0"]);
    expect(await cmd.setUpdateCheck(false)).toEqual({ supported: true, enabled: false, state: { kind: "off" }, checkedAt: null });
    expect(fixture.world.updates.requests).toBe(1);
  });

  it("gives each scenario's answer", async () => {
    const cases: [ScenarioName, string][] = [
      ["update-available", "available"],
      ["update-current", "current"],
      ["update-error", "failed"],
    ];
    for (const [scenario, kind] of cases) {
      install(scenario);
      expect((await cmd.updateStatus()).state.kind).toBe("notChecked");
      expect((await cmd.checkForUpdate()).state.kind).toBe(kind);
      clearMocks();
    }
  });

  it("asks nothing inside Flatpak, whatever was saved", async () => {
    install("update-flatpak");
    expect(await cmd.checkForUpdate()).toEqual({ supported: false, enabled: true, state: { kind: "off" }, checkedAt: null });
    expect(fixture.world.updates.requests).toBe(0);
  });
});
