// Show Masternodes' Auto (MG-00 D1 as amended by MG-10 Q5). Settings ▸ General's hint states exactly
// what Auto tests, so a change to `autoShowsMasternodes` must change this file and the hint together.

import { describe, expect, it } from "vitest";
import type { CollateralCoin } from "../api/generated/CollateralCoin";
import type { MasternodeOverview } from "../api/generated/MasternodeOverview";
import type { MasternodeRow } from "../api/generated/MasternodeRow";
import type { MyProposal } from "../api/generated/MyProposal";
import type { MyProposalState } from "../api/generated/MyProposalState";
import type { ProTxOperation } from "../api/generated/ProTxOperation";
import { settings } from "../i18n/catalog/settings";
import { autoShowsMasternodes } from "./queries";

const overview = (masternodes: Partial<MasternodeRow>[] = [], collateralCoins: Partial<CollateralCoin>[] = []) =>
  ({ masternodes, collateralCoins }) as unknown as MasternodeOverview;
const proposal = (kind: MyProposalState["kind"]) => ({ state: { kind } }) as unknown as MyProposal;
const op = (kind: ProTxOperation["kind"], state: string) => ({ kind, state: { kind: state } }) as unknown as ProTxOperation;

describe("Show Masternodes' Auto", () => {
  it("shows the item for exactly the conditions its hint names", () => {
    expect(settings.general.showMasternodesHint).toBe(
      "Shows Masternodes in the sidebar, where you set up and follow masternodes and vote on proposals and usernames. Auto shows it while this wallet holds a masternode or one of its keys, tracks a masternode, holds a coin of exactly 1,000 or 4,000 Dash, has a registration in progress, or has a proposal in progress.",
    );
    // "holds a masternode or one of its keys": mn_list's rows carry every role the wallet holds.
    expect(autoShowsMasternodes(overview([{ mine: true, tracked: false }]), [], [])).toBe(true);
    // "tracks a masternode"
    expect(autoShowsMasternodes(overview([{ mine: false, tracked: true }]), [], [])).toBe(true);
    // "holds a coin of exactly 1,000 or 4,000 Dash": the engine's collateral-shaped coins.
    expect(autoShowsMasternodes(overview([], [{ amount: 1_000 * 100_000_000 }]), [], [])).toBe(true);
    // "has a registration in progress": a ProRegTx not yet confirmed or conflicted.
    expect(autoShowsMasternodes(overview(), [], [op("registration", "sent")])).toBe(true);
    // "has a proposal in progress"
    expect(autoShowsMasternodes(overview(), [proposal("confirming")], [])).toBe(true);
  });

  it("hides it otherwise, and while the reads have no answer", () => {
    expect(autoShowsMasternodes(overview(), [], [])).toBe(false);
    expect(autoShowsMasternodes(overview(), [proposal("expired"), proposal("feeFailed")], [])).toBe(false);
    expect(autoShowsMasternodes(overview(), [], [op("registration", "confirmed"), op("registration", "conflicted"), op("update_registrar", "sent")])).toBe(false);
    expect(autoShowsMasternodes(undefined, undefined, undefined)).toBe(false);
  });
});
