// Mine (§4.25.3, MG-06 D24, D15–D17): the wallet's own proposals in MG-06's states. [Submit] shows
// only where the engine acts on it; a slow one says "Sending…" and keeps the row; its failure is
// said on the row.

import { useState } from "react";
import type { MyProposal } from "../../api/generated/MyProposal";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import { useBackend } from "../../api/queries";
import { DashButton } from "../../components/basics";
import { EmptyState } from "../../components/history";
import { chainOf } from "../../format/display";
import { GovText, governanceErrorText, mineRow } from "../../governance/model";
import { useGovernanceRefresh, useTurnOnGovernance } from "../../governance/queries";
import { failureText } from "../pay/model";
import { StatusChip } from "./common";

export function MineList({ walletId, network, mine, syncOff = false }: { walletId: string; network: NetworkInfo; mine: MyProposal[]; syncOff?: boolean }) {
  if (mine.length === 0) return <EmptyState testId="mine-empty" title={GovText.emptyMine} message={GovText.mineNote} />;
  return (
    <>
      {syncOff && <h2 className="mg-section-title">Mine</h2>}
      <section className="tx-group card-l1">
        <ul className="mg-list" data-testid="mine-list">
          {mine.map((p) => (
            <MineRow key={p.hash} walletId={walletId} network={network} p={p} />
          ))}
        </ul>
      </section>
      <p className="mg-muted mg-small">{GovText.mineNote}</p>
    </>
  );
}

function MineRow({ walletId, network, p }: { walletId: string; network: NetworkInfo; p: MyProposal }) {
  const b = useBackend();
  const refresh = useGovernanceRefresh();
  const turnOn = useTurnOnGovernance();
  const view = mineRow(p);
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState<string | null>(null);
  const submit = async () => {
    if (busy) return;
    setBusy(true);
    setFailed(null);
    try {
      await b.governanceSubmitProposal(walletId, p.hash);
    } catch (e) {
      setFailed(governanceErrorText(e) ?? failureText(e, chainOf(network)));
    } finally {
      setBusy(false);
      void refresh();
    }
  };
  const error = failed ?? view.error;
  return (
    <li className="mg-row mg-row-wide mg-mine-row" data-testid={`mine-row-${p.name}`}>
      <span className="mg-row-text">
        <span className="mg-row-title">{p.name}</span>
        <span className="mg-row-sub">{view.sub}</span>
        {error && (
          <span className="mg-row-sub mg-mine-error" role="status" data-testid={`mine-error-${p.name}`}>
            {error}
          </span>
        )}
      </span>
      <span className="mg-row-trailing">
        <StatusChip chip={view.chip} testId={`mine-chip-${p.name}`} />
        {view.meta && <span className="mg-row-meta">{view.meta}</span>}
      </span>
      {view.submit && (
        <DashButton variant="tintedBlue" size="small" data-testid={`mine-submit-${p.name}`} disabled={busy} onClick={() => void submit()}>
          {busy ? GovText.submitting : "Submit"}
        </DashButton>
      )}
      {view.turnOn && (
        <DashButton variant="tintedBlue" size="small" data-testid={`mine-turn-on-${p.name}`} onClick={() => void turnOn()}>
          {GovText.turnOn}
        </DashButton>
      )}
    </li>
  );
}
