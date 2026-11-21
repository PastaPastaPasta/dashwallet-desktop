import type { Tree } from "../core";

export const rates = {
  disclosure:
    "The wallet asks rates.ctx.com for DASH prices about every 5 minutes, and BitPay and DashCentral when it cannot reach it. No address, balance or transaction is sent; the requests show these services your IP address, the time, and that a Dash wallet is running.",
  testOnly: "Test coins have no price: prices are fetched only while Mainnet is open.",
  lastFailed: "Last update failed at {time}.",
  updated: "Updated {time} from {source}.",
  overHalfHour: "Prices are over 30 minutes old.",
  movedHalf: "Prices moved more than 50% since the last update.",
  unavailable: "Prices unavailable",
  staleTitle: "Price from {time}, may be out of date",
  sourceFallback: "BitPay and DashCentral",
  reason: {
    neverFetched: "Prices have not been fetched yet.",
    failed: "Prices could not be fetched. The wallet tries again in a few minutes.",
    expired: "The last prices are over 24 hours old, so none are shown.",
    noCurrency: "Choose a local currency in Settings ▸ General.",
    currencyMissing: "There is no price for the chosen currency. Choose another in Settings ▸ General.",
  },
} as const satisfies Tree;
