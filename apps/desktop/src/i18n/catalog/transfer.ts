import type { Tree } from "../core";

export const transfer = {
  title: "Transfer",
  shortcut: "Transfer",
  from: "From",
  to: "To",
  pick: { from: "Transfer from", to: "Transfer to" },
  swap: "Swap From and To",
  pools: { core: "Dash wallet", shielded: "Shielded", platform: "Platform" },
  unknownBalance: "—",
  /** An end's button: which end, its pool, its balance (or what it receives), and what the button does. */
  endLabel: "{end}: {pool}, {balance}. {pick}",
  balanceNotKnown: "balance not known",
  /** What the Dash wallet holds in CoinJoin, under the From end. */
  mixing: "{amount} mixing",
  amountIn: "Amount ({unit})",
  max: "Max",
  wholeBalance: "This moves the spendable balance on your Platform addresses.",
  platformToCoreIntro: "The spendable balance on your Platform addresses moves to your Dash wallet. Dash Platform pays it to an address in your Dash wallet.",
  fee: "Fee about {fee} · You receive about {receive}",
  quoting: "Working out the fee…",
  /** Under a transfer's progress once its call has run for a second (it may wait for the wallet's start-up). */
  preparing: "Preparing the transfer…",
  /** A network switch refused while a transfer waits for its wallet's start-up. */
  preparingBlocksClose: "A transfer in one of your wallets is still being prepared. Try again when it's done.",
  feeOnNextStep: "Fee shown on the next step",
  invalidAmount: "Enter an amount.",
  notEnoughSpendable: "Not enough spendable Dash: {amount} available.",
  why: { amount: "Enter an amount first.", max: "Working out the most you can move…" },
  /** Max from the Dash wallet (one lock of every coin in its main account, the fee out of it): what it
   * leaves there, before Confirm. */
  maxLeft: "{left} stays in your Dash wallet, because Max doesn't include mixed coins, payments from contacts or coins in other accounts.",
  maxArriving: "{arriving} still arriving stays in your Dash wallet. Move it once it confirms.",
  /** Why Max from the Dash wallet can't run. */
  maxLocked: "Max can't run: some coins in your Dash wallet are locked. Enter an amount instead.",
  /** `count` is how many coins the wallet's main account holds. */
  maxTooMany: "Max needs at most 50 coins; this wallet has {count}. Enter an amount instead.",
  maxMixing: "Max can't run while mixing. Stop mixing first, or enter an amount.",
  maxMoveRunning: "Another move of your coins is running. Try Max again when it's done.",
  maxChanged: "Your Dash wallet changed since this was worked out. Review the amount again.",
  /** Max from the Dash wallet below the smallest amount: `amount` is what Max can move, `min` the
   * smallest amount. `maxBelowMinimumAll` when nothing in the wallet's other accounts stays. */
  maxBelowMinimum: "Max can move only {amount}, which is less than the smallest amount, {min}. Max doesn't include mixed coins, payments from contacts or coins in other accounts.",
  maxBelowMinimumAll: "Max can move only {amount}, which is less than the smallest amount, {min}.",
  continue: "Continue",
  unsupported: "Moving from Platform to Shielded isn't available on desktop yet. Move the funds to your Dash wallet first, then to Shielded.",
  /** Shielded → Platform on a network where it is off (mainnet). */
  offOnNetwork: "Moving from Shielded to Platform isn't available on this network yet. Move the funds to your Dash wallet first, then to Platform.",
  /** The off routes on mainnet, where Dash wallet → Platform is off too: no detour through it. */
  offOnMainnet: {
    shieldedToPlatform: "This version can move from Shielded to Platform on test networks only.",
    platformToShielded: "Moving from Platform to Shielded isn't available in this version.",
  },
  /** An Activity row whose group settled with no evidence of which payment or move went through. */
  status: {
    settledUnknown: {
      title: "Settled with another payment or move",
      text: "This move and another payment or move from the same Platform addresses couldn't both go through, and only one did. Dash Wallet can't tell which. Check your Platform balance and Dash wallet before moving again.",
    },
  },
  /** What this version keeps off on mainnet, each in place of its controls; `mainnetOffChip` beside the pool it can't reach. */
  platform: {
    mainnetOff: "This version can move and send from Platform addresses on test networks only.",
    mainnetOffAdd: "This version can add Dash to Platform on test networks only.",
    mainnetOffChip: "Test networks only",
    /** In place of `mainnetOff` while the Platform addresses hold a balance. */
    mainnetHeld: "{amount} is on your Platform addresses. It's still yours. This version can move it to your Dash wallet on test networks only.",
    /** In place of both while a payment or move made earlier holds the addresses: its exit is offered. */
    mainnetHeldExit: "A payment or move you made earlier from your Platform balance hasn't finished, and Dash Wallet can't tell yet whether it will go through. You can move what's on those Platform addresses to your Dash wallet instead. Only one of the two can go through, never both. If the earlier one goes through first, its recipient is paid and this move fails. If this move goes through, the earlier one doesn't, so whatever it paid for stays unpaid.",
    /** `mainnetHeldExit` for a hold of `count` (2 or more) records. */
    mainnetHeldExitSeveral: "{count} payments or moves you made earlier from your Platform balance haven't finished, and Dash Wallet can't tell yet whether they will go through. You can move what's on those Platform addresses to your Dash wallet instead. If one of them goes through first, its recipient is paid and this move fails. If this move goes through, none of them do, so whatever they paid for stays unpaid.",
    /** After `mainnetHeldExit` while the addresses outside the hold hold credits. */
    mainnetHeldExitRest: "The rest of your Platform balance, {rest}, is still yours. This version can move it on test networks only.",
  },
  none: {
    absent: "Transfers move Dash between your Dash wallet, Shielded and Platform. This version has neither Shielded nor Platform addresses.",
    inactive: "Transfers move Dash between your Dash wallet, Shielded and Platform. Shielded is not active on this network yet, and this version has no Platform addresses.",
  },
  platformEmpty: "Nothing on your Platform addresses to move yet.",
  platformInactive: "Platform addresses are not active on this network yet.",
  noCoreBalance: "Nothing in your Dash wallet to move yet.",
  loading: "Reading your balances…",
  /** `what` is a route's name (`routeNames`). */
  running: "Running: {what}.",
  show: "Show",
  pauseMixing: "Pause mixing",
  another: "Another transfer",
  routeNames: {
    coreToPlatform: "Add to Platform",
    shieldedToCore: "Move from Shielded to Dash wallet",
    shieldedToPlatform: "Move from Shielded to Platform",
    platformToCore: "Move from Platform to Dash wallet",
  },

  /** Platform → Dash wallet (platform/move.ts). */
  move: {
    dust: "{amount} on your Platform addresses cannot be moved to your Dash wallet right now.",
    message: "Everything on your Platform addresses moves to your Dash wallet. Dash Platform pays it to the address below.",
    rows: { taken: "Taken from Platform, about", fee: "Network fee, about", receive: "You receive about", to: "To your Dash wallet" },
    quoting: "Working out the fee…",
    quoteFailed: "The fee could not be worked out.",
    back: "Back",
    go: "Confirm & move",
    running: { title: "Moving…", text: "Dash Platform confirms the move. You can close this: Activity shows it." },
    done: {
      title: "Moved",
      text: "About {amount} is on its way to your Dash wallet. It shows in Activity once it arrives.",
      textWithFee: "About {amount} is on its way to your Dash wallet. The network fee was {fee}. It shows in Activity once it arrives.",
      debitOnly: "{taken} left your Platform addresses. Activity shows what arrives in your Dash wallet.",
      /** `debitOnly` when nothing says which member of its group went through. */
      debitUnknown: "{taken} left your Platform addresses, for this move or another payment or move from the same addresses. Dash Wallet can't tell which. If it was this move, it arrives in your Dash wallet and shows in Activity.",
    },
    pending: {
      title: "Not confirmed yet",
      text: "Dash Platform has not said whether the move of about {amount} went through. Activity shows it when it does. Do not move again until it does.",
      debitOnly: "Dash Platform has not said whether the move of {taken} went through. Activity shows it when it does. Do not move again until it does.",
    },
    replaced: { title: "Replaced by a retry", text: "This move was replaced by a retry. Activity shows the retry." },
    failed: { title: "The move did not go through", text: "Another payment or move from your Platform balance went first, possibly from another device using the same recovery phrase. Check your Platform balance: anything still there can be moved again." },
    unseen: "Your move has an update.",
    waiting: {
      text: "A move to your Dash wallet is not confirmed yet. Do not move again until Activity shows that it went through.",
      held: "An earlier payment or move from Platform may still go through. Wait for it to finish, then move again.",
      sendHeld: "A payment from Platform may still go through. Moving waits until it settles.",
      heldSend: "An earlier payment from Platform may still go through. Wait for it to finish, or send that same payment again from Pay and press Retry that payment, then move again.",
      action: "View in Activity",
    },
    check: { loading: "Checking for an unfinished move…", failed: "Could not check for an unfinished move.", again: "Check again" },
    show: "Show",
    close: "Close",
    /** An `invalid_argument` refusal without a detail. */
    notAccepted: "That was not accepted.",
    error: {
      insufficient: "Not enough on your Platform addresses: {needed} needed with the fee, {available} available.",
      insufficientFee: "There is not enough on your Platform addresses to cover the fee.",
      outOfRange: "Dash Platform moves between {min} and {max} at a time.",
      outOfRangeAmount: "Dash Platform does not take a move of this amount.",
      broadcastUnknown: "It may have been sent. Check Activity before trying again.",
      contextUnavailable: "Waiting for the network to sync and verify your Platform addresses. Try again shortly.",
      trustMismatch: "Your Platform addresses could not be verified against the network. Nothing was moved.",
      unavailable: "Dash Platform did not answer. Try again.",
      signerUnavailable: "Unlock the wallet fully and try again.",
      grantExceeded: "The cost went above what you approved. Try again.",
      quoteChanged: "What you can move from your Platform addresses changed since you reviewed this move. Review it again.",
      featureOff: "Platform addresses are turned off.",
      /** `code` is the engine's error code. */
      stopped: "It stopped with {code}.",
    },
  },

  /** Dash wallet → Platform (platform/fund.ts). */
  fund: {
    card: {
      unfinished: "{amount} was locked to add to Platform and is not applied yet.",
      stuck: {
        tooSmall: "{amount} was locked to add to Platform, but it's worth less than the Platform fee, so it can't be added.",
        unsupported: "{amount} was locked to add to Platform, and this version can't finish it.",
      },
      finish: "Finish",
      checkAgain: "Check again",
      busy: "Another add to Platform is running.",
    },
    form: {
      intro: "Move Dash from your wallet to your Platform addresses. It is locked first, then Dash Platform adds it to your Platform balance.",
      invalidAmount: "Enter an amount.",
      why: { amount: "Enter an amount first." },
      next: "Continue",
    },
    confirm: {
      quoting: "Working out the fee…",
      quoteFailed: "The fee could not be worked out.",
      /** Max from the Dash wallet: `total` is everything the lock spends, `fee` its network fee. */
      maxAll: "Move {total} from your Dash wallet to your Platform address? That's everything Max can move. The network fee (about {fee}) comes out of it.",
      rows: {
        from: "From",
        fromValue: "Your Dash wallet",
        to: "To",
        toValue: "Your Platform address",
        amount: "Amount",
        fee: "Network fee",
        total: "Total from your wallet",
        platformFee: "Platform fee, taken from the amount",
        receive: "You receive about",
      },
      back: "Back",
      go: "Confirm & move",
    },
    running: {
      title: "Working…",
      steps: { lock: "Locking funds", wait: "Waiting for the lock to confirm", add: "Adding to Platform" },
      text: "You can close this: Activity shows it.",
    },
    done: {
      applied: { title: "Added to Platform", text: "About {amount} was added to your Platform balance." },
      waiting: { title: "Locked, not applied yet", text: "Your Dash is locked and waiting for the network's confirmation. Finish it from Unfinished transfers on Home: nothing is lost." },
      /** `text`: the Add to Platform address result, which has no Check again button: it names Home's Unfinished transfers
       * card and its button. `row`: Home's row, beside its `card.checkAgain` button. Keep "Check again" and the card's
       * title the same as theirs. */
      checking: {
        title: "Checking",
        text: "This amount may already be on your Platform addresses. To confirm it, choose Check again on the Unfinished transfers card on Home. If its amount wasn't added yet, Check again tries to add it. If it still shows Checking, try again later.",
        row: "This amount may already be on your Platform addresses. Choose Check again to confirm it. If its amount wasn't added yet, Check again tries to add it. If it still shows Checking, try again later.",
      },
      /** `why` is the engine's error code. */
      notApplied: { title: "Locked, not applied", text: "Your Dash is locked but Dash Platform did not add it yet ({why}). Finish it from Unfinished transfers on Home: nothing is lost." },
    },
    approvalEnded: {
      pending: "Your {amount} is locked, but adding it to Platform stopped before it was applied. Choose Finish: it adds the same locked Dash, not more.",
      checking: "Checking that add stopped before Platform answered. Choose Check again: it checks the same lock and adds no more Dash.",
      unknown: "Adding to Platform stopped before it finished. If Unfinished transfers on Home shows an add waiting, choose Finish there: it adds the same locked Dash, not more. Otherwise, try again.",
      finish: "Finish",
      checkAgain: "Check again",
    },
    grantInvalid: {
      add: "The approval was not accepted, so nothing was locked. Try again.",
      resume: "The approval was not accepted. Your locked Dash is still waiting: choose Finish again.",
    },
    unseen: "Your add to Platform has an update.",
    show: "Show",
    close: "Close",
    back: "Back",
    error: {
      belowMinimum: "Too small to add: the smallest amount is {amount}.",
      tooSmall: "Too small to add.",
      insufficient: "Not enough Dash in your wallet: {needed} needed with the fee, {available} available.",
      insufficientShort: "Not enough Dash in your wallet.",
      coinsLocked: "Your Dash is reserved right now. Pause mixing or wait for pending payments to finish, then try again.",
      tooManyInputs: "That takes too many separate coins at once. Consolidate your coins first, then try again.",
      pending: "An earlier add to Platform is not finished. Finish it from Unfinished transfers on Home first.",
      featureOff: "Platform addresses are not active on this network yet.",
      broadcastUnknown: "The lock may have been sent. Check Activity before trying again.",
      willBeSent: "The payment is still being sent. Try again in a minute.",
      signerUnavailable: "Unlock the wallet fully and try again.",
      contextUnavailable: "Waiting for the network to sync before Platform can be used. Try again shortly.",
    },
  },

  /** Dash wallet ⇄ Shielded and Send from Shielded (shielded/flow.ts, ShieldedSheet.tsx). */
  shielded: {
    /** While Shielded's account is not established (shielded/syncing.ts). */
    sync: {
      syncing: "Shielded is still syncing; try again when it finishes.",
      scanning: "Scanning…",
      /** `when` is how long ago, as "5 min ago". */
      lastChecked: "Last checked {when}",
    },
    card: {
      inactive: "Shielded is not active on this network yet.",
      finish: "Finish",
      checkAgain: "Check again",
      why: { empty: "Nothing in Shielded to send yet.", busy: "Another Shielded move is running." },
      unfinished: "{amount} was locked to add to Shielded and is not applied yet.",
    },
    titles: { add: "Add to Shielded", send: "Send from Shielded", receive: "Receive in Shielded" },
    form: {
      why: { amount: "Enter an amount first.", to: "Enter a Shielded address first.", memo: "The note is too long." },
      add: "Move Dash from your wallet into Shielded. It is locked first, then Dash Platform adds it to your private balance.",
      send: "Send privately to another Shielded address.",
      out: "Move Shielded funds to your Dash wallet's next receive address.",
      toPlatform: "Move Shielded funds to your Platform address.",
      to: "To (a Shielded address)",
      memo: "Memo (optional)",
      memoHint: "Up to 36 bytes, visible to the recipient only.",
      available: "Available: {amount}",
      invalidAmount: "Enter an amount.",
      next: "Continue",
    },
    confirm: {
      quoting: "Working out the fee…",
      quoteFailed: "The fee could not be worked out.",
      /** Max from the Dash wallet: `total` is everything the lock spends, `fee` its network fee. */
      maxAll: "Move {total} from your Dash wallet to Shielded? That's everything Max can move. The network fee (about {fee}) comes out of it.",
      amount: "Amount",
      fee: "Network fee",
      coreFee: "Core network fee",
      poolFee: "Platform pool fee, taken from the amount",
      poolFeeFixed: "The Platform pool fee is fixed at about {amount}, whatever you add.",
      total: "Total",
      totalFromWallet: "Total from your wallet",
      lock: "Amount to add",
      gain: "Added to Shielded",
      receive: "You receive about",
      to: "To",
      slow: "Making the private proof takes up to a minute. Keep Dash Wallet open.",
      slowFirst: "The first Shielded payment after starting takes longer: the prover is still getting ready.",
      back: "Back",
      go: "Confirm",
    },
    running: {
      title: "Working…",
      prove: "Proving the transaction. This takes up to a minute; keep Dash Wallet open. You can close this: Activity shows it.",
      lock: "Locking the Dash and waiting for the network. You can close this: Activity shows it.",
    },
    done: {
      add: { title: "Added to Shielded", text: "Your Shielded balance is now {balance}." },
      waiting: { title: "Locked, not applied yet", text: "Your Dash is locked and waiting for the network's confirmation. Finish it from Unfinished transfers on Home: nothing is lost." },
      /** `text`: the Shielded and CoinJoin move results, which have no Check again button: it names Home's Unfinished
       * transfers card and its button. `row`: Home's row, beside its `card.checkAgain` button. Keep "Check again" and the
       * card's title the same as theirs. */
      checking: {
        title: "Checking",
        text: "This amount may already be in your shielded balance. To confirm it, choose Check again on the Unfinished transfers card on Home. If its amount wasn't added yet, Check again tries to add it. If it still shows Checking, try again later.",
        row: "This amount may already be in your shielded balance. Choose Check again to confirm it. If its amount wasn't added yet, Check again tries to add it. If it still shows Checking, try again later.",
      },
      /** `why` is the engine's error code. */
      notApplied: { title: "Locked, not applied", text: "Your Dash is locked but Dash Platform did not add it yet ({why}). Finish it from Unfinished transfers on Home: nothing is lost." },
      send: { title: "Sent", text: "{amount} was sent privately." },
      unconfirmed: { title: "Not confirmed yet", text: "Your payment went out and Dash Platform has not confirmed it yet. Activity shows it as pending until it does. Do not send it again: the wallet checks it itself." },
      out: { title: "Moved", text: "About {amount} is on its way to your Dash wallet. It arrives when Dash Platform's withdrawal is mined." },
      toPlatform: { text: "Moved to your Platform address {addr}." },
    },
    unrecorded: {
      title: "An earlier payment may still go through",
      text: "A shielded payment from before the restart may still go through, and the wallet no longer has its record. If you go on, you may pay twice.",
      go: { send: "Send anyway", out: "Move anyway" },
      back: "Go back",
      refused: "A shielded payment from before the restart may still go through. Confirm to send anyway.",
    },
    fundingUnknown: "The payment may have been sent. Open Tools ▸ Repair to settle it; you can move Dash into Shielded again once it has.",
    close: "Close",
    show: "Show",
    unseen: "Your Shielded move has an update.",
    /** Sending from Shielded waits until the last move's result is seen. */
    unseenSend: "Your last Shielded move has an update. Open it before you send from Shielded again.",
    receive: {
      text: "Money sent to this address stays private. Anyone with a Shielded balance can pay it.",
      copy: "Copy address",
      loading: "Getting your Shielded address…",
      qr: "QR code of {address}",
      copyAddress: "Copy Shielded address",
    },
    error: {
      poolTooSmall: "Shielded withdrawals open once the network's shielded pool has enough activity. Try again later.",
      invalidAddress: "Enter a Shielded address of this network.",
      insufficient: "Not enough in Shielded: {needed} needed with the fee, {available} available.",
      insufficientShort: "Not enough in Shielded.",
      fundingInsufficient: "Not enough Dash in your wallet: {needed} needed with the fee, {available} available.",
      fundingInsufficientShort: "Not enough Dash in your wallet.",
      largerAmount: "Enter a larger amount.",
      belowMinimumAdd: "Too small to add: the smallest amount is {amount}.",
      belowMinimum: "Too small: the smallest amount is {amount}.",
      coinsLocked: "Your Dash is reserved right now (for example by CoinJoin). Try again shortly.",
      tooManyInputs: "That takes too many separate amounts at once. Consolidate your coins first, then try again.",
      pending: "An earlier add to Shielded is not finished. Finish it from Unfinished transfers on Home first.",
      contextUnavailable: "Waiting for the network to sync before Shielded can be used. Try again shortly.",
      trustMismatch: "Your Shielded account could not be verified against the network. Nothing was moved.",
      featureOff: "Shielded is turned off.",
      toPlatformUnknown: "This move may still go through. Until Dash Wallet knows, these Shielded funds stay held. Don't move them again until Activity shows this move as done or failed.",
    },
  },
} as const satisfies Tree;
