// The instant username while the own contest is open (DP1-09 D4 reversed, DP1-10 D13): the vote's
// length per network, what became of a registration's temporary name (the guards on a names read),
// the offer's conditions and the Join page's gate.

import { describe, expect, it } from "vitest";
import type { IdentitySummary } from "../api/generated/IdentitySummary";
import type { RegistrationStatus } from "../api/generated/RegistrationStatus";
import { contestRow, INSTANT_COPY, instantOffer, registrationOffered, temporaryOutcome, VOTE_COPY, VOTE_POLL_MS, voteDuration, type InstantInputs, type NamesRead } from "./instantOffer";

const ROW_AT = 1_800_000_000;

const row = (r: Partial<RegistrationStatus> = {}): RegistrationStatus => ({
  draft: "reg-1",
  phase: "contested",
  label: "alice",
  temporaryLabel: "alice2",
  identity: "A",
  txid: null,
  waiting: null,
  holdsKey: false,
  running: false,
  fundsCommitted: true,
  contestEndsAt: null,
  failure: null,
  nameFailure: null,
  profileFailure: null,
  invitation: null,
  fromPlatform: false,
  createdAt: ROW_AT - 60,
  updatedAt: ROW_AT,
  ...r,
});

const summary = (s: Partial<IdentitySummary> = {}): IdentitySummary => ({
  identity: "A",
  index: 0,
  names: [],
  mainName: null,
  namesUpdating: false,
  namesCheck: null,
  isMain: true,
  balance: "5000000000",
  balanceAsOf: null,
  balanceStale: false,
  hasDashpayKeys: true,
  profile: null,
  unverified: false,
  creditLevel: "ok",
  ...s,
});

/** A read that started `afterSecs` after the row last changed, answering `s`. */
const read = (afterSecs: number, s: Partial<IdentitySummary> = {}): NamesRead => ({ startedAt: (ROW_AT + afterSecs) * 1000, identities: [summary(s)] });

describe("voteDuration: Platform's vote poll per network (dpp_voting_versions v2)", () => {
  it("mainnet: two weeks", () => {
    expect(VOTE_POLL_MS.mainnet).toBe(1_209_600_000);
    expect(voteDuration("mainnet")).toBe("2 weeks");
  });

  it("testnet: 90 minutes", () => {
    expect(VOTE_POLL_MS.testnet).toBe(5_400_000);
    expect(voteDuration("testnet")).toBe("1 hour 30 minutes");
  });

  it("devnet: 90 minutes", () => {
    expect(VOTE_POLL_MS.devnet).toBe(5_400_000);
    expect(voteDuration("devnet")).toBe("1 hour 30 minutes");
  });

  it("regtest: 90 minutes", () => {
    expect(VOTE_POLL_MS.regtest).toBe(5_400_000);
    expect(voteDuration("regtest")).toBe("1 hour 30 minutes");
  });

  it("a network without a value: the existing copy's \"about two weeks\", never a blank", () => {
    expect(voteDuration("testnet", { mainnet: 1_209_600_000 })).toBe("about two weeks");
    expect(INSTANT_COPY.offer(voteDuration("devnet", {}))[0]).toContain("voted on for about two weeks.");
  });

  it("goes into the offer's body, the MG copy verbatim", () => {
    expect(INSTANT_COPY.offer(voteDuration("mainnet"))).toEqual([
      "Your requested username will be voted on for 2 weeks. For immediate contacts and payments, you can create a similar instant username.",
      "If your request is approved, it becomes your main username and the instant one stays yours too, with your contacts and history kept. If not, keep using the instant one without interruption.",
    ]);
  });
});

describe("temporaryOutcome: only a names read started after the row reached Contested", () => {
  it("a read started after the row changed, without the name: failed", () => {
    expect(temporaryOutcome(row(), read(1))).toBe("failed");
    expect(temporaryOutcome(row({ phase: "done" }), read(60))).toBe("failed");
  });

  it("propagation lag: the first read (in flight at the change) lacks the name, the next has it; no failure ever", () => {
    expect(temporaryOutcome(row(), read(-2))).toBeNull();
    expect(temporaryOutcome(row(), read(0))).toBeNull();
    expect(temporaryOutcome(row(), read(3, { names: ["alice2"], mainName: "alice2" }))).toBe("owned");
  });

  it("a read started in the same second as the change says nothing (the row's time is whole seconds)", () => {
    expect(temporaryOutcome(row(), { startedAt: ROW_AT * 1000 + 999, identities: [summary()] })).toBeNull();
    expect(temporaryOutcome(row(), { startedAt: (ROW_AT + 1) * 1000, identities: [summary()] })).toBe("failed");
  });

  it("a failed read (none answered) says nothing, neither owned nor failed", () => {
    expect(temporaryOutcome(row(), undefined)).toBeNull();
  });

  it("names not settled say nothing", () => {
    expect(temporaryOutcome(row(), read(5, { namesUpdating: true }))).toBeNull();
    expect(temporaryOutcome(row(), read(5, { namesCheck: "pending" }))).toBeNull();
    expect(temporaryOutcome(row(), read(5, { namesCheck: "failed" }))).toBeNull();
    expect(temporaryOutcome(row(), { startedAt: (ROW_AT + 5) * 1000, identities: [] })).toBeNull();
  });

  it("owned under DPNS's homograph folding, even from a stale read", () => {
    expect(temporaryOutcome(row(), read(-10, { names: ["AL1CE2"] }))).toBe("owned");
  });

  it("nothing for a row that asked for none or has not reached its contest", () => {
    expect(temporaryOutcome(row({ temporaryLabel: null }), read(5))).toBeNull();
    expect(temporaryOutcome(row({ phase: "nameRequested" }), read(5))).toBeNull();
    expect(temporaryOutcome(row({ phase: "failed" }), read(5))).toBeNull();
    expect(temporaryOutcome(row({ identity: null }), read(5))).toBeNull();
  });

  it("contestRow: the identity's last registration of the label that asked for a temporary name", () => {
    const rows = [row({ draft: "a" }), row({ draft: "b", temporaryLabel: null }), row({ draft: "c", identity: "B" }), row({ draft: "d", phase: "failed" })];
    expect(contestRow(rows, "A", "ALICE")?.draft).toBe("a");
    // A failed request of the same label never hides the offer: only a row that reached its contest counts.
    expect(contestRow([row({ phase: "failed" }), row({ phase: "nameRequested" })], "A", "alice")).toBeUndefined();
    expect(contestRow(rows, "A", "bob")).toBeUndefined();
  });
});

describe("instantOffer: the card's and the request's offer while the contest is open", () => {
  const on: InstantInputs = { offered: true, nameCredits: "2000000000", asked: null };

  it("offers Create instant username to an identity with no other name", () => {
    expect(instantOffer(summary(), null, on)).toEqual({ kind: "create" });
  });

  it("gated by the Join page's gate, never by the network", () => {
    expect(instantOffer(summary(), null, { ...on, offered: false })).toBeNull();
    expect(instantOffer(summary(), null, undefined)).toBeNull();
    expect(registrationOffered({ watchOnly: true })).toBe(false);
    expect(registrationOffered(undefined)).toBe(false);
    // A mainnet wallet with its keys: registration from the Dash wallet is live there.
    const mainnetWallet = { watchOnly: false, network: "mainnet" };
    expect(registrationOffered(mainnetWallet)).toBe(true);
  });

  it("nothing while the names are unsettled or a temporary name is held", () => {
    expect(instantOffer(undefined, null, on)).toBeNull();
    expect(instantOffer(summary({ namesUpdating: true }), null, on)).toBeNull();
    expect(instantOffer(summary({ namesCheck: "failed" }), null, on)).toBeNull();
    expect(instantOffer(summary(), "alice2", on)).toBeNull();
  });

  it("credits short of a name: Top up", () => {
    expect(instantOffer(summary({ balance: "1999999999" }), null, on)).toEqual({ kind: "short" });
    expect(instantOffer(summary({ balance: "2000000000" }), null, on)).toEqual({ kind: "create" });
    // Unknown either way: offer it; the credits path says what it costs.
    expect(instantOffer(summary({ balance: null }), null, on)).toEqual({ kind: "create" });
    expect(instantOffer(summary({ balance: "1" }), null, { ...on, nameCredits: null })).toEqual({ kind: "create" });
  });

  it("a registration's temporary name: Try again once it failed, nothing while it is not known", () => {
    expect(instantOffer(summary(), null, { ...on, asked: { label: "alice2", outcome: "failed" } })).toEqual({ kind: "failed", label: "alice2" });
    expect(instantOffer(summary(), null, { ...on, asked: { label: "alice2", outcome: null } })).toBeNull();
    expect(instantOffer(summary(), null, { ...on, offered: false, asked: { label: "alice2", outcome: "failed" } })).toBeNull();
  });

  it("the copy", () => {
    expect(VOTE_COPY.instantHint("alice")).toBe("While “alice” is out for a vote, you can create an instant username to use now.");
    expect(INSTANT_COPY.failure("alice2")).toBe("The instant username “alice2” could not be registered, so you have no username to use while the vote runs.");
    expect(INSTANT_COPY.success("alice2", "alice")).toBe(
      "In the meantime you are reachable at “alice2”, which is yours to keep. If the vote awards you “alice”, you will be reachable at both usernames.",
    );
    expect(VOTE_COPY.wonMain("alice", "alice2")).toBe("“alice” is now your main username. Your instant username “alice2” stays yours too.");
    expect(INSTANT_COPY.confirm("alice2")).toBe("You chose @alice2 as an instant username. It can't be changed once it's registered.");
  });
});
