// The status bar's voting item (§4.25.1, MG-00 D19, D7): "Oct 10 votes: about 1 d 7 h left" while
// the estimated cutoff is ahead and one of the wallet's voting masternodes can vote. It opens
// Masternodes.

import { navigate } from "../../app/ui";
import { clockView } from "../../governance/model";
import { useGovernanceInfo, useGovernanceSyncState, useProposals, useVotingByProposal } from "../../governance/queries";
import { useNowSecs } from "./common";

export function GovernanceStatusItem({ walletId }: { walletId: string }) {
  const now = useNowSecs();
  const synced = useGovernanceSyncState().data?.phase === "synced";
  const info = useGovernanceInfo();
  const rows = useProposals("current", "", synced).data;
  const voting = useVotingByProposal(walletId, (rows ?? []).map((r) => r.hash), synced);
  const canVote = [...voting.values()].some((nodes) => nodes?.some((n) => n.valid));
  const clock = info.data && clockView(info.data, now);
  if (!synced || !canVote || !clock?.statusBar) return null;
  return (
    <button type="button" className="status-button" data-testid="status-governance" title={clock.statusTooltip} onClick={() => navigate("masternodes")}>
      {clock.statusBar}
    </button>
  );
}
