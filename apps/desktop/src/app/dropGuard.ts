// DP2-08 D11: Tauri's own drop handler is off (`dragDropEnabled: false`), so the webview gets
// HTML5 drops. A drop area (Add Contact's Scan) handles its own and cancels the default; a text
// field takes a dragged text as typing it would (an address into Pay); anywhere else the document
// refuses the drop, so a dropped file or link never navigates the webview. A dropped payment link
// (`dash:` or `pay:`) opens Pay as File ▸ Open URI… would (QT-019), once the shell bridge set the
// handler. Other links (contacts, invitations) are not taken by a drop: routing those can save them
// in the link stash, which a stray drag must not do; Open URI… and a click still open them.

/** The input types a dragged text can be typed into. */
const TEXT_INPUTS = new Set(["text", "search", "email", "url", "tel", "password", "number"]);

/** An editable text field, getting a drag without files. */
function intoTextField(e: DragEvent): boolean {
  if ([...(e.dataTransfer?.types ?? [])].includes("Files")) return false;
  const t = e.target as { tagName?: string; type?: string; isContentEditable?: boolean; readOnly?: boolean; disabled?: boolean } | null;
  if (!t) return false;
  if (t.isContentEditable === true) return true;
  const field = t.tagName === "TEXTAREA" || (t.tagName === "INPUT" && TEXT_INPUTS.has(t.type ?? ""));
  return field && !t.readOnly && !t.disabled;
}

/** A payment link: one line, `dash:` or `pay:`, no spaces. */
const LINK = /^(dash|pay):\S+$/i;

let onLink: ((text: string) => void) | null = null;

/** Who opens a dropped link (the shell bridge, while the page lives); `null`: none, a drop of
 * one is refused as any other. */
export function setLinkDrop(handler: ((text: string) => void) | null) {
  onLink = handler;
}

/** A drag that may carry a link: text, no files. Its data is readable only on drop. */
function mayCarryLink(e: DragEvent): boolean {
  const types = [...(e.dataTransfer?.types ?? [])];
  return onLink !== null && !types.includes("Files") && (types.includes("text/uri-list") || types.includes("text/plain"));
}

/** The dropped link, if the drop is one: the first entry of a URI list (`#` lines are comments),
 * else the plain text. */
export function droppedLink(data: Pick<DataTransfer, "getData"> | null | undefined): string | null {
  if (!data) return null;
  // A list of comments only names no URI: the plain text may.
  const listed = data.getData("text/uri-list").split(/\r?\n/).find((l) => l.trim() && !l.startsWith("#"));
  const text = (listed ?? data.getData("text/plain")).trim();
  return LINK.test(text) ? text : null;
}

export function installDropGuard(doc: Document = document) {
  doc.addEventListener("dragover", (e) => {
    if (e.defaultPrevented || intoTextField(e)) return;
    e.preventDefault();
    // The effect the source allows: a source that allows only a link operation is refused a copy.
    if (e.dataTransfer) e.dataTransfer.dropEffect = mayCarryLink(e) ? (/copy|all|uninitialized/i.test(e.dataTransfer.effectAllowed ?? "") ? "copy" : /link/i.test(e.dataTransfer.effectAllowed ?? "") ? "link" : "copy") : "none";
  });
  doc.addEventListener("drop", (e) => {
    if (intoTextField(e)) return;
    // A drop area (Add Contact's Scan) took it already: not a link to open too.
    const taken = e.defaultPrevented;
    e.preventDefault();
    if (taken || !mayCarryLink(e)) return;
    const link = droppedLink(e.dataTransfer);
    if (link) onLink?.(link);
  });
}
