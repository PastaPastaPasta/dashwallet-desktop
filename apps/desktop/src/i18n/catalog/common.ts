import type { Tree } from "../core";

export const common = {
  cancel: "Cancel",
  save: "Save",
  /** "a, b and c": the last two items of a list. */
  listLast: "{rest} and {last}",
  listSeparator: ", ",
  /** An amount field holds text that is no amount; `example` is a number in the unit, as typed. */
  invalidAmount: "Enter an amount in {unit}, for example {example}.",
  /** In place of a funds action while the wallet data restored from before an upgrade is checked (RESTORED). */
  fundsPaused: "Anything that moves your funds is paused until syncing finishes.",
} as const satisfies Tree;
