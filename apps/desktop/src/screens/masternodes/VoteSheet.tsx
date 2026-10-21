// The vote sheet (§4.25.4, D4, D11–D13): one outcome for the checked masternodes whose voting key
// is in this wallet, a governance grant from the passphrase (MG-05), then `governance_vote`; the
// result lists each masternode's cast, a refusal in the engine's words as a sentence.

import { useMemo, useState } from "react";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import type { ProposalRow } from "../../api/generated/ProposalRow";
import type { VoteCast } from "../../api/generated/VoteCast";
import type { VoteOutcomeName } from "../../api/generated/VoteOutcomeName";
import type { VotingMasternode } from "../../api/generated/VotingMasternode";
import type { WalletSummary } from "../../api/generated/WalletSummary";
import { useBackend } from "../../api/queries";
import { AmountText, DashButton } from "../../components/basics";
import { LoadingState } from "../../components/history";
import { Sheet } from "../../components/sheet";
import { chainOf, useDisplay } from "../../format/display";
import {
  GovText,
  governanceErrorText,
  outcomeText,
  paymentsSuffix,
  presetOutcome,
  selectionNote,
  shortHash,
  statusChip,
  voteResult,
  voteRows,
  votesView,
} from "../../governance/model";
import { useGovernanceInfo, useGovernanceRefresh, useVotingMasternodes } from "../../governance/queries";
import { failureText } from "../pay/model";
import { StatusChip, useNowSecs } from "./common";
import { CredentialField, useCredential } from "./credential";

const OUTCOMES: VoteOutcomeName[] = ["yes", "no", "abstain"];

/** What the result view shows once the votes went out. */
interface VoteDone {
  casts: VoteCast[];
  nodes: VotingMasternode[];
  outcome: VoteOutcomeName;
}

export function VoteSheet({ row, wallet, network, onClose }: { row: ProposalRow; wallet: WalletSummary; network: NetworkInfo; onClose: () => void }) {
  const nodes = useVotingMasternodes(wallet.walletId, row.hash);
  const [result, setResult] = useState<VoteDone | null>(null);
  const [busy, setBusy] = useState(false);
  return (
    <Sheet title={`Vote on ${row.name}`} width="medium" testId="vote-sheet" closable={!busy} onClose={onClose} focusKey={result ? "result" : nodes.data ? "form" : "loading"}>
      {result ? (
        <VoteResult {...result} onDone={onClose} />
      ) : nodes.data ? (
        // A failed re-read (after a vote elsewhere) keeps the form on the last rows.
        <VoteForm row={row} wallet={wallet} network={network} nodes={nodes.data} busy={busy} setBusy={setBusy} onCancel={onClose} onResult={setResult} />
      ) : nodes.isError ? (
        <p className="form-problem" role="alert" data-testid="vote-sheet-load-error">
          {governanceErrorText(nodes.error) ?? failureText(nodes.error, chainOf(network))}
        </p>
      ) : (
        <LoadingState testId="vote-sheet-loading" text="Reading your masternodes…" />
      )}
    </Sheet>
  );
}

function VoteForm({
  row,
  wallet,
  network,
  nodes,
  busy,
  setBusy,
  onCancel,
  onResult,
}: {
  row: ProposalRow;
  wallet: WalletSummary;
  network: NetworkInfo;
  nodes: VotingMasternode[];
  busy: boolean;
  setBusy: (b: boolean) => void;
  onCancel: () => void;
  onResult: (r: VoteDone) => void;
}) {
  const b = useBackend();
  const shown = useDisplay(network);
  const now = useNowSecs();
  const info = useGovernanceInfo();
  const refresh = useGovernanceRefresh();
  const rows = useMemo(() => voteRows(nodes, now), [nodes, now]);
  const [outcome, setOutcome] = useState<VoteOutcomeName | null>(() => presetOutcome(nodes));
  const [checked, setChecked] = useState<ReadonlySet<string>>(() => new Set(rows.filter((r) => r.eligible).map((r) => r.proTxHash)));
  const credential = useCredential(wallet.walletId, "Enter your passphrase to vote.");
  const picked = rows.filter((r) => r.eligible && checked.has(r.proTxHash));
  const toggle = (hash: string) =>
    setChecked((s) => {
      const next = new Set(s);
      if (!next.delete(hash)) next.add(hash);
      return next;
    });
  const threshold = info.data?.passingThreshold;

  const vote = async () => {
    if (busy || outcome === null || picked.length === 0 || !credential.ready) return;
    setBusy(true);
    try {
      const grant = await credential.authorize({ kind: "governance" });
      if (!grant) return;
      const casts = await b.governanceVote(
        wallet.walletId,
        row.hash,
        outcome,
        picked.map((r) => r.proTxHash),
        grant.id,
      );
      void refresh();
      onResult({ casts, nodes, outcome });
    } catch (e) {
      credential.setError(governanceErrorText(e) ?? failureText(e, shown.chain, shown.amountUnit));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form
      className="mg-sheet-body"
      onSubmit={(e) => {
        e.preventDefault();
        void vote();
      }}
    >
      <p className="mg-muted mg-small mg-flush">
        <AmountText text={shown.compact(row.paymentAmount)} unitName={shown.name} /> {paymentsSuffix(row.paymentsRequested)} ·{" "}
        <StatusChip chip={statusChip(row.status, row.closeCall)} /> {threshold !== undefined && votesView(row.tally, threshold).head}
      </p>
      <div className="segmented mg-outcomes" role="group" aria-label="Your vote" data-testid="vote-outcome">
        {OUTCOMES.map((o) => (
          <button
            key={o}
            type="button"
            className="segment"
            aria-pressed={outcome === o}
            data-testid={`vote-outcome-${o}`}
            disabled={busy}
            onClick={() => setOutcome(o)}
          >
            {outcomeText(o)}
          </button>
        ))}
      </div>
      <ul className="mg-nodes card-l1" aria-label="Vote with" data-testid="vote-nodes">
        <li className="mg-node mg-node-head" aria-hidden="true">
          <span />
          <span>Vote with</span>
          <span className="mg-node-weight">Weight</span>
          <span className="mg-node-result">Last vote seen</span>
        </li>
        {rows.map((r) => (
          <li key={r.proTxHash} className="mg-node" aria-disabled={r.eligible ? undefined : true} data-testid={`vote-node-${shortHash(r.proTxHash)}`}>
            <input
              type="checkbox"
              id={`vote-${r.proTxHash}`}
              checked={r.eligible && checked.has(r.proTxHash)}
              disabled={!r.eligible || busy}
              aria-describedby={`vote-${r.proTxHash}-sub`}
              onChange={() => toggle(r.proTxHash)}
            />
            <label htmlFor={`vote-${r.proTxHash}`}>
              <span className="mg-node-name">{r.name}</span>
              <span className="mg-node-sub" id={`vote-${r.proTxHash}-sub`}>
                {r.sub}
              </span>
            </label>
            <span className={`mg-node-weight ${r.eligible ? "" : "mg-muted"}`}>{r.weight}</span>
            <span className="mg-node-result mg-muted">{r.lastSeen}</span>
          </li>
        ))}
      </ul>
      {picked.length > 1 && (
        <p className="mg-notice mg-notice-warning" data-testid="vote-privacy">
          {GovText.privacy}
        </p>
      )}
      <div className="mg-spread mg-vote-bottom">
        <p className="mg-muted mg-small mg-flush mg-vote-note">{GovText.voteNote}</p>
        <CredentialField c={credential} busy={busy} testId="vote" className="mg-vote-credential" />
      </div>
      <footer className="mg-sheet-footer">
        <span className="mg-sheet-footer-note" data-testid="vote-selection">
          {selectionNote(picked)}
        </span>
        <span className="mg-actions">
          <DashButton variant="tintedGray" size="medium" data-testid="vote-cancel" disabled={busy} onClick={onCancel}>
            Cancel
          </DashButton>
          <DashButton
            type="submit"
            variant="filledBlue"
            size="medium"
            data-testid="vote-send"
            disabled={busy || outcome === null || picked.length === 0 || !credential.ready}
          >
            {busy ? "Voting…" : outcome ? `Vote ${outcomeText(outcome)}` : "Vote"}
          </DashButton>
        </span>
      </footer>
    </form>
  );
}

function VoteResult({ casts, nodes, outcome, onDone }: VoteDone & { onDone: () => void }) {
  const view = voteResult(casts, nodes, outcome);
  return (
    <div className="mg-sheet-body" data-testid="vote-result">
      <div className="tx-detail-summary">
        <h3 className="tx-detail-title mg-flush" data-testid="vote-result-title">
          {view.title}
        </h3>
        <span className="tx-detail-date">{view.line}</span>
      </div>
      <ul className="mg-nodes card-l1">
        {view.rows.map((r) => (
          <li key={r.proTxHash} className="mg-node mg-node-result-row" data-testid={`vote-result-${shortHash(r.proTxHash)}`}>
            <span>
              <span className="mg-node-name">{r.name}</span>
              <span className="mg-node-sub">{r.sub}</span>
            </span>
            <StatusChip chip={r.sent ? { text: "Sent", tone: "success", tooltip: "" } : { text: "Not sent", tone: "neutral", tooltip: "" }} />
          </li>
        ))}
      </ul>
      <p className="mg-muted mg-small mg-flush">{GovText.voteCounts}</p>
      <footer className="mg-sheet-footer">
        <span />
        <span className="mg-actions">
          <DashButton variant="filledBlue" size="medium" data-testid="vote-done" data-autofocus onClick={onDone}>
            Done
          </DashButton>
        </span>
      </footer>
    </div>
  );
}
