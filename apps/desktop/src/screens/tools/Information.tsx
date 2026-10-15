// Tools ▸ Information (QT-143, QT-144, QT-040): dash-qt's Information tab over `nodeInfo`, read
// again on every sync status change (D4); the warning banner shows the most severe warning.

import { useEffect, useState } from "react";
import { useBackend, useSyncStatus } from "../../api/queries";
import { useLaunch } from "../../app/App";
import { DetailRow } from "../../components/detail";
import { LoadingState } from "../../components/history";
import { useModel } from "../../onboarding/model";
import { informationSections, InformationModel } from "../../tools/information";
import { NotYet, ToolCard } from "./common";

export function InformationTab() {
  const b = useBackend();
  const launch = useLaunch();
  const [model] = useState(() => new InformationModel(b));
  useModel(model);
  const synced = useSyncStatus().dataUpdatedAt;
  useEffect(() => void model.load(), [model, synced]);
  if (model.error && !model.info)
    return (
      <p className="form-problem" role="alert" data-testid="information-error">
        {model.error}
      </p>
    );
  if (!model.info) return <LoadingState testId="information-loading" text="Reading the node information" />;
  return (
    <div className="tool-stack" data-testid="tools-information">
      {model.banner && (
        <p className="tool-banner" role="status" data-testid="information-banner">
          {model.banner}
        </p>
      )}
      {informationSections(launch, model.info).map((section) => (
        <ToolCard key={section.title} title={section.title} testId={`information-${section.title.toLowerCase().replace(/\W+/g, "-")}`}>
          <dl className="detail-list">
            {section.rows.map((r) => (
              <DetailRow key={r.title} label={r.title}>
                <span className="tool-value">{r.value}</span>
                {r.note && <span className="field-hint tool-note">{r.note}</span>}
              </DetailRow>
            ))}
          </dl>
        </ToolCard>
      ))}
      <NotYet title="DashPay" owner="DP6-03" testId="information-dashpay-not-yet">
        The DashPay sync loops, the last pass and the quorum source.
      </NotYet>
      <NotYet title="Network traffic" owner="M6" testId="information-traffic-not-yet">
        The SPV client reports no byte counters yet.
      </NotYet>
    </div>
  );
}
