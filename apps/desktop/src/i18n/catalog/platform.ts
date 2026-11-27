import type { Tree } from "../core";

/** The Platform addresses' engine refusals that no one screen owns. */
export const platform = {
  /** The engine refused a send or move from the Platform addresses on mainnet (`platform.feature_off`, `platform_address_sends`). */
  mainnetSendOff: "This version sends from Platform addresses on test networks only.",
  /** Platform's structure pre-check refused a move to the Dash wallet, a send to a Platform address or a
   * top-up from Platform before anything was signed: nothing was sent and no balance changed. */
  preflight: {
    /** Only while the network proved a protocol version above every one this build plans at. */
    rulesUnsupported:
      "Dash Platform's rules have changed, and this version of Dash Wallet can't send this under them. Nothing was sent, and your balance is unchanged. Update Dash Wallet, then try again.",
    refused: "Dash Platform would refuse this transfer as Dash Wallet built it, so nothing was sent and your balance is unchanged. If this keeps happening, update Dash Wallet.",
    /** The same refusals of the exact retry of a send that still holds: the earlier payment may land. */
    refusedRetry:
      "Dash Platform would refuse this attempt as Dash Wallet built it, so this attempt sent nothing. Your earlier payment may still go through, so check Activity before you pay again. If this keeps happening, update Dash Wallet.",
    rulesUnsupportedRetry:
      "Dash Platform's rules have changed, and this version of Dash Wallet can't send this under them, so this attempt sent nothing. Your earlier payment may still go through. Update Dash Wallet, and check Activity before you pay again.",
    /** The same refusals of the move out of a top-up that still holds: the earlier top-up may land. */
    refusedHeldTopUp:
      "Dash Platform would refuse this move as Dash Wallet built it, so this attempt sent nothing. Your earlier top-up may still go through, so check Activity before you try again. If this keeps happening, update Dash Wallet.",
    rulesUnsupportedHeldTopUp:
      "Dash Platform's rules have changed, and this version of Dash Wallet can't make this move under them, so this attempt sent nothing. Your earlier top-up may still go through. Update Dash Wallet, and check Activity before you try again.",
    /** The same refusals of the move out of a held cancel, move or contact payment: the earlier one may land. */
    refusedHeld:
      "Dash Platform would refuse this move as Dash Wallet built it, so this attempt sent nothing. Your earlier payment, move or cancel may still go through, so check Activity before you try again. If this keeps happening, update Dash Wallet.",
    rulesUnsupportedHeld:
      "Dash Platform's rules have changed, and this version of Dash Wallet can't make this move under them, so this attempt sent nothing. Your earlier payment, move or cancel may still go through. Update Dash Wallet, and check Activity before you try again.",
  },
  /** The cancel of a move to the Dash wallet or a contact payment from Platform that may still go through:
   * the same Platform funds go to the wallet's own Platform address instead, and only one of the two goes through. */
  cancel: {
    move: {
      action: "Cancel move",
      title: "Cancel this move?",
      body: "Dash Wallet can't tell yet whether this move of {amount} to your Dash wallet will go through. Cancelling uses the same Platform funds to send to your own Platform address instead, so only one of the two can go through. If the cancel goes through, the amount stays in your Platform balance, minus a fee of about {fee}.",
      confirm: "Cancel move",
      done: "Move cancelled. The amount is in your Platform balance, minus the fee.",
      lost: "The move went through before the cancel, so the amount is in your Dash wallet. The cancel wasn't charged.",
      /** The move is too small to cancel (`credits.send_out_of_range`): Move to Dash wallet is offered instead. */
      tooSmall:
        "This move of {amount} is too small to cancel. You can move the same funds to your Dash wallet instead. Only one of the two can go through, and either way the amount, less fees, reaches your Dash wallet.",
    },
    payment: {
      action: "Cancel payment",
      title: "Cancel this payment?",
      body: "Dash Wallet can't tell yet whether this payment of {amount} to {recipient} will go through. Cancelling sends the same Platform balance to your own Platform address instead, so only one of the two can go through. If the cancel goes through, {recipient} isn't paid, and the amount stays in your Platform balance, minus a fee of about {fee}. If the payment goes through first, {recipient} is paid and the cancel fails.",
      confirm: "Cancel payment",
      done: "Payment cancelled. {recipient} wasn't paid, and the amount is in your Platform balance, minus the fee.",
      lost: "The payment went through before the cancel, so {recipient} was paid. The cancel wasn't charged.",
      tooSmall:
        "This payment of {amount} to {recipient} is too small to cancel. You can move the same funds to your Dash wallet instead. Only one of the two can go through: if the move does, {recipient} isn't paid, and the amount, less fees, goes to your Dash wallet.",
    },
    dismiss: "Keep waiting",
    pending: "Cancelling. Dash Wallet is waiting for Dash Platform to confirm which one went through.",
  },
} as const satisfies Tree;
