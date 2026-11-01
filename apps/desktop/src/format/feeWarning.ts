// "Most of this goes to fees" (DEC-1058 #8): the confirm sheets of the Platform and Shielded moves
// say so when the fees are a tenth of the amount or more. Credits, as the quotes have them.

/** The share of the amount, in percent, from which the fees are worth a warning. */
export const FEE_WARNING_PERCENT = 10n;

export interface FeeWarning {
  /** Whole percent of the amount the fees come to (floored). */
  percent: number;
  /** The fees are more than the half: most of the amount goes to them. */
  most: boolean;
}

/** The warning for `fees` out of `amount`, or `null` below `FEE_WARNING_PERCENT`. */
export function feeWarning(fees: bigint, amount: bigint): FeeWarning | null {
  if (amount <= 0n || fees <= 0n || fees * 100n < amount * FEE_WARNING_PERCENT) return null;
  return { percent: Number((fees * 100n) / amount), most: fees * 2n > amount };
}

export const feeWarningText = (w: FeeWarning): string => (w.most ? "Most of this goes to fees." : `Fees are ${w.percent}% of this amount.`);
