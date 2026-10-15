// DP1-08 fixture states shared by dashpay.spec.ts and visual.spec.ts.

/** Each Home card state (docs/design/DP1-08-status-card.md S2–S17), its scenario, title and chip. */
export const CARDS: Record<string, { scenario: string; extra?: string; title: string; chip: string | null }> = {
  error: { scenario: "dashpay-no-identity", extra: "&fail=dashpay_status:none", title: "Could not read DashPay's state", chip: null },
  looking: { scenario: "dashpay-starting", title: "Looking for your DashPay identity…", chip: null },
  notStarted: { scenario: "dashpay-not-started", title: "DashPay has not started", chip: null },
  unlockToLook: { scenario: "dashpay-identity-unsettled", title: "Unlock to look for your DashPay identity", chip: null },
  lookFailed: { scenario: "dashpay-look-failed", title: "Could not finish looking for your DashPay identity", chip: null },
  join: { scenario: "dashpay-no-identity", title: "Join DashPay", chip: "Join DashPay" },
  joinWaiting: { scenario: "dashpay-waiting-for-sync", title: "Join DashPay", chip: "Join DashPay" },
  watchOnly: { scenario: "dashpay-watch-only", title: "DashPay needs this wallet's keys", chip: null },
  registering: { scenario: "dashpay-registering-unread", title: "Creating your username", chip: "Registering…" },
  creating: { scenario: "dashpay-registering", title: "Creating – @alice", chip: "@alice · Registering" },
  creatingUnlock: { scenario: "dashpay-registering-unlock", title: "Creating – @alice", chip: "@alice · Registering" },
  creatingPaused: { scenario: "dashpay-registering-paused", title: "Creating – @alice", chip: "@alice · Registering" },
  creatingRepair: { scenario: "tools-stuck-funding", title: "Creating – @alice", chip: "@alice · Registering" },
  voting: { scenario: "dashpay-contest-pending", title: "Voting – @alice", chip: "@alice · Pending" },
  votingNoEnd: { scenario: "dashpay-contest-pending-no-end", title: "Voting – @alice", chip: "@alice · Pending" },
  noUsername: { scenario: "dashpay-no-username", title: "Finish username registration", chip: "No username" },
};

/** Each identity chip (DASHPAY §4, DP1-08 S9–S18) and a scenario that shows it. */
export const CHIPS: Record<string, string> = {
  join: "dashpay-no-identity",
  registering: "dashpay-registering-unread",
  registeringLabel: "dashpay-registering",
  contest: "dashpay-contest-pending",
  identity: "dashpay-registered",
  noUsername: "dashpay-no-username",
  updating: "dashpay-names-updating",
  namesUnknown: "dashpay-names-unknown",
};

/** The text a chip settles on once the identities are read; until then it says "Updating…". */
export const CHIP_TEXT: Record<string, string> = {
  registeringLabel: "@alice",
  identity: "@alice",
  noUsername: "No username",
  namesUnknown: "Username not loaded",
};
