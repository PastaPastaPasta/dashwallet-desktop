// The selected proposal (§4.25.3 detail pane): its chip and note, what it asks for, where it pays,
// its link (shown and copied, D9), the weighted votes with their range, the wallet's voting
// masternodes, and Vote (the page opens the vote sheet, D31).

import type { GovernanceInfo } from "../../api/generated/GovernanceInfo";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import type { ProposalRow } from "../../api/generated/ProposalRow";
import type { VotingMasternode } from "../../api/generated/VotingMasternode";
import type { WalletSummary } from "../../api/generated/WalletSummary";
import { AmountText, DashButton } from "../../components/basics";
import { CopyIconButton } from "../../components/copy";
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
import { useT, type Translate } from "../../i18n";
import { useNodeNames } from "../../masternodes/queries";
import { MoreMenu, StatusChip, useNowSecs, VoteBar } from "./common";

/** Why Vote is disabled, or `null` (D18, §4.25.4). */
function voteBlocked(t: Translate, row: ProposalRow, nodes: VotingMasternode[] | undefined, watchOnly: boolean): string | null {
  if (watchOnly) return GovText.voteWatchOnly;
  if (isEnded(row)) return t("masternodes.governance.pane.ended");
  if (nodes === undefined) return t("masternodes.governance.pane.reading");
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
  const t = useT();
  const shown = useDisplay(network);
  const now = useNowSecs();
  const votes = votesView(row.tally, info.passingThreshold);
  const blocked = voteBlocked(t, row, nodes, wallet.watchOnly);
  const names = useNodeNames(wallet.walletId);
  const copy = (text: string) => () => void copyWithToast(text);
  return (
    <aside className="contacts-pane" aria-label={t("masternodes.governance.pane.label")} data-testid="proposal-pane">
      <section className="mg-pane card-l1">
        <div>
          <h2 className="mg-pane-title">{row.name}</h2>
          <div className="chip-row mg-pane-chip">
            <StatusChip chip={statusChip(row.status, row.closeCall)} testId="proposal-pane-chip" />
            <span className="mg-muted mg-small">{paneNote(row, info)}</span>
          </div>
        </div>
        <dl className="mg-facts">
          <dt>{t("masternodes.governance.pane.asksFor")}</dt>
          <dd>
            {t.rich("masternodes.governance.ask", { amount: <AmountText text={shown.compact(row.paymentAmount)} unitName={shown.name} />, payments: askSpan(row) })}
          </dd>
          <dt>{t("masternodes.governance.pane.paidTo")}</dt>
          <dd className="copy-value">
            <span className="mono" title={row.paymentAddress}>
              {shortAddress(row.paymentAddress)}
            </span>
            <CopyIconButton text={row.paymentAddress} label={t("masternodes.governance.pane.copyAddress")} />
          </dd>
          <dt>{t("masternodes.governance.pane.link")}</dt>
          <dd className="copy-value">
            <span className="mg-pane-link" title={GovText.linkNote} data-testid="proposal-pane-link">
              {linkText(row.url)}
            </span>
            <CopyIconButton text={row.url} label={t("masternodes.governance.pane.copyLink")} />
          </dd>
        </dl>
        <div className="mg-stack mg-pane-votes">
          <div className="mg-spread mg-small">
            <strong>{t("masternodes.governance.pane.votesWeighted")}</strong>
            <span className="mg-muted">{votes.head}</span>
          </div>
          <VoteBar tally={row.tally} threshold={info.passingThreshold} weight={info.validWeight} />
          <div className="mg-vote-legend">
            <span className="l-yes">{votes.legend.yes}</span>
            <span className="l-no">{votes.legend.no}</span>
            <span className="l-abstain">{votes.legend.abstain}</span>
            <span className="l-threshold">{t("masternodes.governance.numbersSheet.fundingLine")}</span>
          </div>
          <p className="mg-muted mg-small mg-flush">{votes.range}</p>
        </div>
        {nodes && nodes.length > 0 && (
          <div>
            <h3 className="mg-section-title">{t("masternodes.governance.pane.yourMasternodes")}</h3>
            <ul className="mg-roles mg-roles-votes" data-testid="proposal-pane-masternodes">
              {nodes.map((n) => (
                <li key={n.proTxHash} className="mg-roles-pair">
                  <span>
                    {nodeName(n, names)}
                    {n.weight > 1 && <span className="mg-muted"> {t("masternodes.governance.pane.weight", { weight: String(n.weight) })}</span>}
                  </span>
                  <span className="mg-muted">{n.valid ? lastVoteLine(n, now) : t("masternodes.governance.pane.banned")}</span>
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
            {t("masternodes.governance.pane.vote")}
          </DashButton>
          <MoreMenu
            testId="proposal-more"
            items={[
              { id: "copy-link", title: t("masternodes.governance.pane.copyLink"), onSelect: copy(row.url) },
              { id: "copy-hash", title: t("masternodes.governance.pane.copyHash"), onSelect: copy(row.hash) },
            ]}
          />
        </div>
        {blocked && <p className="mg-muted mg-small mg-flush" data-testid="proposal-vote-blocked">{blocked}</p>}
      </section>
    </aside>
  );
}
