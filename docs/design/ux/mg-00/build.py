#!/usr/bin/env python3
"""Builds the MG-00 mock boards (UX-SPEC §4.25) into self-contained HTML files.

Each `src/<board>.html` holds frames:

    <!-- frame id="mn-list" title="My masternodes" route="masternodes" -->
    <main class="page">…</main>
    <!-- sheet -->            (optional: what the sheet layer shows above the shell)
    <div class="sheet sheet-medium">…</div>
    <!-- /frame -->

A frame's attributes: `id` (the PNG's name), `title`, `route` (the sidebar item that is current),
and optional `behind` (another frame of the board whose page shows behind this frame's sheet),
`badge` (the Masternodes item's badge), `status` (the status bar's left text), `lock` ("Unlocked", "Locked", "Watch-only"),
`wallet` (the toolbar's wallet name) and `net` ("tDASH", "DASH").

The output `<board>.html` inlines the renderer's real styles (apps/desktop/src/styles: the
generated tokens and app.css), Inter, the icons the frames use and `mg-00.css` (the components
these screens add), so it opens anywhere and looks like the app. `?theme=dark` (or the button at
the top) switches the theme; `#<frame id>` shows one frame. render.mjs screenshots every frame in
both themes into docs/design/screenshots/mg-00/.

Usage: python3 -I docs/design/ux/mg-00/build.py
"""

import base64
import html
import pathlib
import re

HERE = pathlib.Path(__file__).resolve().parent
REPO = HERE.parents[3]
STYLES = REPO / "apps/desktop/src/styles"
ICONS = REPO / "Resources/Icons"
FONTS = REPO / "apps/desktop/node_modules/@fontsource/inter/files"

BOARDS = ["masternodes", "governance", "registration", "maintenance", "shared"]

# The icons a fragment may use as `src="icon:<name>"` (Resources/Icons/<name>@2x.png).
ICON_NAMES = [
    "glyph-dash-currency",
    "action-copy",
    "masternode-keys",
    "masternode-keys-dark",
    "masternode-network-monitor",
    "masternode-network-monitor-dark",
    "governance-voting",
    "governance-voting-list",
    "governance-vote-approved",
    "message-warning",
    "form-search",
    "form-search-dark",
    "tx-received",
    "tx-received-dark",
    "tx-mining",
    "tx-mining-dark",
]

# Sidebar glyphs, copied from apps/desktop/src/components/shell.tsx; MASTERNODES is new (§4.25.1).
SIDEBAR = [
    ("home", "Home", '<path d="M3 9.5 10 3.5l7 6V17a1 1 0 0 1-1 1h-4v-5H8v5H4a1 1 0 0 1-1-1V9.5Z" />'),
    ("pay", "Pay", '<path d="M5 15 15 5M8 5h7v7" />'),
    ("receive", "Receive", '<path d="M15 5 5 15m7 0H5V8" />'),
    ("contacts", "Contacts", '<circle cx="10" cy="7" r="3" /><path d="M4 17c.6-3 3-4.5 6-4.5s5.4 1.5 6 4.5" />'),
    ("activity", "Activity", '<path d="M3 5h2m3 0h9M3 10h2m3 0h9M3 15h2m3 0h9" />'),
    ("coinJoin", "CoinJoin", '<circle cx="7.5" cy="10" r="4.5" /><circle cx="12.5" cy="10" r="4.5" />'),
    (
        "masternodes",
        "Masternodes",
        '<rect x="3.5" y="3.5" width="13" height="5.5" rx="1.5" /><rect x="3.5" y="11" width="13" height="5.5" rx="1.5" />'
        '<path d="M6.5 6.25h.01M6.5 13.75h.01M10 6.25h3.5M10 13.75h3.5" />',
    ),
]

BELL = '<path d="M5 14V9a5 5 0 0 1 10 0v5l1.5 1.5h-13L5 14Z" /><path d="M8.5 17.5h3" />'
EYE = '<path d="M2 10s3-5.5 8-5.5S18 10 18 10s-3 5.5-8 5.5S2 10 2 10Z" /><circle cx="10" cy="10" r="2.5" />'
LOCK = '<rect x="4.5" y="9" width="11" height="8" rx="1.5" /><path d="M7 9V6.5a3 3 0 0 1 6 0V9" />'
CLOSE = '<path d="M5 5l10 10M15 5 5 15" />'


def minify_css(css: str) -> str:
    css = re.sub(r"/\*.*?\*/", "", css, flags=re.S)
    css = re.sub(r"\s+", " ", css)
    css = re.sub(r"\s*([{};:,>])\s*", r"\1", css)
    return css.strip()


def data_uri(path: pathlib.Path, mime: str) -> str:
    return f"data:{mime};base64,{base64.b64encode(path.read_bytes()).decode()}"


def fonts_css() -> str:
    out = []
    for weight in (400, 500, 600, 700):
        uri = data_uri(FONTS / f"inter-latin-{weight}-normal.woff2", "font/woff2")
        out.append(
            f"@font-face{{font-family:Inter;font-style:normal;font-display:block;font-weight:{weight};src:url({uri}) format('woff2')}}"
        )
    return "".join(out)


def svg(paths: str, cls: str) -> str:
    return f'<svg class="{cls}" viewBox="0 0 20 20" aria-hidden="true">{paths}</svg>'


def shell(frame: dict, page: str, sheet: str) -> str:
    route = frame.get("route", "masternodes")
    items = []
    for key, title, paths in SIDEBAR:
        current = key == route
        badge = ""
        if key == "contacts":
            badge = '<span class="sidebar-badge" aria-label="1 waiting">1</span>'
        if key == "masternodes" and frame.get("badge"):
            badge = f'<span class="sidebar-badge" aria-label="{frame["badge"]} need attention">{frame["badge"]}</span>'
        items.append(
            f'<li><button type="button" class="sidebar-item{" is-current" if current else ""}"'
            f'{" aria-current=\"page\"" if current else ""}>{svg(paths, "sidebar-icon")}'
            f'<span class="sidebar-title">{title}</span>{badge}</button></li>'
        )
    lock = frame.get("lock", "Unlocked")
    net = frame.get("net", "tDASH")
    status_right = "".join(f"<span>{x}</span>" for x in frame.get("right", f"{net}|HD|{lock}|8 peers|Synced").split("|"))
    layer = f'<div class="sheet-scrim">{sheet}</div>' if sheet.strip() else ""
    return f"""
<div class="app" data-mock-shell>
 <div class="app-chrome">
  <header class="toolbar">
   <span class="toolbar-start">
    <button type="button" class="identity-chip" aria-label="My profile, @alice"><span class="identity-chip-avatar" aria-hidden="true">A</span><span>@alice</span></button>
    <span class="toolbar-wallet">{html.escape(frame.get("wallet", "Main"))}</span>
   </span>
   <span class="toolbar-actions">
    <button type="button" class="toolbar-button" aria-label="Notifications">{svg(BELL, "toolbar-icon")}</button>
    <button type="button" class="toolbar-button" aria-label="Hide balance">{svg(EYE, "toolbar-icon")}</button>
    <button type="button" class="toolbar-button" aria-label="Lock wallet">{svg(LOCK, "toolbar-icon")}</button>
   </span>
  </header>
  <div class="app-body">
   <nav class="sidebar" aria-label="Main"><ul role="list" class="sidebar-list">{"".join(items)}</ul></nav>
   {page}
  </div>
  <footer class="status-bar"><span class="status-left"><span role="status">{html.escape(frame.get("status", "Up to date"))}</span></span><span class="status-right">{status_right}</span></footer>
 </div>
 {layer}
</div>"""


FRAME_RE = re.compile(r"<!--\s*frame\s+(.*?)-->(.*?)<!--\s*/frame\s*-->", re.S)
ATTR_RE = re.compile(r'(\w+)="([^"]*)"')


def build_board(name: str, base_css: str, icons: dict) -> str:
    src = (HERE / "src" / f"{name}.html").read_text()
    board_title = re.search(r"<!--\s*board\s+title=\"([^\"]+)\"\s*-->", src)
    found = [(dict(ATTR_RE.findall(attrs)), body) for attrs, body in FRAME_RE.findall(src)]
    pages = {frame["id"]: body.partition("<!-- sheet -->")[0] for frame, body in found}
    frames = []
    for frame, body in found:
        page, _, sheet = body.partition("<!-- sheet -->")
        if "behind" in frame:
            page = pages[frame["behind"]]
        # Inline glyphs: <dash/> is an amount's Ð glyph; <x-close/> the sheet's close button.
        for part in ("page", "sheet"):
            text = page if part == "page" else sheet
            text = text.replace(
                "<dash/>", '<span class="glyph" aria-hidden="true" style="--glyph-url:var(--mg-glyph);--glyph-factor:0.8"></span>'
            )
            text = text.replace(
                "<x-close/>", f'<button type="button" class="sheet-close" aria-label="Close">{svg(CLOSE, "")}</button>'
            )
            text = re.sub(r'src="icon:([a-z0-9-]+)"', lambda m: f'src="{icons[m.group(1)]}"', text)
            if part == "page":
                page = text
            else:
                sheet = text
        frames.append(
            f'<section class="mock-frame" id="{frame["id"]}" aria-label="{html.escape(frame["title"])}">'
            f'<h2 class="mock-caption">{html.escape(frame["id"])} — {html.escape(frame["title"])}</h2>'
            f'<div class="mock-window">{shell(frame, page, sheet)}</div></section>'
        )
    title = board_title.group(1) if board_title else name
    return f"""<!doctype html>
<!-- Generated by docs/design/ux/mg-00/build.py from src/{name}.html. Do not edit. -->
<html lang="en" data-theme="light">
<head>
<meta charset="utf-8">
<title>MG-00 mocks — {html.escape(title)}</title>
<style>{base_css}</style>
<script>
(() => {{
  const t = new URLSearchParams(location.search).get("theme");
  if (t === "dark" || t === "light") document.documentElement.dataset.theme = t;
}})();
</script>
</head>
<body class="mock-board">
<header class="mock-board-header">
 <h1>MG-00 · {html.escape(title)}</h1>
 <p>Mocks for UX-SPEC §4.25, drawn with the renderer's tokens and styles. Sample data.</p>
 <button type="button" onclick="const r=document.documentElement;r.dataset.theme=r.dataset.theme==='dark'?'light':'dark'">Light / dark</button>
</header>
{"".join(frames)}
</body>
</html>
"""


def main():
    icons = {n: data_uri(ICONS / f"{n}@2x.png", "image/png") for n in ICON_NAMES}
    tokens = (STYLES / "tokens.generated.css").read_text()
    # The boards switch themes with data-theme only; the prefers-color-scheme copy is not needed.
    tokens = re.sub(r"@media \(prefers-color-scheme: dark\) \{.*\Z", "", tokens, flags=re.S)
    base_css = "".join(
        [
            fonts_css(),
            minify_css(tokens),
            minify_css((STYLES / "app.css").read_text()),
            f":root{{--mg-glyph:url({icons['glyph-dash-currency']})}}",
            minify_css((HERE / "mg-00.css").read_text()),
        ]
    )
    for name in BOARDS:
        if not (HERE / "src" / f"{name}.html").exists():
            continue
        out = HERE / f"{name}.html"
        out.write_text(build_board(name, base_css, icons))
        print(f"{out.relative_to(REPO)}: {out.stat().st_size // 1024} KiB")


if __name__ == "__main__":
    main()
