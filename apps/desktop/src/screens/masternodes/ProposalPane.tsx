// The selected proposal (§4.25.3 detail pane): its chip and note, what it asks for, where it pays,
// its link (shown and copied, D9), the weighted votes with their range, the wallet's voting
// masternodes, and Vote (the page opens the vote sheet, D31).

import type { GovernanceInfo } from "../../api/generated/GovernanceInfo";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import type { ProposalRow } from "../../api/generated/ProposalRow";
import type { VotingMasternode } from "../../api/generated/VotingMasternode";
import type { WalletSummary } from "../../api/generated/WalletSummary";
import { AmountText, DashButton, Icon } from "../../components/basics";
import { copyWithToast } from "../../components/toast";
import { useDisplay } from "../../format/display";
import {
  askSpan,
  GovText,
  isEnded,
  lastVoteLine,
  linkText,
  nodeName,
  paneNote,
  shortAddress,
  statusChip,
  votesView,
} from "../../governance/model";
import { MoreMenu, StatusChip, useNowSecs, VoteBar } from "./common";

/** Why Vote is disabled, or `null` (D18, §4.25.4). */
function voteBlocked(row: ProposalRow, nodes: VotingMasternode[] | undefined, watchOnly: boolean): string | null {
  if (watchOnly) return GovText.voteWatchOnly;
  if (isEnded(row)) return "This proposal has ended";
  if (nodes === undefined) return "Reading your masternodes…";
  if (!nodes.some((n) => n.valid)) return GovText.noVotingKey;
  return null;
}

export function ProposalPane({
  row,
  nodes,
  info,
  wallet,
  network,
  onVote,
}: {
  row: ProposalRow;
  nodes: VotingMasternode[] | undefined;
  info: GovernanceInfo;
  wallet: WalletSummary;
  network: NetworkInfo;
  onVote: () => void;
}) {
  const shown = useDisplay(network);
  const now = useNowSecs();
  const votes = votesView(row.tally, info.passingThreshold);
  const blocked = voteBlocked(row, nodes, wallet.watchOnly);
  const copy = (text: string) => () => void copyWithToast(text);
  return (
    <aside className="contacts-pane" aria-label="Proposal" data-testid="proposal-pane">
      <section className="mg-pane card-l1">
        <div>
          <h2 className="mg-pane-title">{row.name}</h2>
          <div className="chip-row mg-pane-chip">
            <StatusChip chip={statusChip(row.status, row.closeCall)} testId="proposal-pane-chip" />
            <span className="mg-muted mg-small">{paneNote(row, info)}</span>
          </div>
        </div>
        <dl className="mg-facts">
          <dt>Asks for</dt>
          <dd>
            <AmountText text={shown.compact(row.paymentAmount)} unitName={shown.name} /> {askSpan(row)}
          </dd>
          <dt>Paid to</dt>
          <dd className="copy-value">
            <span className="mono" title={row.paymentAddress}>
              {shortAddress(row.paymentAddress)}
            </span>
            <button type="button" className="copy-button" aria-label="Copy address" title="Copy address" onClick={copy(row.paymentAddress)}>
              <Icon id="copy" size={20} />
            </button>
          </dd>
          <dt>Link</dt>
          <dd className="copy-value">
            <span className="mg-pane-link" title={GovText.linkNote} data-testid="proposal-pane-link">
              {linkText(row.url)}
            </span>
            <button type="button" className="copy-button" aria-label="Copy link" title="Copy link" onClick={copy(row.url)}>
              <Icon id="copy" size={20} />
            </button>
          </dd>
        </dl>
        <div className="mg-stack mg-pane-votes">
          <div className="mg-spread mg-small">
            <strong>Votes, weighted</strong>
            <span className="mg-muted">{votes.head}</span>
          </div>
          <VoteBar tally={row.tally} threshold={info.passingThreshold} weight={info.validWeight} />
          <div className="mg-vote-legend">
            <span className="l-yes">{votes.legend.yes}</span>
            <span className="l-no">{votes.legend.no}</span>
            <span className="l-abstain">{votes.legend.abstain}</span>
            <span className="l-threshold">Funding line</span>
          </div>
          <p className="mg-muted mg-small mg-flush">{votes.range}</p>
        </div>
        {nodes && nodes.length > 0 && (
          <div>
            <h3 className="mg-section-title">Your masternodes</h3>
            <ul className="mg-roles mg-roles-votes" data-testid="proposal-pane-masternodes">
              {nodes.map((n) => (
                <li key={n.proTxHash} className="mg-roles-pair">
                  <span>
                    {nodeName(n)}
                    {n.weight > 1 && <span className="mg-muted"> · weight {n.weight}</span>}
                  </span>
                  <span className="mg-muted">{n.valid ? lastVoteLine(n, now) : "PoSe banned"}</span>
                </li>
              ))}
            </ul>
          </div>
        )}
        <div className="mg-actions">
          <DashButton
            variant="filledBlue"
            size="small"
            data-testid="proposal-vote"
            disabled={blocked !== null}
            title={blocked ?? undefined}
            onClick={onVote}
          >
            Vote
          </DashButton>
          <MoreMenu
            testId="proposal-more"
            items={[
              { id: "copy-link", title: "Copy link", onSelect: copy(row.url) },
              { id: "copy-hash", title: "Copy proposal hash", onSelect: copy(row.hash) },
            ]}
          />
        </div>
        {blocked && <p className="mg-muted mg-small mg-flush" data-testid="proposal-vote-blocked">{blocked}</p>}
      </section>
    </aside>
  );
}
