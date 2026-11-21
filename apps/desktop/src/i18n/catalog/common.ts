import type { Tree } from "../core";

export const common = {
  cancel: "Cancel",
  save: "Save",
  /** "a, b and c": the last two items of a list. */
  listLast: "{rest} and {last}",
  listSeparator: ", ",
} as const satisfies Tree;
