/** The node the AT-SPI tree must show for the element `arguments[0]`, read from the DOM: a
 * dialog's name; a button's, link's, tab's or field's own name; else the first visible heading in
 * it, else its first visible button or link, else its first visible paragraph. Null when it has
 * none. */
export const MARKER_SCRIPT = `const el = document.querySelector(arguments[0]);
if (!el) return null;
const squash = (s) => (s ?? "").replace(/\\s+/g, " ").trim();
const byIds = (ids) => squash(ids.split(/\\s+/).map((id) => document.getElementById(id)?.textContent ?? "").join(" "));
const nameOf = (e) => {
  const by = e.getAttribute("aria-labelledby");
  if (by) return byIds(by);
  if (e.getAttribute("aria-label")) return squash(e.getAttribute("aria-label"));
  if (e.labels?.length) return squash([...e.labels].map((l) => l.textContent).join(" "));
  return squash(e.textContent);
};
const shown = (e) => e.getClientRects().length > 0;
const control = (e) => {
  const role = e.getAttribute("role");
  if (role === "tab") return ["PAGE_TAB", nameOf(e)];
  if (e.matches("a[href]") || role === "link") return ["LINK", nameOf(e)];
  if (e.matches("button") || role === "button") return [e.hasAttribute("aria-pressed") ? "TOGGLE_BUTTON" : "PUSH_BUTTON", nameOf(e)];
  if (e.matches("select")) return ["COMBO_BOX", nameOf(e)];
  if (e.matches("input[type=checkbox]")) return ["CHECK_BOX", nameOf(e)];
  if (e.matches("input[type=password]")) return ["PASSWORD_TEXT", nameOf(e)];
  if (e.matches("input:not([type=radio]), textarea")) return ["ENTRY", nameOf(e)];
  return null;
};
let m = null;
if (el.matches('[role="dialog"]')) m = ["DIALOG", nameOf(el)];
else m = control(el);
if (!m) {
  const h = [...el.querySelectorAll("h1, h2, h3, h4, h5, h6")].find(shown);
  if (h) m = ["HEADING", squash(h.textContent)];
}
if (!m) {
  const c = [...el.querySelectorAll("button, a[href]")].find(shown);
  if (c) m = control(c);
}
if (!m) {
  const p = el.matches("p") ? (shown(el) ? el : undefined) : [...el.querySelectorAll("p")].find(shown);
  if (p) m = ["PARAGRAPH", squash(p.textContent)];
}
return m && m[1] ? m : null;`;
