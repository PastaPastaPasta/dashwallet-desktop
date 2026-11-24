// Settings ▸ About (QT-018, QT-153, IOS-107): the versions, the network and where its data is, and
// updates (PK-03), and the license (UX-SPEC §4.18).
// Help ▸ About keeps U-01's small sheet. Command-line options (D22) are not shown yet; support is a
// link to where problems are reported.

import { useId, useState } from "react";
import type { ProjectLinkName } from "../../api/generated/ProjectLinkName";
import { isAppError, type Backend } from "../../api/backend";
import { Model, useModel } from "../../onboarding/model";
import { useBackend, useBackupDirectory, useNodeInfo } from "../../api/queries";
import { DashButton } from "../../components/basics";
import { useLaunch } from "../../app/App";
import { CopyValue, DetailRow } from "../../components/detail";
import { useT } from "../../i18n";
import { NotYetRow, pendingText, SettingsCard } from "./parts";
import { UpdatesCard } from "./Updates";

const LINKS: ProjectLinkName[] = ["issues", "sourceCode", "website"];

/** The sentence with its `{url}` as selectable text and a copy button, wherever a translation puts it. */
export function FailedLink({ url }: { url: string }) {
  const t = useT();
  return <>{t.rich("settings.about.links.failed", { url: <CopyValue value={url} action={t("receive.dash.copyLink")} /> })}</>;
}

/** The URL a failed `open_project_link` names; null for a failure without one. */
export const failedLinkUrl = (e: unknown) => (isAppError(e) && e.data?.kind === "link" ? e.data.url : null);

/** The links, the warning under Report a problem, and a failed open's link to copy. */
export function ProjectLinksView({ failed, onOpen }: { failed: { url: string | null } | null; onOpen: (link: ProjectLinkName) => void }) {
  const t = useT();
  const hintId = useId();
  return (
    <SettingsCard title={t("settings.about.links.title")} testId="settings-about-links">
      {LINKS.map((link) => (
        <div key={link} className="settings-row">
          <DashButton
            variant="plainBlue"
            data-testid={`settings-link-${link}`}
            aria-describedby={link === "issues" ? `${hintId}-issues` : undefined}
            onClick={() => onOpen(link)}
          >
            {t(`settings.about.links.${link}`)}
          </DashButton>
          {link === "issues" && (
            <p id={`${hintId}-issues`} className="field-hint settings-hint" data-testid="settings-link-issues-hint">
              {t("settings.about.links.issuesHint")}
            </p>
          )}
        </div>
      ))}
      {failed && (
        <p className="form-problem" role="alert" data-testid="settings-link-failed">
          {failed.url === null ? t("onboarding.error.generic") : <FailedLink url={failed.url} />}
        </p>
      )}
    </SettingsCard>
  );
}

/** Opens a project page; only the latest click's failure shows, so an earlier click that fails
 * late can't bring back its alert after a later one worked. */
export class LinkOpener extends Model {
  failed: { url: string | null } | null = null;
  #latest = 0;

  constructor(private readonly api: Pick<Backend, "openProjectLink">) {
    super();
  }

  async open(link: ProjectLinkName) {
    const request = ++this.#latest;
    this.failed = null;
    this.changed();
    try {
      await this.api.openProjectLink(link);
    } catch (e) {
      if (request !== this.#latest) return;
      this.failed = { url: failedLinkUrl(e) };
      this.changed();
    }
  }
}

/** Support and the project's pages, opened in the browser by Rust (the renderer has no opener). */
function ProjectLinksCard() {
  const backend = useBackend();
  const [opener] = useState(() => new LinkOpener(backend));
  useModel(opener);
  return <ProjectLinksView failed={opener.failed} onOpen={(link) => void opener.open(link)} />;
}

export function AboutTab() {
  const t = useT();
  const launch = useLaunch();
  const node = useNodeInfo();
  const backups = useBackupDirectory();
  return (
    <>
      <SettingsCard title={t("settings.about.title")} testId="settings-about">
        <dl className="detail-list settings-details">
          <DetailRow label={t("settings.about.version")} testId="settings-about-version">
            {launch.buildVersion}
          </DetailRow>
          <DetailRow label={t("settings.about.engine")} testId="settings-about-engine">
            {launch.engineVersion}
          </DetailRow>
          <DetailRow label={t("settings.about.network")} testId="settings-about-network">
            {launch.network.dirName}
          </DetailRow>
          <DetailRow label={t("settings.about.userAgent")} testId="settings-about-agent">
            {node.data ? node.data.userAgent : pendingText(node)}
          </DetailRow>
          <DetailRow label={t("settings.about.dataDir")} testId="settings-about-data-dir">
            {node.data ? <CopyValue value={node.data.dataDir} action={t("settings.about.copyDataDir")} mono /> : pendingText(node)}
          </DetailRow>
          <DetailRow label={t("settings.about.backups")} testId="settings-about-backups">
            {backups.data ? <CopyValue value={backups.data} action={t("settings.about.copyBackups")} mono /> : pendingText(backups)}
          </DetailRow>
        </dl>
      </SettingsCard>
      <SettingsCard title={t("settings.about.license.title")} testId="settings-about-license">
        <p className="field-hint">{t("settings.about.license.intro")}</p>
        <pre className="license-text" tabIndex={0} data-testid="settings-about-license-text">
          {t("settings.about.license.text")}
        </pre>
        <p className="field-hint">{t("settings.about.license.thirdParty")}</p>
      </SettingsCard>
      <UpdatesCard />
      <SettingsCard title={t("settings.about.more")} testId="settings-about-more">
        <NotYetRow testId="settings-command-line" title={t("settings.about.commandLine")} why={t("settings.about.commandLineWhy")} />
      </SettingsCard>
      <ProjectLinksCard />
    </>
  );
}
