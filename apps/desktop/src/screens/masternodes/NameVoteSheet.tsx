// The username vote sheet (§4.25.4 for names, MG-11 D10–D15): one choice (a contender, Abstain or
// Lock) for the checked masternodes, a governance grant from the passphrase (MG-05), then
// `governance_vote_name`; the result lists each masternode's vote, an `unconfirmed` one as not
// confirmed until Check again re-reads Platform and finds its choice there (MG-07 D7).

import { useMemo, useState } from "react";
import type { ContestedName } from "../../api/generated/ContestedName";
import type { ContestedNames } from "../../api/generated/ContestedNames";
import type { NameChoice } from "../../api/generated/NameChoice";
import type { NameVoteCast } from "../../api/generated/NameVoteCast";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import type { WalletSummary } from "../../api/generated/WalletSummary";
import { useBackend } from "../../api/queries";
import { DashButton } from "../../components/basics";
import { Sheet } from "../../components/sheet";
import { useDisplay } from "../../format/display";
import { selectionNote, shortHash } from "../../governance/model";
import {
  choiceKey,
  choiceOptions,
  defaultVoters,
  endsText,
  lastVoteNotice,
  nameErrorText,
  NameText,
  nameVoterRows,
  nameVoteResult,
  pickedVoters,
  sameChoice,
  voteButtonText,
} from "../../governance/names";
import { failureText } from "../pay/model";
import { StatusChip, useNowSecs } from "./common";
import { CredentialField, useCredential } from "./credential";

/** What the result shows once the votes went out. */
interface NameVoteDone {
  casts: NameVoteCast[];
  choice: NameChoice;
}

export function NameVoteSheet({
  contest,
  names,
  wallet,
  network,
  onClose,
  reread,
}: {
  contest: ContestedName;
  names: ContestedNames;
  wallet: WalletSummary;
  network: NetworkInfo;
  onClose: () => void;
  /** Re-reads the contests (the tab's query): after a vote, and for Check again. */
  reread: () => Promise<ContestedNames | undefined>;
}) {
  const [result, setResult] = useState<NameVoteDone | null>(null);
  const [busy, setBusy] = useState(false);
  return (
    <Sheet title={`Vote on ${contest.label}`} width="medium" testId="name-vote-sheet" closable={!busy} onClose={onClose} focusKey={result ? "result" : "form"}>
      {result ? (
        <NameVoteResult {...result} contest={contest} names={names} reread={reread} onDone={onClose} />
      ) : (
        <NameVoteForm
          contest={contest}
          names={names}
          wallet={wallet}
          network={network}
          busy={busy}
          setBusy={setBusy}
          onCancel={onClose}
          onResult={(r) => {
            void reread();
            setResult(r);
          }}
          onRefused={() => void reread()}
        />
      )}
    </Sheet>
  );
}

function NameVoteForm({
  contest,
  names,
  wallet,
  network,
  busy,
  setBusy,
  onCancel,
  onResult,
  onRefused,
}: {
  contest: ContestedName;
  names: ContestedNames;
  wallet: WalletSummary;
  network: NetworkInfo;
  busy: boolean;
  setBusy: (b: boolean) => void;
  onCancel: () => void;
  onResult: (r: NameVoteDone) => void;
  onRefused: () => void;
}) {
  const b = useBackend();
  const shown = useDisplay(network);
  const now = useNowSecs();
  const { label } = contest;
  const allowed = names.votesAllowed;
  // No choice is preset: the present one would only be refused as `already_voted` (D10).
  const [choice, setChoice] = useState<NameChoice | null>(null);
  const [checked, setChecked] = useState<ReadonlySet<string>>(() => defaultVoters(names.voters, label, allowed));
  const rows = useMemo(() => nameVoterRows(names.voters, label, choice, allowed), [names.voters, label, choice, allowed]);
  const picked = pickedVoters(rows, checked);
  const last = lastVoteNotice(picked, names.voters, label, allowed);
  const credential = useCredential(wallet.walletId, "Enter your passphrase to vote.");
  const toggle = (hash: string) =>
    setChecked((s) => {
      const next = new Set(s);
      if (!next.delete(hash)) next.add(hash);
      return next;
    });

  const vote = async () => {
    if (busy || choice === null || picked.length === 0 || !credential.ready) return;
    setBusy(true);
    try {
      const grant = await credential.authorize({ kind: "governance" });
      if (!grant) return;
      const casts = await b.governanceVoteName(
        wallet.walletId,
        label,
        choice,
        picked.map((r) => r.proTxHash),
        grant.id,
      );
      onResult({ casts, choice });
    } catch (e) {
      credential.setError(nameErrorText(e) ?? failureText(e, shown.chain, shown.amountUnit));
      // A closed contest or a contender gone: the list re-reads under the sheet.
      onRefused();
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
      <p className="mg-muted mg-small mg-flush">{endsText(contest, now)}</p>
      <fieldset className="mg-choices" disabled={busy} data-testid="name-vote-choices">
        <legend className="mg-section-title">Your vote</legend>
        {choiceOptions(contest).map((o) => {
          const key = choiceKey(o.choice);
          return (
            <label key={key} className="mg-choice" data-testid={`name-choice-${key}`}>
              <input type="radio" name="name-choice" value={key} checked={sameChoice(choice, o.choice)} onChange={() => setChoice(o.choice)} />
              <span className="mg-choice-text">
                <span className={o.identity ? "mono" : undefined} title={o.identity ?? undefined}>
                  {o.title}
                </span>
                <span className="mg-muted mg-small">
                  {o.choice.kind === "lock" ? "Nobody gets the name" : o.choice.kind === "abstain" ? "Counted, for no one" : "Gets the name"} · {o.votes.toLocaleString("en-US")}{" "}
                  {o.votes === 1 ? "vote" : "votes"}
                </span>
              </span>
            </label>
          );
        })}
      </fieldset>
      {!contest.contendersComplete && <p className="mg-muted mg-small mg-flush">{NameText.partial}</p>}
      <ul className="mg-nodes card-l1" aria-label="Vote with" data-testid="name-vote-nodes">
        <li className="mg-node mg-node-head" aria-hidden="true">
          <span />
          <span>Vote with</span>
          <span className="mg-node-weight">Weight</span>
          <span className="mg-node-result">Its vote now</span>
        </li>
        {rows.map((r) => (
          <li key={r.proTxHash} className="mg-node" aria-disabled={r.eligible ? undefined : true} data-testid={`name-vote-node-${shortHash(r.proTxHash)}`}>
            <input
              type="checkbox"
              id={`name-vote-${r.proTxHash}`}
              checked={r.eligible && checked.has(r.proTxHash)}
              disabled={!r.eligible || busy}
              aria-describedby={`name-vote-${r.proTxHash}-sub`}
              onChange={() => toggle(r.proTxHash)}
            />
            <label htmlFor={`name-vote-${r.proTxHash}`}>
              <span className="mg-node-name">{r.name}</span>
              <span className="mg-node-sub" id={`name-vote-${r.proTxHash}-sub`}>
                {r.sub}
              </span>
            </label>
            <span className={`mg-node-weight ${r.eligible ? "" : "mg-muted"}`}>{r.weight}</span>
            <span className="mg-node-result mg-muted">{r.present}</span>
          </li>
        ))}
      </ul>
      {picked.length > 1 && (
        <p className="mg-notice mg-notice-warning" data-testid="name-vote-privacy">
          {NameText.privacy}
        </p>
      )}
      {last && (
        <p className="mg-notice mg-notice-warning" data-testid="name-vote-last">
          {last}
        </p>
      )}
      <div className="mg-spread mg-vote-bottom">
        <p className="mg-muted mg-small mg-flush mg-vote-note">{NameText.limitNote(allowed)}</p>
        <CredentialField c={credential} busy={busy} testId="name-vote" className="mg-vote-credential" />
      </div>
      <footer className="mg-sheet-footer">
        <span className="mg-sheet-footer-note" data-testid="name-vote-selection">
          {selectionNote(picked)}
        </span>
        <span className="mg-actions">
          <DashButton variant="tintedGray" size="medium" data-testid="name-vote-cancel" disabled={busy} onClick={onCancel}>
            Cancel
          </DashButton>
          <DashButton type="submit" variant="filledBlue" size="medium" data-testid="name-vote-send" disabled={busy || choice === null || picked.length === 0 || !credential.ready}>
            {busy ? "Voting…" : voteButtonText(choice)}
          </DashButton>
        </span>
      </footer>
    </form>
  );
}

function NameVoteResult({
  casts,
  choice,
  contest,
  names,
  reread,
  onDone,
}: NameVoteDone & {
  contest: ContestedName;
  names: ContestedNames;
  reread: () => Promise<ContestedNames | undefined>;
  onDone: () => void;
}) {
  // Only an explicit re-read can turn an `unconfirmed` vote into a counted one (D14).
  const [after, setAfter] = useState<ContestedNames | undefined>(undefined);
  const [checking, setChecking] = useState(false);
  const view = nameVoteResult(casts, names.voters, contest.label, choice, after);
  const check = async () => {
    setChecking(true);
    try {
      setAfter((await reread()) ?? after);
    } finally {
      setChecking(false);
    }
  };
  return (
    <div className="mg-sheet-body" data-testid="name-vote-result">
      <div className="tx-detail-summary">
        <h3 className="tx-detail-title mg-flush" data-testid="name-vote-result-title">
          {view.title}
        </h3>
        <span className="tx-detail-date">{view.line}</span>
      </div>
      <ul className="mg-nodes card-l1">
        {view.rows.map((r) => (
          <li key={r.proTxHash} className="mg-node mg-node-result-row" data-testid={`name-vote-result-${shortHash(r.proTxHash)}`}>
            <span>
              <span className="mg-node-name">{r.name}</span>
              <span className="mg-node-sub">{r.sub}</span>
            </span>
            <StatusChip chip={r.chip} />
          </li>
        ))}
      </ul>
      <p className="mg-muted mg-small mg-flush">{NameText.counts}</p>
      <footer className="mg-sheet-footer">
        <span />
        <span className="mg-actions">
          {view.unconfirmed && (
            <DashButton variant="tintedGray" size="medium" data-testid="name-vote-check" disabled={checking} onClick={() => void check()}>
              {checking ? "Checking…" : "Check again"}
            </DashButton>
          )}
          <DashButton variant="filledBlue" size="medium" data-testid="name-vote-done" data-autofocus onClick={onDone}>
            Done
          </DashButton>
        </span>
      </footer>
    </div>
  );
}
