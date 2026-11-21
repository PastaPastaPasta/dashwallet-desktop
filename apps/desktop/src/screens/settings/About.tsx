// Settings ▸ About (QT-018, QT-153, IOS-107): the versions, the network and where its data is, and
// updates (PK-03).
// Help ▸ About keeps U-01's small sheet. Command-line options (D22) and support (U-05) are not
// shown yet.

import { useBackupDirectory, useNodeInfo } from "../../api/queries";
import { useLaunch } from "../../app/App";
import { CopyValue, DetailRow } from "../../components/detail";
import { NotYetRow, pendingText, SettingsCard } from "./parts";
import { UpdatesCard } from "./Updates";

export function AboutTab() {
  const launch = useLaunch();
  const node = useNodeInfo();
  const backups = useBackupDirectory();
  return (
    <>
      <SettingsCard title="Dash Wallet" testId="settings-about">
        <dl className="detail-list settings-details">
          <DetailRow label="Version" testId="settings-about-version">
            {launch.appVersion}
          </DetailRow>
          <DetailRow label="Engine" testId="settings-about-engine">
            {launch.engineVersion}
          </DetailRow>
          <DetailRow label="Network" testId="settings-about-network">
            {launch.network.dirName}
          </DetailRow>
          <DetailRow label="User agent" testId="settings-about-agent">
            {node.data ? node.data.userAgent : pendingText(node)}
          </DetailRow>
          <DetailRow label="Data directory" testId="settings-about-data-dir">
            {node.data ? <CopyValue value={node.data.dataDir} label="data directory" mono /> : pendingText(node)}
          </DetailRow>
          <DetailRow label="Backup folder" testId="settings-about-backups">
            {backups.data ? <CopyValue value={backups.data} label="backup folder" mono /> : pendingText(backups)}
          </DetailRow>
        </dl>
      </SettingsCard>
      <UpdatesCard />
      <SettingsCard title="More" testId="settings-about-more">
        <NotYetRow testId="settings-command-line" title="Command-line options" why="These options are not available here yet." />
        <NotYetRow testId="settings-support" title="Support, source code and log export" why="Support and log export are coming soon." />
      </SettingsCard>
    </>
  );
}
