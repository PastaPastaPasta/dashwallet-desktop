// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
// The fixture's update check (PK-03a): the setting and what the next check finds, under dw-app's rules
// (`api/updates.rs`, `dw_engine::updates`). No request leaves the fixture; `requests` counts the checks
// that would reach GitHub, so a test can see that none happen while the check is off or in Flatpak.

import type { AppError } from "../api/generated/commands";
import type { UpdatesView } from "../api/generated/UpdatesView";
import type { Handlers } from "./handlers";
import type { World } from "./scenarios";
import { FIXTURE_NOW, secs } from "./wallet";

export interface UpdatesWorld {
  /** False inside Flatpak. */
  supported: boolean;
  enabled: boolean;
  state: UpdatesView["state"];
  checkedAt: number | null;
  /** What the next check finds: a newer version, `null` for nothing newer, `"fail"` for a failed request. */
  serves: string | null;
  requests: number;
  /** The versions whose release notes were opened. */
  opened: string[];
}

/** Off by default (D-N1), and the latest release is not newer. */
export function newUpdatesWorld(): UpdatesWorld {
  return { supported: true, enabled: false, state: { kind: "off" }, checkedAt: null, serves: null, requests: 0, opened: [] };
}

/** `update_status`. */
export function updatesView({ updates: u }: World): UpdatesView {
  return { supported: u.supported, enabled: u.enabled, state: u.supported ? u.state : { kind: "off" }, checkedAt: u.supported ? u.checkedAt : null };
}

const fail = (code: AppError["code"], message: string): AppError => ({ code, message, data: null });

export function updatesHandlers(): Handlers {
  return {
    update_status: (_, { world }) => updatesView(world),
    set_update_check: ({ enabled }, { world }) => {
      const u = world.updates;
      if (u.enabled !== enabled) Object.assign(u, { enabled, state: { kind: enabled ? "notChecked" : "off" }, checkedAt: null });
      return updatesView(world);
    },
    check_for_update: (_, { world }) => {
      const u = world.updates;
      if (!u.supported) return updatesView(world);
      if (!u.enabled) throw fail("updates.off", "the update check is off (fixture)");
      u.requests += 1;
      u.checkedAt = secs(FIXTURE_NOW);
      u.state =
        u.serves === "fail"
          ? { kind: "failed" }
          : u.serves === null
            ? { kind: "current" }
            : { kind: "available", release: { version: u.serves, publishedAt: secs(FIXTURE_NOW) - 2 * 86_400 } };
      return updatesView(world);
    },
    open_release_notes: (_, { world }) => {
      const s = world.updates.state;
      if (s.kind !== "available") throw fail("app.release_notes_open", "no update is available (fixture)");
      world.updates.opened.push(s.release.version);
      return null;
    },
  };
}

/** The check on, as the user left it: the window checks once at start. */
function on(w: World, u: Partial<UpdatesWorld>): World {
  w.updates = { ...newUpdatesWorld(), enabled: true, state: { kind: "notChecked" }, ...u };
  return w;
}

export function updatesScenarios<T>(base: (t: T) => World) {
  return {
    /** The check is on and finds 0.9.0: About names it, the status bar shows the chip. */
    "update-available": (t: T) => on(base(t), { serves: "0.9.0" }),
    /** The check is on and the latest release is this one. */
    "update-current": (t: T) => on(base(t), {}),
    /** The check is on and GitHub does not answer. */
    "update-error": (t: T) => on(base(t), { serves: "fail" }),
    /** The default: off, no request, although a newer release is out. */
    "update-off": (t: T) => {
      const w = base(t);
      w.updates = { ...newUpdatesWorld(), serves: "0.9.0" };
      return w;
    },
    /** Inside Flatpak, which updates the app: no setting, no request, whatever was saved. */
    "update-flatpak": (t: T) => on(base(t), { supported: false, serves: "0.9.0" }),
  };
}
