// Settings ▸ About (QT-018, QT-153, IOS-107): the versions, the network and where its data is, and
// updates (PK-03).
// Help ▸ About keeps U-01's small sheet. Command-line options (D22) and support (U-05) are not
// shown yet.

import { useBackupDirectory, useNodeInfo } from "../../api/queries";
import { useLaunch } from "../../app/App";
import { CopyValue, DetailRow } from "../../components/detail";
import { useT } from "../../i18n";
import { NotYetRow, pendingText, SettingsCard } from "./parts";
import { UpdatesCard } from "./Updates";

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
            {launch.appVersion}
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
      <UpdatesCard />
      <SettingsCard title={t("settings.about.more")} testId="settings-about-more">
        <NotYetRow testId="settings-command-line" title={t("settings.about.commandLine")} why={t("settings.about.commandLineWhy")} />
        <NotYetRow testId="settings-support" title={t("settings.about.support")} why={t("settings.about.supportWhy")} />
      </SettingsCard>
    </>
  );
}
