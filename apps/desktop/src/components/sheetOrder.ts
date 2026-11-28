// Every sheet, in place or in the layer, numbered as it opens. A toast alert hides under a sheet
// opened after it (toast.tsx). No imports: toast.tsx is reached from e2e helpers, which load no
// assets.

let seq = 0;
const open = new Set<number>();
const watchers = new Set<() => void>();
const changed = () => watchers.forEach((w) => w());

/** Numbers a sheet as it opens; the returned function closes it. */
export function sheetOpened(): () => void {
  const id = ++seq;
  open.add(id);
  changed();
  return () => {
    open.delete(id);
    changed();
  };
}

/** The number the newest sheet opened so far was given (0: none yet). */
export const lastSheetOpened = () => seq;

/** The number of the newest sheet open now (0: none open). */
export const newestOpenSheet = () => (open.size ? Math.max(...open) : 0);

export function watchSheets(w: () => void) {
  watchers.add(w);
  return () => void watchers.delete(w);
}
