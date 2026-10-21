// Governance numbers… (QT-134, MG-00 D20, §4.25.3): this cycle's heights and dates, the voting
// weight, the funding line and the budget, from `info` (the counts once synced).

import type { GovernanceInfo } from "../../api/generated/GovernanceInfo";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import { DashButton } from "../../components/basics";
import { DetailRow } from "../../components/detail";
import { Sheet } from "../../components/sheet";
import { useDisplay } from "../../format/display";
import { numbersRows } from "../../governance/model";
import { useGovernanceSyncState, useProposals } from "../../governance/queries";

export function NumbersSheet({ info, network, onClose }: { info: GovernanceInfo; network: NetworkInfo; onClose: () => void }) {
  const shown = useDisplay(network);
  const sync = useGovernanceSyncState();
  const rows = useProposals("current", "", sync.data?.phase === "synced").data;
  const { cycle, votes } = numbersRows(info, sync.data, rows, (d) => `${shown.compact(d)} ${shown.name}`);
  return (
    <Sheet title="Governance numbers" width="medium" testId="numbers-sheet" onClose={onClose}>
      <div className="mg-sheet-body">
        <h3 className="mg-section-title mg-flush">This cycle</h3>
        <dl className="detail-list card-l1" data-testid="numbers-cycle">
          {cycle.map((r) => (
            <DetailRow key={r.label} label={r.label}>
              {r.value}
            </DetailRow>
          ))}
        </dl>
        <h3 className="mg-section-title mg-flush">Votes and budget</h3>
        <dl className="detail-list card-l1" data-testid="numbers-votes">
          {votes.map((r) => (
            <DetailRow key={r.label} label={r.label}>
              {r.value}
            </DetailRow>
          ))}
        </dl>
        <footer className="mg-sheet-footer">
          <span />
          <span className="mg-actions">
            <DashButton variant="filledBlue" size="medium" data-testid="numbers-done" data-autofocus onClick={onClose}>
              Done
            </DashButton>
          </span>
        </footer>
      </div>
    </Sheet>
  );
}
