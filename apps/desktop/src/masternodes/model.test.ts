// My masternodes' view model (model.ts, MG-10 §1.2, §1.7, §1.9, §3.8).

import { createElement, Fragment } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { DEFAULT_PREFS, ui } from "../app/ui";
import { LANGUAGES, t, type Language } from "../i18n";
import type { MasternodeDetail } from "../api/generated/MasternodeDetail";
import type { MasternodeOverview } from "../api/generated/MasternodeOverview";
import type { MasternodePayout } from "../api/generated/MasternodePayout";
import type { MasternodeRow } from "../api/generated/MasternodeRow";
import { CLUB, HOME, newMasternodesWorld, OFFICE, OLD } from "../fixture/masternodes";
import { compact } from "../format/amount";
import { nodeName, voteRows } from "../governance/model";
import {
  activityChip,
  addKeyWaitText,
  attachedLines,
  bannedCount,
  formatCredits,
  groups,
  canAttach,
  heldRoles,
  keyErrorText,
  keyResultText,
  keyRoles,
  latestRequest,
  lastPayoutTitle,
  MnText,
  mnErrorText,
  nodeNames,
  offersAddKey,
  operatorShare,
  payoutGroups,
  registrarLine,
  rowMeta,
  rowName,
  rowSub,
  sharesPayoutAddress,
  statusChip,
  summary,
  trackErrorText,
  untrackKeysText,
  voteBlocked,
} from "./model";

const world = () => newMasternodesWorld();
const rowOf = (hash: string) => world().rows.find((r) => r.proTxHash === hash)!;
const overview = (rows: MasternodeRow[], patch: Partial<MasternodeOverview> = {}): MasternodeOverview => ({
  list: { synced: true, height: 1 },
  masternodes: rows,
  collateralCoins: [],
  payouts30d: rows.filter((r) => r.mine).reduce((s, r) => s + r.payouts30d, 0),
  unattributed30d: 0,
  payoutsIncomplete: false,
  ...patch,
});
const pay = (txid: string, amount: number, unattributed: boolean): MasternodePayout => ({ txid, recordIndex: 0, amount, address: "yX", height: 1, timestamp: 1_760_000_000, matured: true, unattributed });

describe("formatCredits", () => {
  it("shows the Dash value of the credits through the Dash formatter", () => {
    const dash = (d: number) => compact(d, false);
    expect(formatCredits("100000000000", dash)).toBe("1.00");
    expect(formatCredits("42000000000", dash)).toBe("0.42");
    expect(formatCredits("999", dash)).toBe("0.00");
    expect(formatCredits("100000000000", (d) => compact(d, false, "mDASH"))).toBe(compact(100_000_000, false, "mDASH"));
    expect(formatCredits("100000000000", (d) => `${d} duffs`)).toBe("100000000 duffs");
  });
});

describe("rows", () => {
  it("names a row by its label, else by its type and index, a tracked one (index 0) by its hash", () => {
    expect(rowName(rowOf(HOME))).toBe("Home node");
    expect(rowName({ ...rowOf(HOME), label: null, typeIndex: 2 })).toBe("Masternode 2");
    expect(rowName({ ...rowOf(CLUB), typeIndex: 1 })).toBe("Evonode 1");
    expect(rowName({ ...rowOf(CLUB), typeIndex: 0 })).toBe(`Evonode ${CLUB.slice(0, 8)}`);
    expect(rowName({ ...rowOf(CLUB), kind: null, typeIndex: 0 })).toBe(`Masternode ${CLUB.slice(0, 8)}`);
  });

  it("says the service, or that the list doesn't hold it", () => {
    expect(rowSub(rowOf(HOME))).toBe("Masternode · 203.0.113.7:19999");
    expect(rowSub({ ...rowOf(HOME), service: null })).toBe("Masternode · not in the list");
  });

  it("maps every status to MG-00's chip", () => {
    expect(["enabled", "pose_banned", "removed", "unknown", "unconfirmed"].map((s) => statusChip(s as MasternodeRow["status"]).text)).toEqual([
      "Active",
      "PoSe banned",
      "Retired",
      "Unknown",
      "Registering",
    ]);
  });

  it("groups banned own nodes first, then mine, tracked and retired", () => {
    const rows = world().rows;
    expect(groups(rows).map((g) => [g.id, g.rows.map((r) => r.proTxHash)])).toEqual([
      ["attention", [OFFICE]],
      ["mine", [HOME]],
      ["tracked", [CLUB]],
      ["retired", [OLD]],
    ]);
    // Banned without the owner key or the collateral: an ordinary own row, no badge.
    const office = { ...rowOf(OFFICE), roles: { ...rowOf(OFFICE).roles, owner: false, collateral: false, voting: true } };
    expect(groups([office])[0].id).toBe("mine");
    expect(bannedCount(overview(rows))).toBe(1);
    expect(bannedCount(overview([office]))).toBe(0);
    expect(bannedCount(undefined)).toBe(0);
  });

  it("dates a paid meta only from an attributed payout", () => {
    const home = rowOf(HOME);
    expect(rowMeta(home, true, "en-US")).toMatch(/^Paid /);
    expect(rowMeta({ ...home, lastPayout: { ...home.lastPayout!, unattributed: true } }, true)).toBeNull();
    expect(rowMeta(home, false)).toBe(MnText.listSyncing);
    expect(rowMeta(rowOf(OFFICE), true, "en-US")).toMatch(/^noticed /);
    expect(rowMeta({ ...rowOf(CLUB), lastPayout: null }, true)).toBe("Tracked");
  });
});

describe("summary", () => {
  it("counts own live nodes, active and tracked", () => {
    const s = summary(overview(world().rows));
    expect(s).toMatchObject({ count: 2, countNote: "1 active · 1 tracked", unattributed: null, unattributedNote: null });
  });

  it("adds coinbase income the wallet can't match, and the shared-masternode note", () => {
    const s = summary(overview(world().rows, { unattributed30d: 210_000_000, payoutsIncomplete: true }));
    expect(s.unattributed).toBe(210_000_000);
    expect(s.unattributedNote).toBe(`${MnText.unmatched} ${MnText.incomplete}`);
    expect(summary(overview(world().rows, { unattributed30d: 5 })).unattributedNote).toBe(MnText.unmatched);
  });
});

describe("pane", () => {
  it("lists held roles first, watched in a watch-only wallet", () => {
    const office = rowOf(OFFICE);
    expect(heldRoles(office, false).map((h) => [h.text, h.held])).toEqual([
      ["Owner key", true],
      ["Collateral, 1,000 Dash", true],
      ["Voting key", false],
      ["Payout address", false],
      ["Operator key", false],
    ]);
    expect(heldRoles(office, true)[0].text).toBe("Owner key (watched)");
    expect(heldRoles(rowOf(CLUB), false).find((h) => h.id === "collateral")?.text).toBe("Collateral, 4,000 Dash");
  });

  it("says why Vote is off", () => {
    expect(voteBlocked(rowOf(HOME), false)).toBeNull();
    expect(voteBlocked(rowOf(HOME), true)).toBe("Voting needs the wallet that holds the voting keys");
    expect(voteBlocked({ ...rowOf(HOME), status: "pose_banned" }, false)).toBe(MnText.voteBanned);
    expect(voteBlocked(rowOf(OFFICE), false)).toBe(MnText.voteBanned);
    expect(voteBlocked({ ...rowOf(HOME), roles: { ...rowOf(HOME).roles, voting: false } }, false)).toBe(MnText.voteNoKey);
    expect(voteBlocked({ ...rowOf(HOME), status: "removed" }, false)).toBe(MnText.voteRetired);
    expect(voteBlocked({ ...rowOf(HOME), status: "unconfirmed" }, false)).toBe(MnText.voteRegistering);
  });

  it("replaces Change keys with §1.7's reason line, never for available or shared", () => {
    const detail = (registrar: MasternodeDetail["registrar"]) => ({ registrar }) as MasternodeDetail;
    expect(registrarLine(detail({ kind: "available", signer: "keychain" }))).toBeNull();
    expect(registrarLine(detail({ kind: "shared" }))).toBeNull();
    expect(registrarLine(detail({ kind: "no_owner_key" }))).toEqual({ text: "This wallet doesn't hold this masternode's owner key.", unlock: false });
    expect(registrarLine(detail({ kind: "vault_locked" }))).toEqual({ text: "Unlock to check the keys you added.", unlock: true });
    const unavailable = (row: MasternodeRow) => registrarLine({ registrar: { kind: "registration_unavailable" }, row } as MasternodeDetail)?.text;
    const short = "Changing keys needs this masternode's registration, which this wallet doesn't hold.";
    const club = rowOf(CLUB);
    const added = (role: "owner" | "payout" | "voting") => ({ ...club, roles: { ...club.roles, attached: [{ role, keyId: null, check: "unchecked" as const }] } });
    // A regular masternode has no credits, and the app adds no owner or payout key: one sentence.
    expect(unavailable({ ...added("owner"), kind: "regular" })).toBe(short);
    expect(unavailable({ ...club, roles: { ...club.roles, attached: [] } })).toBe(short);
    expect(unavailable({ ...club, roles: { ...club.roles, attached: null } })).toBe(short);
    expect(unavailable(added("voting"))).toBe(short);
    // An evonode with an owner or payout key a dwcli import left can still withdraw its credits.
    expect(unavailable(added("owner"))).toBe(`${short} You can still withdraw evonode credits with an added owner or payout key.`);
    expect(unavailable(added("payout"))).toBe(`${short} You can still withdraw evonode credits with an added owner or payout key.`);
  });

  it("splits 30 days of payouts into attributed and flagged, each with its txids", () => {
    const g = payoutGroups([pay("a", 100, false), pay("b", 50, true), pay("c", 25, false), pay("b", 5, true)]);
    expect(g).toEqual([
      { heading: MnText.payoutsHeading, total: 125, count: 2, txids: ["a", "c"], flagged: false },
      { heading: MnText.flaggedHeading, total: 55, count: 2, txids: ["b"], flagged: true },
    ]);
    expect(payoutGroups([pay("b", 5, true)]).map((x) => x.flagged)).toEqual([true]);
    expect(payoutGroups([])).toEqual([]);
  });

  it("qualifies the last payout and notices a shared payout address", () => {
    expect(lastPayoutTitle(pay("a", 1, false))).toBe("Last payout");
    expect(lastPayoutTitle(pay("a", 1, true))).toBe("Last payment to its payout address");
    const rows = world().rows;
    const home = rows.find((r) => r.proTxHash === HOME)!;
    expect(sharesPayoutAddress(home, rows)).toBe(false);
    expect(sharesPayoutAddress(home, [...rows, { ...rowOf(OFFICE), payoutAddress: home.payoutAddress }])).toBe(true);
    expect(sharesPayoutAddress({ ...home, payoutAddress: null }, [{ ...home, proTxHash: OFFICE, payoutAddress: null }])).toBe(false);
  });

  it("reads Core's operator reward in hundredths of a percent", () => {
    expect(operatorShare(500)).toBe("5% of each payout");
    expect(operatorShare(1250)).toBe("12.5% of each payout");
  });
});

describe("track and keys (MG-10 §1.3, §1.4; MG-00 D13 amended for 1.0)", () => {
  it("offers the voting key only: owner, operator and payout keys stay out of the app (MG ruling 11-24)", () => {
    const plain = rowOf(CLUB);
    // Even with an owner key already attached (a dwcli import), only the voting key is offered.
    const club = { ...plain, roles: { ...plain.roles, attached: [{ role: "owner" as const, keyId: null, check: "unchecked" as const }] } };
    expect(keyRoles(club).map((r) => r.role)).toEqual(["voting"]);
    expect(keyRoles(club)[0]).toEqual({ role: "voting", title: "Voting key", help: "Lets this wallet vote on proposals and usernames with this masternode." });
  });

  it("never offers a second voting key: Remove comes first (Astra r1 1)", () => {
    const club = rowOf(CLUB);
    const withVoting = { ...club, roles: { ...club.roles, attached: [{ role: "voting" as const, keyId: null, check: "matched" as const }] } };
    expect(keyRoles(withVoting)).toEqual([]);
    expect(offersAddKey(withVoting)).toBe(false);
    expect(offersAddKey(club)).toBe(true);
    // Locked, the attached keys are unread: Add keys stays, and the sheet asks for the unlock first.
    expect(offersAddKey({ ...club, roles: { ...club.roles, attached: null } })).toBe(true);
    expect(offersAddKey(rowOf(HOME))).toBe(false);
    expect([canAttach("unlocked"), canAttach("unencrypted"), canAttach("locked"), canAttach("unlockedMixingOnly")]).toEqual([true, true, false, false]);
  });

  it("says Stop tracking deletes the added keys from every wallet here, and that a name (tracked or renamed) is shared", () => {
    expect(t("masternodes.myMasternodes.untrack.keys", { count: 1 })).toBe(
      "The key you added for it is deleted from Dash Wallet on this computer, in every wallet. Keep your own copy if you'll need it again.",
    );
    expect(t("masternodes.myMasternodes.untrack.keys", { count: 2 })).toBe(
      "The 2 keys you added for it are deleted from Dash Wallet on this computer, in every wallet. Keep your own copy if you'll need them again.",
    );
    // A tracked masternode's name is the tracked registry's (tracking.rs `set_label`), which every wallet on
    // this network reads, not this wallet's alone.
    expect(t("masternodes.myMasternodes.track.labelHelp")).toBe("Only Dash Wallet on this computer sees it.");
    // Rename too: an own masternode's name is in app.sqlite's global settings (follow.rs `set_label`), shared alike.
    expect(t("masternodes.myMasternodes.renameHelp")).toBe("Only Dash Wallet on this computer sees it. Leave it empty for the default name.");
  });

  it("keeps only the latest search current, so an earlier query's answer never shows under a later one", () => {
    const searches = latestRequest();
    const a = searches.start();
    const b = searches.start();
    expect([searches.isCurrent(a), searches.isCurrent(b)]).toEqual([false, true]);
    // The field changed after B was sent: B's answer is stale too.
    searches.drop();
    expect(searches.isCurrent(b)).toBe(false);
    expect(searches.isCurrent(searches.start())).toBe(true);
  });

  it("says what each attach answered", () => {
    expect(keyResultText("matched")).toBe("Matches this masternode's voting key.");
    expect(keyResultText("unchecked")).toBe("Saved. This masternode isn't in the masternode list now, so the key can't be checked. It can vote once the key matches.");
    expect(keyResultText("key_mismatch")).toBe("This key doesn't belong to this masternode.");
  });

  it("turns attach and track refusals into sentences, never echoing the key", () => {
    const err = (code: string, message = "the key 33333333 is not this masternode's") => ({ code, message, data: null });
    expect(keyErrorText(err("masternodes.key_mismatch"))).toBe("This key doesn't belong to this masternode.");
    expect(keyErrorText(err("invalid_argument"))).toBe("Enter the private key as a compressed WIF for this network, or as 64 hex characters.");
    expect(keyErrorText(err("vault.locked"))).toBe("Unlock to add a key.");
    expect(trackErrorText(err("invalid_argument"), null)).toBe("Enter an IP address and port, or a 64-character ProTx hash.");
    expect(trackErrorText(err("masternodes.already_tracked"), "Friend's evonode")).toBe("Already tracked as Friend's evonode.");
    expect(trackErrorText(err("masternodes.list_unavailable"), null)).toBe("The masternode list is still loading. Try again in a moment.");
  });

  it("lists every added key, voting included, or nothing while locked", () => {
    const club = rowOf(CLUB);
    const keys = {
      ...club,
      roles: {
        ...club.roles,
        attached: [
          { role: "voting" as const, keyId: null, check: "matched" as const },
          { role: "payout" as const, keyId: null, check: "unchecked" as const },
        ],
      },
    };
    // Each is named as a key, never "Payout address": Remove deletes a private key, and always asks first.
    expect(attachedLines(keys)?.map((l) => [l.text, l.remove, l.confirmTitle])).toEqual([
      ["Voting key · matches", "Remove voting key", "Remove the voting key?"],
      ["Payout key · not checked", "Remove payout key", "Remove the payout key?"],
    ]);
    expect(t("masternodes.myMasternodes.keys.removeText")).toBe(
      "This deletes it from Dash Wallet on this computer, for every wallet. It isn't in your backups, so keep your own copy if you'll need it again.",
    );
    expect(attachedLines({ ...club, roles: { ...club.roles, attached: null } })).toBeNull();
  });

  it("refuses an occupied role in so many words, and waits for the roles before offering a key", () => {
    const e = { code: "masternodes.key_exists", message: "a key is already attached", data: { kind: "masternodeKey" as const, role: "voting" as const } };
    expect(keyErrorText(e)).toBe("A voting key is already added for this masternode on this computer. Remove it first.");
    const club = rowOf(CLUB);
    // Unread (locked or not re-read yet): no key field until the attached keys are known.
    expect(keyRoles({ ...club, roles: { ...club.roles, attached: null } })).toEqual([]);
    // An open sheet the vault could write says why it has no field: still reading, or a voting key came meanwhile.
    expect(addKeyWaitText({ roles: { ...club.roles, attached: null } })).toBe("Loading keys…");
    const voting = { role: "voting" as const, keyId: null, check: "matched" as const };
    expect(addKeyWaitText({ roles: { ...club.roles, attached: [voting] } })).toBe(
      "A voting key is already added for this masternode on this computer. Remove it first.",
    );
    expect(addKeyWaitText({ roles: { ...club.roles, attached: [] } })).toBeNull();
  });

  it("warns that Stop tracking deletes any added keys while their count isn't known", () => {
    const club = rowOf(CLUB);
    const key = { role: "voting" as const, keyId: null, check: "matched" as const };
    expect(untrackKeysText({ roles: { ...club.roles, attached: null } })).toBe(
      "Any keys you added for it are deleted from Dash Wallet on this computer, for every wallet. They aren't in your backups, so keep your own copy if you'll need them again.",
    );
    expect(untrackKeysText({ roles: { ...club.roles, attached: [] } })).toBeNull();
    expect(untrackKeysText({ roles: { ...club.roles, attached: [key, { ...key, role: "payout" as const }] } })).toMatch(/^The 2 keys you added for it are deleted/);
  });

  it("lets a tracked masternode vote once its added voting key holds the role", () => {
    const club = rowOf(CLUB);
    expect(voteBlocked(club, false)).toBe(MnText.voteNoKey);
    expect(voteBlocked({ ...club, roles: { ...club.roles, voting: true } }, false)).toBeNull();
  });
});

describe("Activity and errors", () => {
  it("titles the Activity chip by attribution", () => {
    expect(activityChip("Home node", false)).toBe("Payouts of Home node");
    expect(activityChip("Home node", true)).toBe("Payments to Home node's payout address");
  });

  it("turns masternode codes into §3.8's sentences and keeps unknown messages", () => {
    const e = (code: string, message = "engine text") => ({ code, message, data: null });
    expect(mnErrorText(e("masternodes.update_pending"))).toMatch(/^An earlier change to this masternode/);
    expect(mnErrorText(e("masternodes.in_flight"))).toBe("Another step on this masternode is still running.");
    expect(mnErrorText(e("masternodes.no_owner_key"))).toBe("This wallet doesn't hold this masternode's owner key.");
    expect(mnErrorText(e("masternodes.shared_registration_unknown"))).toBe("This wallet's history doesn't hold this masternode's registration.");
    expect(mnErrorText(e("internal", "fixture: the masternode list read failed"))).toBe("fixture: the masternode list read failed");
    expect(mnErrorText(new Error("boom"))).toBe("boom");
  });
});

describe("governance vote rows (MG-09 F3)", () => {
  it("name a masternode by the wallet's label and the list's service", () => {
    const names = nodeNames(overview(world().rows));
    const home = { proTxHash: HOME, kind: "regular" as const };
    expect(nodeName(home, names)).toBe("Home node · 203.0.113.7:19999");
    expect(nodeName({ proTxHash: CLUB, kind: "evonode" }, names)).toBe(`Evonode ${CLUB.slice(0, 8)} · 198.51.100.21:19999`);
    expect(nodeName({ proTxHash: "ab".repeat(32), kind: "regular" }, names)).toBe("Masternode abababab");
    // Same name as the My masternodes row: an unlabelled own node reads "Masternode N".
    const unlabelled = nodeNames(overview([{ ...rowOf(HOME), label: null, typeIndex: 2 }]));
    expect(nodeName(home, unlabelled)).toBe(`${rowName({ ...rowOf(HOME), label: null, typeIndex: 2 })} · 203.0.113.7:19999`);
    expect(nodeName(home, unlabelled)).toBe("Masternode 2 · 203.0.113.7:19999");
    expect(nodeName(home)).toBe(`Masternode ${HOME.slice(0, 8)}`);
    const rows = voteRows([{ proTxHash: HOME, kind: "regular", weight: 1, valid: true, key: { kind: "keychain", index: 0 }, collateralResolved: true, currentVote: null, voteTime: null, keyHolders: 1, keyLastVote: null, keyLastVoteTime: null }], 0, "en-US", names);
    expect(rows[0].name).toBe("Home node · 203.0.113.7:19999");
  });
});

describe("the masternode list's height", () => {
  it("reads as before in English, and is grouped for the display language", () => {
    expect(MnText.listHeight(2_358_432)).toBe("Masternode list at block 2,358,432");
    expect(MnText.listHeight(999)).toBe("Masternode list at block 999");
    const de: Language = { tag: "de-x-dwtest", name: "Deutsch", dir: "ltr", messages: new Map() };
    LANGUAGES.push(de);
    try {
      ui.set({ prefs: { ...DEFAULT_PREFS, language: "de-x-dwtest" } });
      expect(MnText.listHeight(2_358_432)).toBe("Masternode list at block 2.358.432");
    } finally {
      LANGUAGES.pop();
      ui.set({ prefs: { ...DEFAULT_PREFS, language: null } });
    }
  });
});

describe("a payout group's line", () => {
  it("reads as before: its total, then 1 payment or the count", () => {
    expect([1, 2, 4, 1234].map((count) => t("masternodes.myMasternodes.payoutsIn", { amount: "1.25 DASH", count }))).toEqual([
      "1.25 DASH in 1 payment",
      "1.25 DASH in 2 payments",
      "1.25 DASH in 4 payments",
      "1.25 DASH in 1,234 payments",
    ]);
  });
});

describe("the summary's and groups' counts", () => {
  it("group a count of 1,234 in English", () => {
    const yours = t.rich("masternodes.myMasternodes.summary.yours", { count: 1234, muted: (c) => createElement("span", null, c) });
    expect(renderToStaticMarkup(createElement(Fragment, null, yours))).toBe("1,234 <span>yours</span>");
    expect(t("masternodes.myMasternodes.summary.active", { count: 1234 })).toBe("1,234 active");
    expect(t("masternodes.myMasternodes.summary.activeTracked", { active: 1234, tracked: 1234 })).toBe("1,234 active · 1,234 tracked");
    expect(t("masternodes.myMasternodes.retiredCount", { count: 1234 })).toBe("Retired (1,234)");
  });
});
