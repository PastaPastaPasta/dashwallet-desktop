import { useEffect, useId } from "react";
import { useBackend } from "../../api/queries";
import { DashButton, DashLogo } from "../../components/basics";
import { useT } from "../../i18n";
import { useModel } from "../../onboarding/model";
import { installClosesApp, unavailableText, updatesModel, type UpdatesModel, type UpdatesView } from "../../settings/updates";
import { useSavePrefs } from "./General";
import { Hint, Problem, SettingsCard } from "./parts";

export function UpdatesCard() {
  const t = useT();
  const model = useModel(updatesModel(useBackend()));
  const { prefs, save, problem: notSaved } = useSavePrefs();
  const hint = useId();
  useEffect(() => void model.load(), [model]);
  const view = model.view;

  const turn = async (on: boolean) => {
    if ((await save({ checkUpdatesAutomatically: on })) && on) await model.check();
  };

  const problem = model.problem ?? notSaved;
  return (
    <SettingsCard title={t("settings.updates.title")} testId="settings-updates">
      {view.kind === "unavailable" ? (
        <div className="settings-row" data-testid={`settings-updates-unavailable-${view.reason}`}>
          <Hint>{unavailableText(view.reason)}</Hint>
          {view.reason !== "flatpak" && (
            <DashButton variant="plainBlue" size="small" data-testid="settings-updates-releases" onClick={() => void model.openReleases()}>
              {t("settings.updates.releasesPage")}
            </DashButton>
          )}
        </div>
      ) : view.kind !== "loading" ? (
        <>
          <label className="checkbox-row">
            <input
              type="checkbox"
              data-testid="settings-updates-auto"
              checked={prefs.checkUpdatesAutomatically}
              aria-describedby={hint}
              onChange={(e) => void turn(e.target.checked)}
            />
            {t("settings.updates.auto")}
          </label>
          <Hint id={hint}>{t("settings.updates.autoHint")}</Hint>
          {(view.kind === "idle" || view.kind === "checking" || view.kind === "upToDate" || view.kind === "found") && (
            <div className="settings-row">
              <DashButton variant="tintedBlue" size="small" data-testid="settings-updates-check" disabled={model.busy} onClick={() => void model.check()}>
                {t("settings.updates.check")}
              </DashButton>
            </div>
          )}
          <div className="updates-feedback">
            {!(problem && view.kind === "upToDate") && <UpdateState view={view} model={model} />}
            <Problem text={problem} testId="settings-updates-problem" />
          </div>
        </>
      ) : (
        <Problem text={problem} testId="settings-updates-problem" />
      )}
    </SettingsCard>
  );
}

function UpdateState({ view, model }: { view: UpdatesView; model: UpdatesModel }) {
  const t = useT();
  const closes = installClosesApp();
  const line = (text: string) => (
    <p className="field-hint" role="status" data-testid={`settings-updates-${view.kind}`}>
      {text}
    </p>
  );
  const restart = (
    <div className="settings-row">
      <DashButton variant="filledBlue" size="small" data-testid="settings-updates-restart" onClick={() => void model.restart()}>
        {t("settings.updates.restart")}
      </DashButton>
    </div>
  );
  switch (view.kind) {
    case "checking":
      return line(t("settings.updates.checking"));
    case "upToDate":
      return line(t("settings.updates.upToDate"));
    case "found":
    case "downloaded":
      return (
        <>
          {line(t(view.kind === "found" ? "settings.updates.found" : "settings.updates.downloaded", { version: view.version }))}
          {closes && <Hint>{t("settings.updates.installClosesApp")}</Hint>}
          <div className="settings-row">
            <DashButton variant="plainBlue" size="small" data-testid="settings-updates-notes" onClick={() => void model.openNotes()}>
              {t("settings.updates.notes")}
            </DashButton>
            <DashButton variant="filledBlue" size="small" data-testid="settings-updates-install" disabled={model.busy} onClick={() => void model.install()}>
              {closes ? t("settings.updates.installAndRestart") : t("settings.updates.install")}
            </DashButton>
          </div>
        </>
      );
    case "downloading": {
      const { version, received, total } = view;
      return line(
        total ? t("settings.updates.downloadingPercent", { version, percent: Math.min(100, Math.floor((received * 100) / total)) }) : t("settings.updates.downloading", { version }),
      );
    }
    case "installing":
      return line(t("settings.updates.installing", { version: view.version }));
    case "installed":
      return (
        <>
          {line(t("settings.updates.installed", { version: view.version }))}
          <Hint>{t("settings.updates.restartHint")}</Hint>
          {restart}
        </>
      );
    case "restartRequired":
      return (
        <>
          {line(t("settings.updates.restartRequired"))}
          {restart}
        </>
      );
    default:
      return null;
  }
}

/** Over the whole app once a failed Windows install has closed the engine: the wallet screens
 * cannot work until Dash Wallet restarts, so they say only that. */
export function UpdateRestartNotice() {
  const t = useT();
  const model = useModel(updatesModel(useBackend()));
  if (model.view.kind !== "restartRequired") return null;
  return (
    <div className="onboarding pre-release" role="alertdialog" aria-modal="true" aria-labelledby="update-restart-title" aria-describedby="update-restart-body" data-testid="update-restart-notice">
      <div className="onboarding-panel">
        <header className="onboarding-header">
          <span className="onboarding-header-side" />
          <span className="onboarding-brand">
            <DashLogo className="onboarding-logo" />
          </span>
          <span className="onboarding-header-side" />
        </header>
        <main className="onboarding-body">
          <h1 className="onboarding-title" id="update-restart-title">
            {t("settings.updates.restartRequiredTitle")}
          </h1>
          <p className="onboarding-text" id="update-restart-body">
            {t("settings.updates.restartRequired")}
          </p>
          <DashButton variant="filledBlue" size="large" fill autoFocus data-testid="update-restart-now" onClick={() => void model.restart()}>
            {t("settings.updates.restart")}
          </DashButton>
          <Problem text={model.problem} testId="update-restart-problem" />
        </main>
      </div>
    </div>
  );
}
