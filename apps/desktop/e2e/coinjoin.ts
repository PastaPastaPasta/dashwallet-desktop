// U-06 fixture states shared by coinjoin.spec.ts and visual.spec.ts.

/** Each CoinJoin state (docs/design/U-06-coinjoin.md S2–S14), its scenario, title, and whether the
 * Home card shows it (D1). */
export const STATES: Record<string, { scenario: string; extra?: string; title: string; home: boolean }> = {
  readFailed: { scenario: "funded", extra: "&fail=coinjoin_status:none", title: "Could not read the mixing status", home: false },
  stopping: { scenario: "coinjoin-stopping", title: "Stopping…", home: true },
  starting: { scenario: "coinjoin-starting", title: "Starting…", home: true },
  paused: { scenario: "coinjoin-paused", title: "Paused", home: true },
  finished: { scenario: "coinjoin-finished", title: "Fully Mixed", home: true },
  mixing: { scenario: "coinjoin-mixing", title: "Mixing", home: true },
  disabled: { scenario: "coinjoin-disabled", title: "CoinJoin is turned off", home: false },
  watchOnly: { scenario: "coinjoin-watch-only", title: "Watch-only wallets cannot mix.", home: false },
  locked: { scenario: "coinjoin-locked", title: "Mixing stopped", home: true },
  error: { scenario: "coinjoin-error", title: "Mixing stopped", home: true },
  tooLittle: { scenario: "empty", title: "Not Started", home: false },
  done: { scenario: "coinjoin-custom", title: "Fully Mixed", home: false },
  off: { scenario: "funded", title: "Not Started", home: false },
};
