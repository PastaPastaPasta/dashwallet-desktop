// DEC-1058 #8: a release build has no WebView default context menu (Reload, Inspect Element, the
// system's page items). A text field keeps its own, for cut, copy and paste. A development build
// keeps all of it: Inspect Element is for developers.

/** The input types whose text the menu cuts, copies and pastes. */
const TEXT_INPUTS = new Set(["text", "search", "email", "url", "tel", "password", "number"]);

/** Whether the menu belongs to a text field or an editable area. */
export function inTextField(target: EventTarget | null): boolean {
  const t = target as { tagName?: string; type?: string; isContentEditable?: boolean} | null;
  if (!t) return false;
  if (t.isContentEditable === true) return true;
  return t.tagName === "TEXTAREA" || (t.tagName === "INPUT" && TEXT_INPUTS.has(t.type ?? ""));
}

/** Cancels the context menu outside text fields when `release`; returns the uninstall. */
export function installContextMenuGuard(release: boolean, doc: Document = document): () => void {
  if (!release) return () => {};
  const guard = (e: Event) => {
    if (!inTextField(e.target)) e.preventDefault();
  };
  doc.addEventListener("contextmenu", guard);
  return () => doc.removeEventListener("contextmenu", guard);
}
