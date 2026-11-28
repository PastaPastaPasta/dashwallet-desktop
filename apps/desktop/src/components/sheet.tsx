// SheetScaffold (UX-SPEC C23): a modal dialog with the 64 pt header (title, close), Escape to
// close, focus kept inside while open and returned to the opener on close. Focus starts on the
// element marked `data-autofocus` (when enabled), else on the first focusable one (the close button), and again
// whenever `focusKey` changes (a sheet with steps). `closable={false}` holds the sheet open while
// something it must report back is running.
//
// A sheet opened from inside the shell (Pay, Receive) renders into App's sheet layer, above the
// shell and below the lock screen, and the shell goes inert while it is open (U-03 D22).

import { createContext, useContext, useEffect, useRef, useSyncExternalStore, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { useT } from "../i18n";
import { DashButton } from "./basics";

/** App's layer for sheets opened inside the shell; `null` renders a sheet in place. */
export const SheetLayer = createContext<HTMLElement | null>(null);

let layered = 0;
const watchers = new Set<() => void>();
function hold(by: number) {
  layered += by;
  for (const w of watchers) w();
}

/** Whether a sheet is open in the layer: the shell behind it takes no focus. */
export function useLayeredSheet(): boolean {
  return useSyncExternalStore(
    (w) => {
      watchers.add(w);
      return () => void watchers.delete(w);
    },
    () => layered > 0,
  );
}

/** Every sheet, in place or in the layer, numbered as it opens: the newest number given and the
 * ones open now. A toast alert hides under a sheet opened after it (toast.tsx). */
let sheetSeq = 0;
const openSheets = new Set<number>();
const sheetWatchers = new Set<() => void>();
const sheetsChanged = () => sheetWatchers.forEach((w) => w());

/** The number the newest sheet opened so far was given (0: none yet). */
export const lastSheetOpened = () => sheetSeq;

/** The number of the newest sheet open now (0: none open). */
export const newestOpenSheet = () => (openSheets.size ? Math.max(...openSheets) : 0);

export function watchSheets(w: () => void) {
  sheetWatchers.add(w);
  return () => void sheetWatchers.delete(w);
}

/** A scrim press this soon after a sheet opens is the rest of the click that opened it (a quick
 * second click on the button the sheet replaced), not a dismissal. */
const CARRIED_PRESS_MS = 600;

const FOCUSABLE = 'button:not([disabled]), [href], input:not([disabled]), select, textarea, summary, [tabindex]:not([tabindex="-1"])';

function tabbable(el: HTMLElement): boolean {
  if (el.matches(":disabled") || el.closest("[inert]") || getComputedStyle(el).visibility !== "visible") return false;
  const ownDetails = el.tagName === "SUMMARY" && el.parentElement?.tagName === "DETAILS" ? el.parentElement.parentElement : el;
  if (ownDetails?.closest("details:not([open])")) return false;
  return el.checkVisibility ? el.checkVisibility({ checkVisibilityCSS: true }) : el.getClientRects().length > 0;
}

function focusStart(panel: HTMLElement | null) {
  // A disabled field takes no focus: the first control that can does, else the dialog itself, so a
  // screen reader says its title (a payment being sent has nothing to press).
  (panel?.querySelector<HTMLElement>("[data-autofocus]:not(:disabled)") ?? panel?.querySelector<HTMLElement>(FOCUSABLE) ?? panel)?.focus();
}

export function Sheet({
  title,
  onClose,
  width,
  testId,
  focusKey,
  closable = true,
  returnFocus,
  children,
}: {
  title: string;
  onClose: () => void;
  width: "small" | "medium" | "large";
  testId: string;
  focusKey?: string;
  closable?: boolean;
  /** Where focus goes on close, instead of the element focused at open (gone, or `<body>`
   * when one sheet replaced another). */
  returnFocus?: () => HTMLElement | null;
  children: ReactNode;
}) {
  const t = useT();
  const panel = useRef<HTMLDivElement>(null);
  const openedAt = useRef(performance.now());
  const layer = useContext(SheetLayer);
  useEffect(() => {
    if (!layer) return;
    hold(1);
    return () => hold(-1);
  }, [layer]);
  useEffect(() => {
    const id = ++sheetSeq;
    openSheets.add(id);
    sheetsChanged();
    return () => {
      openSheets.delete(id);
      sheetsChanged();
    };
  }, []);
  useEffect(() => {
    // The opener first: the effect below moves focus away from it.
    const opener = document.activeElement as HTMLElement | null;
    return () => {
      const back = () => {
        const to = returnFocus?.() ?? opener;
        if (to?.isConnected) to.focus();
      };
      if (!layer) return back();
      // In the layer the shell is still inert now, so focusing into it would do nothing: once it is
      // not, return focus if it was lost (not if another sheet took it).
      requestAnimationFrame(() => {
        if (!document.activeElement || document.activeElement === document.body) back();
      });
    };
    // The opener and the return target are read when the sheet opens and closes.
  }, []);
  useEffect(() => focusStart(panel.current), [focusKey]);

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") {
      e.stopPropagation();
      if (closable) onClose();
      return;
    }
    // A sheet opened from inside this one handles its own Tab: React bubbles it here through the portal.
    if (e.key !== "Tab" || e.defaultPrevented || !panel.current || (e.target as Element).closest('[aria-modal="true"]') !== panel.current) return;
    const items = [...panel.current.querySelectorAll<HTMLElement>(FOCUSABLE)].filter(tabbable);
    const active = document.activeElement;
    const at = items.indexOf(active as HTMLElement);
    if (at >= 0 && at !== (e.shiftKey ? 0 : items.length - 1)) return;
    e.preventDefault();
    if (items.length === 0) return panel.current.focus();
    if (at >= 0) return void items.at(e.shiftKey ? -1 : 0)?.focus();
    const after = (el: HTMLElement) => active !== null && (active.compareDocumentPosition(el) & Node.DOCUMENT_POSITION_FOLLOWING) !== 0;
    (e.shiftKey ? items.findLast((el) => !after(el)) ?? items.at(-1) : items.find(after) ?? items[0])?.focus();
  };

  const sheet = (
    <div className="sheet-scrim" onMouseDown={(e) => closable && e.target === e.currentTarget && performance.now() - openedAt.current > CARRIED_PRESS_MS && onClose()}>
      <div
        ref={panel}
        className={`sheet sheet-${width}`}
        role="dialog"
        aria-modal="true"
        aria-labelledby={`${testId}-title`}
        tabIndex={-1}
        data-testid={testId}
        onKeyDown={onKeyDown}
      >
        <header className="sheet-header" data-testid={`${testId}-header`}>
          <span className="sheet-header-side" />
          <h2 id={`${testId}-title`} className="sheet-title">
            {title}
          </h2>
          <span className="sheet-header-side">
            <button type="button" className="sheet-close" aria-label={t("components.sheet.close")} disabled={!closable} onClick={onClose}>
              <svg viewBox="0 0 20 20" aria-hidden="true">
                <path d="M5 5l10 10M15 5 5 15" />
              </svg>
            </button>
          </span>
        </header>
        <div className="sheet-body">{children}</div>
      </div>
    </div>
  );
  return layer ? createPortal(sheet, layer) : sheet;
}

/** A question with Cancel and one confirming button; `onConfirm` runs after the sheet has closed. */
export function QuestionSheet({
  title,
  text,
  confirm,
  testId,
  confirmTestId,
  onConfirm,
  onClose,
}: {
  title: string;
  text: string;
  confirm: string;
  testId: string;
  confirmTestId: string;
  onConfirm: () => void;
  onClose: () => void;
}) {
  const t = useT();
  return (
    <Sheet title={title} width="small" testId={testId} onClose={onClose}>
      <p className="sheet-text">{text}</p>
      <div className="sheet-actions">
        <DashButton variant="tintedGray" data-autofocus onClick={onClose}>
          {t("common.cancel")}
        </DashButton>
        <DashButton
          variant="filledBlue"
          data-testid={confirmTestId}
          onClick={() => {
            onClose();
            onConfirm();
          }}
        >
          {confirm}
        </DashButton>
      </div>
    </Sheet>
  );
}
