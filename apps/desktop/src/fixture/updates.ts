// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
// The fixture's updates (PK-03, dw-app's updates.rs): whether this copy can update itself, what the
// feed serves, and the check, install and restart. No request leaves the fixture; `requests`
// counts the feed reads dw-app would make, so a test can see that none happen while updates are off.

import type { AppError } from "../api/generated/commands";
import type { UpdateAvailability } from "../api/generated/UpdateAvailability";
import type { UpdatePhase } from "../api/generated/UpdatePhase";
import type { UpdateStatus } from "../api/generated/UpdateStatus";
import type { Handlers } from "./handlers";
import type { World } from "./scenarios";
import { FIXTURE_NOW, secs } from "./wallet";

export interface UpdatesWorld {
  availability: UpdateAvailability;
  current: string;
  /** The version the stable feed has; `null`: no release yet. */
  serves: string | null;
  /** The next feed read fails (offline). */
  checkFails: boolean;
  /** The download's signature does not verify against the release key. */
  badSignature: boolean;
  phase: UpdatePhase;
  /** Feed reads made: what would have gone to the network. */
  requests: number;
  restarts: number;
  /** Notes pages opened, by version. */
  notesOpened: string[];
}

/** A build without the updater key, as every build is until the release key exists. */
export function newUpdatesWorld(): UpdatesWorld {
  return {
    availability: "notInThisBuild",
    current: "0.2.0",
    serves: null,
    checkFails: false,
    badSignature: false,
    phase: { kind: "idle" },
    requests: 0,
    restarts: 0,
    notesOpened: [],
  };
}

const fail = (code: AppError["code"], message: string): AppError => ({ code, message, data: null });

const parts = (v: string) => v.split(".").map(Number);

/** `updates::is_offered`: stable, and newer than this build. */
export function isOffered(current: string, release: string): boolean {
  if (!/^\d+\.\d+\.\d+$/.test(release)) return false;
  const [a, b] = [parts(release), parts(current.replace(/[-+].*$/, ""))];
  for (let i = 0; i < 3; i++) if (a[i] !== b[i]) return a[i] > b[i];
  return /[-+]/.test(current);
}

const status = (u: UpdatesWorld): UpdateStatus => ({ availability: u.availability, currentVersion: u.current, phase: u.phase });

function available(u: UpdatesWorld) {
  if (u.availability !== "available") throw fail("app.updates_unavailable", u.availability);
}

function offered(u: UpdatesWorld): string | null {
  const p = u.phase;
  return p.kind === "found" || p.kind === "downloading" || p.kind === "installing" || p.kind === "installed" ? p.version : null;
}

export function updatesHandlers(): Handlers {
  const u = (w: World) => w.updates;
  return {
    update_status: (_, { world }) => status(u(world)),
    check_for_update: (_, { world }) => {
      const s = u(world);
      available(s);
      if (!["idle", "upToDate", "found"].includes(s.phase.kind)) return status(s);
      s.requests++;
      if (s.checkFails) throw fail("app.update_check", "the feed could not be read");
      s.phase = s.serves !== null && isOffered(s.current, s.serves) ? { kind: "found", version: s.serves } : { kind: "upToDate", checkedAt: secs(FIXTURE_NOW) };
      return status(s);
    },
    install_update: ({ version }, { world }) => {
      const s = u(world);
      available(s);
      if (s.phase.kind !== "found" || s.phase.version !== version) throw fail("app.update_state", `install ${version} in ${s.phase.kind}`);
      if (s.badSignature) throw fail("app.update_signature", "the signature does not verify");
      s.phase = { kind: "installed", version };
      return status(s);
    },
    restart_to_update: (_, { world }) => {
      const s = u(world);
      available(s);
      if (s.phase.kind !== "installed") throw fail("app.update_state", `restart in ${s.phase.kind}`);
      s.restarts++;
      return null;
    },
    open_update_notes: (_, { world }) => {
      const s = u(world);
      const version = offered(s);
      if (version === null) throw fail("app.update_state", `notes in ${s.phase.kind}`);
      s.notesOpened.push(version);
      return null;
    },
  };
}
