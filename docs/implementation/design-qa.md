# Design QA

- Source visual truth paths (historical local captures; `[LOCAL_TEMP]` replaces the original machine-specific directory, and these images are not distributed with the source):
  - `[LOCAL_TEMP]/codex-clipboard-4238bdd6-3d99-4db8-a29f-ece73dd82085.png` (dark title-bar reference)
  - `[LOCAL_TEMP]/codex-clipboard-f92e03b2-38b2-480e-9ad2-5e200beddcad.png` (native app and scrollbar defect)
  - `[LOCAL_TEMP]/codex-clipboard-5666d074-0f40-476e-b6e3-f22d4436664e.png` (light-theme reference)
  - `[LOCAL_TEMP]/codex-clipboard-8e5cd347-a35d-478d-ae7f-9b776ef46ffb.png` (preferences-menu reference)
  - `[LOCAL_TEMP]/codex-clipboard-a13f3343-1cd9-4bcb-82d1-e4f48a88dc01.png` (scaled-layout clipping reference)
- Rendered implementation: `http://localhost:1420/`, Codex in-app Browser tab 1.
- Implementation screenshot evidence: Codex in-app Browser capture at 1496 × 851 and overflow capture at 1380 × 360, captured 2026-09-09.
- Viewports and density:
  - Light reference: 1496 × 851 px.
  - Light implementation: 1496 × 851 CSS px at devicePixelRatio 1 (1496 × 851 px).
  - Scrollbar defect reference: 1380 × 942 px.
  - Forced-overflow implementation: 1380 × 360 CSS px at devicePixelRatio 1; the shorter height intentionally forces the same vertical-overflow state.
- State: macOS-style overlay title bar; light system appearance; conversation and quota views; quota content forced to overflow for scrollbar verification.

## Full-view comparison evidence

The light reference and the implementation were compared at the same 1496 × 851 viewport. Both use a pale-gray sidebar, near-white workspace, subtle one-pixel separators, low-elevation white controls/cards, dark neutral copy, and restrained gray secondary text. dogcodes intentionally retains its own navigation, content, green accent, and typography rather than copying ZCode product content.

The user-provided native screenshot verifies that the overlay title bar removes the former white system-title strip and places the macOS traffic lights inside the app-owned top region. The browser-rendered comparison verifies the app-owned title region and theme surfaces; the native traffic lights themselves are outside browser-rendered content.

## Focused region comparison evidence

- Title region: 54px app-owned row, gray over the sidebar and workspace-colored over the main pane. Native title text is hidden; traffic-light inset is 18px.
- Scroll region: `.quota-center-content` reports `overflow-y: auto`. At 1380 × 360 it measured 306px client height and 356px scroll height and scrolled successfully.
- Scrollbar styling: computed width 10px, thumb `rgb(197, 197, 193)` in light mode, and fully transparent track. Dark tokens use `#4e4e4a` with `#666661` hover, so no white gutter is exposed.
- Theme tokens: light computed body background `rgb(247, 247, 246)`, sidebar `rgb(240, 240, 239)`, and `color-scheme: light`.
- Scale layout: the workspace now reserves an explicit auto-sized composer row and lets the conversation list shrink/scroll, so the CS settings mark and composer remain visible at 110% zoom.
- Scale recovery: overall scaling uses an explicit top-left transform with viewport-sized dimensions, and `⌘/Ctrl+0` restores 100% when a persisted scale makes the settings trigger hard to reach.

## Required fidelity surfaces

- Fonts and typography: existing Inter/system stack, sizes, weights, wrapping, and truncation remain unchanged. This request targets window chrome, scrollbar, and color appearance, so retaining dogcodes typography is intentional.
- Spacing and layout rhythm: the 54px header row aligns the sidebar and workspace. Existing navigation, cards, composer, radii, and spacing remain stable across themes.
- Colors and visual tokens: dark mode keeps the existing dogcodes palette. Light mode maps the reference's gray sidebar, near-white canvas, neutral borders, white raised surfaces, and subdued secondary text while preserving semantic green/red/amber states.
- Image quality and asset fidelity: supplied provider icons remain real source assets; no screenshot asset was replaced or approximated with CSS artwork.
- Copy and content: dogcodes labels and data remain unchanged; no ZCode-specific copy leaked into the product.
- Preferences and identity: the top-left product mark/name is removed; the compact CS mark now anchors the lower-left settings trigger. The menu exposes language, theme, zoom, usage, upgrade, and disconnect entries, with nested radio options for system/English/Simplified Chinese, system/light/dark, and 90/100/110%.

## Findings

No actionable P0, P1, or P2 mismatch remains for the requested title-bar, scrollbar, and light-theme scope.

## Comparison history

1. Earlier evidence: the native dark quota screen showed a full-height white scrollbar gutter against the dark workspace. Fix: added explicit thin scrollbar styling, transparent tracks, rounded inset thumbs, hover tokens, and root `color-scheme`. Post-fix evidence: forced-overflow browser capture and computed styles confirm a 10px scrollbar, transparent track, theme-appropriate thumb, and working scroll interaction.
2. Earlier evidence: dogcodes only exposed dark colors while the selected reference showed a light ZCode surface system. Fix: added system-driven light tokens and component mappings for navigation, workspace, messages, composer, quota cards, dialogs, diffs, and handoff UI; removed the forced native dark theme. Post-fix evidence: same-size 1496 × 851 browser capture shows the light hierarchy without layout regressions.

## Interaction and runtime checks

- Opened the conversation screen.
- Opened the quota-monitor screen.
- Forced vertical overflow and scrolled the quota content.
- Checked browser console errors: none.
- Opened the lower-left preferences menu and verified nested language, theme, and zoom menus; verified persisted root attributes and live light/dark/110% changes.
- Rechecked the conversation screen at 110%: the bottom-left CS mark, composer, and send controls remain inside the viewport.
- Ran typecheck, 66 automated tests, and production frontend build: all passed.

## Follow-up polish

No P3 item is required for this scope.

final result: passed
