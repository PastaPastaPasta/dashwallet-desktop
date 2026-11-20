// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
// The fixture's updates (PK-03, dw-app's updates.rs): whether this copy can update itself, what the
// feed serves, and the check, install and restart. No request leaves the fixture; `requests`
// counts the feed reads dw-app would make, so a test can see that none happen while updates are off.
// The running version is the launch's (`appVersion`), as in dw-app.

import type { AppError } from "../api/generated/commands";
import type { UpdateAvailability } from "../api/generated/UpdateAvailability";
import type { UpdateBusyReason } from "../api/generated/UpdateBusyReason";
import type { UpdatePhase } from "../api/generated/UpdatePhase";
import type { UpdateStatus } from "../api/generated/UpdateStatus";
import { installClosesApp } from "../settings/updates";
import type { Handlers } from "./handlers";

/** What goes wrong next: the feed is unreachable or unpublished, the download's signature does not
 * verify, the installer fails (`closed`: after the engine shut down for it, as on Windows), or
 * something runs that Install and Restart wait for. */
export type UpdateFault = "offline" | "unpublished" | "signature" | "install" | "closed" | UpdateBusyReason;

export const UPDATE_FAULTS: UpdateFault[] = ["offline", "unpublished", "signature", "install", "closed", "startup", "payment", "mixing", "platform"];

export interface UpdatesWorld {
  availability: UpdateAvailability;
  /** The version the stable feed has; `null`: no release yet. */
  serves: string | null;
  fault: UpdateFault | null;
  phase: UpdatePhase;
  /** Feed reads made: what would have gone to the network. */
  requests: number;
  restarts: number;
  /** The versions whose release page Release notes opened. */
  notesOpened: string[];
}

/** A build without the updater key, as every build is until the release key exists. */
export function newUpdatesWorld(): UpdatesWorld {
  return { availability: "notInThisBuild", serves: null, fault: null, phase: { kind: "idle" }, requests: 0, restarts: 0, notesOpened: [] };
}

const fail = (code: AppError["code"], message: string, data: AppError["data"] = null): AppError => ({ code, message, data });

const parts = (v: string) => v.split(".").map(Number);

/** `updates::is_offered` for a stable running version: stable, and newer. */
export function isOffered(current: string, release: string): boolean {
  if (!/^\d+\.\d+\.\d+$/.test(release)) return false;
  const [a, b] = [parts(release), parts(current)];
  for (let i = 0; i < 3; i++) if (a[i] !== b[i]) return a[i] > b[i];
  return false;
}

const status = ({ availability, phase }: UpdatesWorld): UpdateStatus => ({ availability, phase });

function available(u: UpdatesWorld) {
  if (u.availability !== "available") throw fail("app.updates_unavailable", u.availability);
}

function notBusy(u: UpdatesWorld) {
  const f = u.fault;
  if (f === "startup" || f === "payment" || f === "mixing" || f === "platform") throw fail("app.update_busy", f, { kind: "updateBusy", reason: f });
}

export function updatesHandlers(): Handlers {
  return {
    update_status: (_, { world: { updates } }) => status(updates),
    check_for_update: (_, { world }) => {
      const u = world.updates;
      available(u);
      if (!["idle", "upToDate", "found"].includes(u.phase.kind)) return status(u);
      u.requests++;
      if (u.fault === "offline") throw fail("app.update_check", "the feed could not be read");
      if (u.fault === "unpublished") throw fail("app.update_not_published", "404");
      u.phase = u.serves !== null && isOffered(world.launch.appVersion, u.serves) ? { kind: "found", version: u.serves } : { kind: "upToDate" };
      return status(u);
    },
    // Only where installing closes the engine (Windows) does it wait for what runs, as in dw-app.
    install_update: ({ version }, { world: { updates: u } }) => {
      available(u);
      if ((u.phase.kind !== "found" && u.phase.kind !== "downloaded") || u.phase.version !== version) {
        throw fail("app.update_state", `install ${version} in ${u.phase.kind}`);
      }
      if (installClosesApp()) notBusy(u);
      if (u.fault === "signature") throw fail("app.update_signature", "the signature does not verify");
      if (u.fault === "install") throw fail("app.update_install", "the update could not be put in place");
      if (u.fault === "closed") {
        u.phase = { kind: "restartRequired", version };
        throw fail("app.update_install", "the installer did not start");
      }
      u.phase = { kind: "installed", version };
      return status(u);
    },
    restart_to_update: (_, { world: { updates: u } }) => {
      available(u);
      if (u.phase.kind !== "restartRequired") notBusy(u);
      if (u.phase.kind !== "installed" && u.phase.kind !== "restartRequired") throw fail("app.update_state", `restart in ${u.phase.kind}`);
      u.restarts++;
      return null;
    },
    open_update_notes: (_, { world: { updates: u } }) => {
      available(u);
      if (!("version" in u.phase)) throw fail("app.update_state", `release notes in ${u.phase.kind}`);
      u.notesOpened.push(u.phase.version);
      return null;
    },
  };
}

/** The phase a screenshot or a test starts in (`updatePhase=<kind>`), for the version the feed serves. */
export function startingPhase(kind: UpdatePhase["kind"], version: string): UpdatePhase {
  switch (kind) {
    case "idle":
    case "checking":
    case "upToDate":
      return { kind };
    case "downloading":
      return { kind, version, received: 47_185_920, total: 123_423_224 };
    default:
      return { kind, version };
  }
}
