// Settings ▸ About ▸ Updates (PK-03): whether this copy can update itself, "Check for updates
// automatically" (off until the user turns it on), "Check for updates", and a found update's
// install and restart, each on a click. dw-app reads the feed only when one of these asks.

import { useEffect } from "react";
import type { Backend } from "../api/backend";
import { hasCode } from "../api/backend";
import type { UiSettings } from "../api/generated/UiSettings";
import type { UpdateAvailability } from "../api/generated/UpdateAvailability";
import type { UpdatePhase } from "../api/generated/UpdatePhase";
import type { UpdateStatus } from "../api/generated/UpdateStatus";
import { ui } from "../app/ui";
import { showToast } from "../components/toast";
import { t } from "../i18n";
import { failureText, Model } from "../onboarding/model";

export type UpdatesApi = Pick<Backend, "updateStatus" | "checkForUpdate" | "installUpdate" | "restartToUpdate" | "openUpdateNotes" | "openReleasesPage">;

/** The first automatic check waits this long after launch, then one runs a day. */
export const AUTO_CHECK_DELAY_MS = 60_000;
export const AUTO_CHECK_EVERY_MS = 24 * 60 * 60 * 1000;

/** What the Updates card shows: the phase, once this copy can update itself. */
export type UpdatesView = { kind: "loading" } | { kind: "unavailable"; reason: Exclude<UpdateAvailability, "available"> } | UpdatePhase;

export function updatesView(status: UpdateStatus | null): UpdatesView {
  if (!status) return { kind: "loading" };
  return status.availability === "available" ? status.phase : { kind: "unavailable", reason: status.availability };
}

/** Why this copy does not update itself. */
export function unavailableText(reason: Exclude<UpdateAvailability, "available">): string {
  return t(`settings.updates.unavailable.${reason}`);
}

export function updateFailureText(e: unknown): string {
  if (hasCode(e, "app.update_busy")) return t(`settings.updates.busy.${e.data?.kind === "updateBusy" ? e.data.reason : "unknown"}`);
  if (hasCode(e, "app.update_check")) return t("settings.updates.failed.check");
  if (hasCode(e, "app.update_not_published")) return t("settings.updates.failed.notPublished");
  if (hasCode(e, "app.update_download")) return t("settings.updates.failed.download");
  if (hasCode(e, "app.update_signature")) return t("settings.updates.failed.signature");
  if (hasCode(e, "app.update_install")) return t("settings.updates.failed.install");
  if (hasCode(e, "app.update_state")) return t("settings.updates.failed.state");
  if (hasCode(e, "app.browser_open")) return t("settings.updates.failed.browser");
  return failureText(e);
}

/** The Windows installer replaces the running app, so Install closes it there. */
export function installClosesApp(userAgent: string = typeof navigator === "undefined" ? "" : navigator.userAgent): boolean {
  return /Windows/i.test(userAgent);
}

export class UpdatesModel extends Model {
  status: UpdateStatus | null = null;
  problem: string | null = null;
  busy = false;

  constructor(
    private readonly api: UpdatesApi,
    private readonly pollMs = 500,
  ) {
    super();
  }

  get view(): UpdatesView {
    return updatesView(this.status);
  }

  /** Runs `f`, showing its failure as the card's problem. */
  private async attempt(f: () => Promise<unknown>): Promise<boolean> {
    this.problem = null;
    this.changed();
    try {
      await f();
      return true;
    } catch (e) {
      this.problem = updateFailureText(e);
      return false;
    } finally {
      this.changed();
    }
  }

  async load() {
    await this.attempt(async () => (this.status = await this.api.updateStatus()));
  }

  /** Reads the feed once; the found version when this check found one that was not on offer. */
  async check(): Promise<string | null> {
    if (this.busy) return null;
    const before = this.view;
    this.busy = true;
    await this.attempt(async () => (this.status = await this.api.checkForUpdate()));
    this.busy = false;
    this.changed();
    const after = this.view;
    return after.kind === "found" && (before.kind !== "found" || before.version !== after.version) ? after.version : null;
  }

  /** Downloads (unless it is downloaded already), verifies and installs the version on offer, showing
   * the download's progress. */
  async install() {
    const view = this.view;
    if (this.busy || (view.kind !== "found" && view.kind !== "downloaded") || !this.status) return;
    this.busy = true;
    const phase: UpdatePhase =
      view.kind === "found" ? { kind: "downloading", version: view.version, received: 0, total: null } : { kind: "installing", version: view.version };
    this.status = { ...this.status, phase };
    const poll = setInterval(() => {
      void this.api.updateStatus().then(
        (s) => {
          if (this.busy && (s.phase.kind === "downloading" || s.phase.kind === "installing")) {
            this.status = s;
            this.changed();
          }
        },
        () => undefined,
      );
    }, this.pollMs);
    const done = await this.attempt(async () => (this.status = await this.api.installUpdate(view.version)));
    clearInterval(poll);
    if (!done) this.status = await this.api.updateStatus().catch(() => this.status);
    if (this.view.kind === "restartRequired") this.problem = null;
    this.busy = false;
    this.changed();
  }

  async restart() {
    const kind = this.view.kind;
    if (this.busy || (kind !== "installed" && kind !== "restartRequired")) return;
    await this.attempt(() => this.api.restartToUpdate());
  }

  async openReleases() {
    await this.attempt(() => this.api.openReleasesPage());
  }

  /** The page of the release on offer. */
  async openNotes() {
    await this.attempt(() => this.api.openUpdateNotes());
  }
}

const models = new WeakMap<UpdatesApi, UpdatesModel>();

/** One model per backend: the Settings card, the automatic check and the restart notice share it. */
export function updatesModel(api: UpdatesApi): UpdatesModel {
  let model = models.get(api);
  if (!model) {
    model = new UpdatesModel(api);
    models.set(api, model);
  }
  return model;
}

/** One automatic check: only when the user turned it on and this copy can update itself. Returns
 * the version it found, when that is news. */
export async function automaticCheck(model: UpdatesModel, prefs: Pick<UiSettings, "checkUpdatesAutomatically">): Promise<string | null> {
  if (!prefs.checkUpdatesAutomatically) return null;
  if (!model.status) await model.load();
  if (model.status?.availability !== "available") return null;
  return model.check();
}

/** The automatic check: a minute after launch, then once a day, each one only when the user has it
 * on. A newly found version says so in a toast. */
export function useAutomaticUpdateCheck(api: UpdatesApi) {
  useEffect(() => {
    const tick = () =>
      void automaticCheck(updatesModel(api), ui.get().prefs).then((version) => {
        if (version) showToast(t("settings.updates.foundToast", { version }), 8);
      });
    const first = setTimeout(tick, AUTO_CHECK_DELAY_MS);
    const every = setInterval(tick, AUTO_CHECK_EVERY_MS);
    return () => {
      clearTimeout(first);
      clearInterval(every);
    };
  }, [api]);
}
