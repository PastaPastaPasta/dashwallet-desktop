// Settings ▸ About ▸ Updates (PK-03): what this copy can do about updates, the automatic check
// (off by default) and a found update's install and restart, each on a click (settings/updates.ts).

import { useEffect, useId, useState } from "react";
import { useBackend } from "../../api/queries";
import { DashButton } from "../../components/basics";
import { useT } from "../../i18n";
import { useModel } from "../../onboarding/model";
import { installClosesApp, unavailableText, updateFailureText, updatesModel, type UpdatesView } from "../../settings/updates";
import { useSavePrefs } from "./General";
import { Hint, Problem, SettingsCard } from "./parts";

export function UpdatesCard() {
  const t = useT();
  const backend = useBackend();
  const model = useModel(updatesModel(backend));
  const { prefs, save, problem: notSaved } = useSavePrefs();
  const [opening, setOpening] = useState<string | null>(null);
  const hint = useId();
  useEffect(() => void model.load(), [model]);
  const view = model.view;
  const version = model.status?.currentVersion;

  const openReleases = () => {
    setOpening(null);
    void backend.openReleasesPage().catch((e: unknown) => setOpening(updateFailureText(e)));
  };
  const turn = async (on: boolean) => {
    if ((await save({ checkUpdatesAutomatically: on })) && on) await model.check();
  };

  return (
    <SettingsCard title={t("settings.updates.title")} testId="settings-updates">
      {version && (
        <p className="settings-row-title" data-testid="settings-updates-version">
          {t("settings.updates.current", { version })}
        </p>
      )}
      {view.kind === "unavailable" ? (
        <div className="settings-row" data-testid={`settings-updates-unavailable-${view.reason}`}>
          <Hint>{unavailableText(view.reason)}</Hint>
          {view.reason !== "flatpak" && (
            <DashButton variant="plainBlue" size="small" data-testid="settings-updates-releases" onClick={openReleases}>
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
          <UpdateState view={view} busy={model.busy} onInstall={() => void model.install()} onRestart={() => void model.restart()} onNotes={() => void model.openNotes()} />
          {view.kind !== "installing" && view.kind !== "installed" && (
            <div className="settings-row">
              <DashButton variant="tintedBlue" size="small" data-testid="settings-updates-check" disabled={model.busy} onClick={() => void model.check()}>
                {t("settings.updates.check")}
              </DashButton>
            </div>
          )}
        </>
      ) : null}
      <Problem text={model.problem ?? notSaved ?? opening} testId="settings-updates-problem" />
    </SettingsCard>
  );
}

function UpdateState({ view, busy, onInstall, onRestart, onNotes }: { view: UpdatesView; busy: boolean; onInstall: () => void; onRestart: () => void; onNotes: () => void }) {
  const t = useT();
  const closes = installClosesApp();
  const line = (text: string) => (
    <p className="field-hint" role="status" data-testid={`settings-updates-${view.kind}`}>
      {text}
    </p>
  );
  switch (view.kind) {
    case "checking":
      return line(t("settings.updates.checking"));
    case "upToDate":
      return line(t("settings.updates.upToDate"));
    case "found":
      return (
        <>
          {line(t("settings.updates.found", { version: view.version }))}
          {closes && <Hint>{t("settings.updates.installClosesApp")}</Hint>}
          <div className="settings-row">
            <DashButton variant="plainBlue" size="small" data-testid="settings-updates-notes" onClick={onNotes}>
              {t("settings.updates.notes")}
            </DashButton>
            <DashButton variant="filledBlue" size="small" data-testid="settings-updates-install" disabled={busy} onClick={onInstall}>
              {closes ? t("settings.updates.installAndRestart") : t("settings.updates.install")}
            </DashButton>
          </div>
        </>
      );
    case "installing":
      return line(
        view.percent === 100
          ? t("settings.updates.installing", { version: view.version })
          : view.percent === null
            ? t("settings.updates.downloading", { version: view.version })
            : t("settings.updates.downloadingPercent", { version: view.version, percent: view.percent }),
      );
    case "installed":
      return (
        <>
          {line(t("settings.updates.installed", { version: view.version }))}
          <Hint>{t("settings.updates.restartHint")}</Hint>
          <div className="settings-row">
            <DashButton variant="filledBlue" size="small" data-testid="settings-updates-restart" onClick={onRestart}>
              {t("settings.updates.restart")}
            </DashButton>
          </div>
        </>
      );
    default:
      return null;
  }
}
