---
name: "Tugboat"
description: "Conventional macOS utility with a native Brig and Codex terminal."
colors:
  background: "#f1f1f1"
  workspace-dot: "#a8c7ff"
  control-background: "#ffffff"
  settings-group: "#ffffff"
  settings-divider: "#f9f9f9"
  field-border: "#b8b8b8"
  surface: "#f1f1f1"
  sidebar: "#1e1e1e"
  sidebar-line: "#333333"
  sidebar-text: "#eeeeee"
  sidebar-hover: "#202020"
  sidebar-selection: "#4d8dff"
  sidebar-selected-text: "#f1f1f1"
  sidebar-brand: "#ffffff"
  line: "#dedede"
  text: "#121212"
  secondary: "#626262"
  selection: "#dce8ff"
  accent: "#4d8dff"
  accent-hover: "#4483ef"
  on-accent: "#f1f1f1"
  focus: "#4d8dff"
  error: "#a52b22"
  contrast-line: "#777777"
  terminal-foreground: "#ffe735"
  terminal-background: "#121212"
  terminal-cursor: "#aeb9c4"
  terminal-selection-background: "#aeb9c4"
  terminal-selection-foreground: "#000000"
  terminal-divider: "#333333"
  terminal-ansi-0: "#1d2021"
  terminal-ansi-1: "#d4380d"
  terminal-ansi-2: "#52c41a"
  terminal-ansi-3: "#ffc069"
  terminal-ansi-4: "#4f86cd"
  terminal-ansi-5: "#d3adf7"
  terminal-ansi-6: "#597ef7"
  terminal-ansi-7: "#bfcad5"
  terminal-ansi-8: "#7c6f64"
  terminal-ansi-9: "#ff9c6e"
  terminal-ansi-10: "#b8bb26"
  terminal-ansi-11: "#ffe735"
  terminal-ansi-12: "#83a598"
  terminal-ansi-13: "#d3869b"
  terminal-ansi-14: "#8ec07c"
  terminal-ansi-15: "#ffffff"
typography:
  body:
    fontFamily: "Inter, -apple-system, BlinkMacSystemFont, sans-serif"
    fontSize: "13px"
  title:
    fontSize: "24px"
    fontWeight: 650
    lineHeight: "30px"
    letterSpacing: "-.025em"
  project-title:
    fontSize: "22px"
    fontWeight: 650
    lineHeight: "30px"
    letterSpacing: "-.025em"
  project-row-name:
    fontSize: "14px"
    fontWeight: 600
    lineHeight: "20px"
  project-meta:
    fontSize: "12px"
    lineHeight: 1.5
  project-monogram:
    fontSize: "12px"
    fontWeight: 600
  table:
    fontSize: "12px"
  table-heading:
    fontSize: "12px"
    fontWeight: 500
  session-title:
    fontSize: "13px"
    fontWeight: 550
  session-date:
    fontSize: "11px"
  session-tab:
    fontSize: "13px"
    fontWeight: 500
  usage-value:
    fontSize: "19px"
    fontWeight: 550
    lineHeight: "24px"
  usage-percentage:
    fontSize: "12px"
    fontWeight: 400
  usage-session-title:
    fontSize: "14px"
    fontWeight: 550
    lineHeight: 1.5
  request-usage:
    fontSize: "12px"
    lineHeight: 1.6
  setup-action:
    fontSize: "13px"
    fontWeight: 550
  description:
    fontSize: "14px"
    lineHeight: 1.5
  brand:
    fontSize: "14px"
    fontWeight: 500
    lineHeight: 1.25
  section-label:
    fontSize: "16px"
    fontWeight: 650
    lineHeight: "20px"
  field:
    fontFamily: "Inter, -apple-system, BlinkMacSystemFont, sans-serif"
    fontSize: "14px"
    lineHeight: "20px"
  field-hint:
    fontSize: "12px"
    fontWeight: 400
    lineHeight: 1.5
  settings-term:
    fontSize: "13px"
    fontWeight: 500
    lineHeight: 1.5
  label:
    fontSize: "13px"
    fontWeight: 600
  hint:
    fontSize: "12px"
  document:
    fontFamily: "Inter, -apple-system, BlinkMacSystemFont, sans-serif"
    fontSize: "12px"
    lineHeight: 1.5
  dialog-title:
    fontSize: "20px"
  terminal:
    fontFamily: "Consolas, JetBrains Mono"
    fontSize: "14pt"
rounded:
  navigation: "6px"
  button: "7px"
  group: "9px"
  setup: "8px"
  field: "5px"
  dialog: "12px"
spacing:
  x4: "4px"
  x6: "6px"
  x8: "8px"
  x10: "10px"
  x12: "12px"
  x14: "14px"
  x16: "16px"
  x20: "20px"
  x22: "22px"
  x24: "24px"
  x28: "28px"
  x32: "32px"
  x36: "36px"
  x40: "40px"
  terminal-inset: "10pt"
components:
  button-primary:
    backgroundColor: "{colors.accent}"
    textColor: "{colors.on-accent}"
    typography: "{typography.label}"
    rounded: "{rounded.button}"
    padding: "9px 16px"
    height: "36px"
  button-primary-hover:
    backgroundColor: "{colors.accent-hover}"
  button-secondary:
    backgroundColor: "{colors.control-background}"
    textColor: "{colors.text}"
    typography: "{typography.body}"
    rounded: "{rounded.navigation}"
    padding: "7px 13px"
    height: "32px"
  button-secondary-hover:
    backgroundColor: "{colors.selection}"
  button-destructive:
    backgroundColor: "{colors.error}"
    textColor: "{colors.control-background}"
    typography: "{typography.label}"
    rounded: "{rounded.button}"
    padding: "9px 16px"
    height: "36px"
  icon-button:
    backgroundColor: "transparent"
    textColor: "{colors.secondary}"
    rounded: "{rounded.navigation}"
    padding: "8px"
    size: "36px"
  icon-button-hover:
    backgroundColor: "{colors.selection}"
    textColor: "{colors.text}"
  session-action:
    backgroundColor: "transparent"
    textColor: "{colors.secondary}"
    rounded: "{rounded.navigation}"
    padding: "6px"
    size: "28px"
  sidebar:
    backgroundColor: "{colors.sidebar}"
    textColor: "{colors.sidebar-text}"
    typography: "{typography.body}"
    padding: "23px 12px 14px"
    width: "280px"
  sidebar-resizer:
    width: "8px"
    cursor: "col-resize"
  navigation-item:
    backgroundColor: "transparent"
    textColor: "{colors.sidebar-text}"
    typography: "{typography.body}"
    rounded: "{rounded.navigation}"
    padding: "8px 12px"
    width: "100%"
  navigation-item-current:
    backgroundColor: "{colors.sidebar-selection}"
    textColor: "{colors.sidebar-selected-text}"
    typography: "{typography.label}"
  sidebar-project:
    backgroundColor: "transparent"
    textColor: "{colors.sidebar-text}"
    typography: "{typography.body}"
    rounded: "{rounded.navigation}"
    padding: "10px 8px"
    width: "100%"
  sidebar-project-current:
    backgroundColor: "{colors.sidebar-selection}"
    textColor: "{colors.sidebar-selected-text}"
  sidebar-session:
    backgroundColor: "transparent"
    textColor: "{colors.sidebar-text}"
    typography: "{typography.body}"
    rounded: "{rounded.navigation}"
    padding: "9px 10px"
    width: "100%"
  sidebar-session-current:
    backgroundColor: "{colors.sidebar-selection}"
    textColor: "{colors.sidebar-selected-text}"
  sidebar-text-button:
    backgroundColor: "transparent"
    textColor: "{colors.sidebar-text}"
    typography: "{typography.hint}"
    rounded: "{rounded.field}"
    padding: "5px 6px"
  project-monogram:
    backgroundColor: "{colors.sidebar-hover}"
    textColor: "{colors.sidebar-text}"
    typography: "{typography.project-monogram}"
    rounded: "{rounded.navigation}"
    size: "32px"
  project-monogram-library:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.text}"
    size: "32px"
  project-library:
    textColor: "{colors.text}"
    padding: "0"
    width: "100%"
  workspace-header:
    backgroundColor: "{colors.background}"
    textColor: "{colors.text}"
    typography: "{typography.project-title}"
    padding: "24px 32px"
    height: "100px"
    width: "100%"
  table-surface:
    backgroundColor: "{colors.control-background}"
    textColor: "{colors.text}"
    typography: "{typography.table}"
    rounded: "{rounded.group}"
    padding: "0"
    width: "100%"
  table-heading:
    backgroundColor: "{colors.settings-divider}"
    textColor: "{colors.secondary}"
    typography: "{typography.table-heading}"
    padding: "13px 16px"
  session-tab:
    backgroundColor: "transparent"
    textColor: "{colors.secondary}"
    typography: "{typography.session-tab}"
    rounded: "{rounded.navigation}"
    padding: "8px 12px"
  session-tab-current:
    backgroundColor: "{colors.control-background}"
    textColor: "{colors.text}"
  usage-summary:
    textColor: "{colors.text}"
    typography: "{typography.usage-value}"
    padding: "0 0 24px"
    width: "100%"
  setup-action:
    backgroundColor: "transparent"
    textColor: "{colors.text}"
    typography: "{typography.setup-action}"
    rounded: "{rounded.setup}"
    padding: "20px"
  project-settings-dialog:
    backgroundColor: "{colors.control-background}"
    textColor: "{colors.text}"
    rounded: "{rounded.dialog}"
    padding: "0"
    width: "min(900px, calc(100vw - 64px))"
  session-usage-dialog:
    backgroundColor: "{colors.control-background}"
    textColor: "{colors.text}"
    rounded: "{rounded.dialog}"
    padding: "0"
    width: "min(900px, calc(100vw - 64px))"
  settings-group:
    backgroundColor: "{colors.settings-group}"
    textColor: "{colors.text}"
    typography: "{typography.body}"
    rounded: "{rounded.group}"
    padding: "0 14px"
  text-field:
    backgroundColor: "{colors.control-background}"
    textColor: "{colors.text}"
    typography: "{typography.field}"
    rounded: "{rounded.field}"
    padding: "7px 10px"
    width: "100%"
  select-field:
    backgroundColor: "{colors.control-background}"
    textColor: "{colors.text}"
    typography: "{typography.field}"
    rounded: "{rounded.field}"
    padding: "7px 38px 7px 10px"
    width: "100%"
  disclosure-group:
    textColor: "{colors.text}"
    typography: "{typography.label}"
  recent-project:
    backgroundColor: "transparent"
    textColor: "{colors.text}"
    typography: "{typography.body}"
    padding: "10px 0"
    width: "100%"
  project-row-name:
    typography: "{typography.project-row-name}"
  project-branch:
    textColor: "{colors.secondary}"
    typography: "{typography.project-meta}"
  terminal-project-header:
    backgroundColor: "{colors.background}"
    textColor: "{colors.text}"
    typography: "{typography.project-title}"
    padding: "16px 24px"
    height: "86px"
    width: "100%"
  terminal-toolbar:
    backgroundColor: "{colors.terminal-background}"
    textColor: "{colors.sidebar-text}"
    typography: "{typography.body}"
    padding: "8px 16px"
  terminal-surface:
    backgroundColor: "{colors.terminal-background}"
    textColor: "{colors.terminal-foreground}"
    typography: "{typography.terminal}"
    padding: "{spacing.terminal-inset}"
  removal-dialog:
    backgroundColor: "{colors.control-background}"
    textColor: "{colors.text}"
    rounded: "{rounded.dialog}"
    padding: "{spacing.x24}"
    width: "min(560px, calc(100vw - 48px))"
---

# Design System: Tugboat

## Overview

**Creative North Star: "Conventional macOS"**

Tugboat is a compact utility for Apple silicon Macs with macOS 15 or later. Its subtitle is “Agent Sandbox”. Preserve the visible dark native titlebar, window buttons, application menus, and folder panel. English labels and locally bundled Inter support direct project work. Native macOS shell affordances retain system typography.

The system covers the implemented project library, project dashboard, settings, policy, boundary-check, session-management, usage, and terminal surfaces. The application uses one fixed composition: a dark native window header, a solid dark sidebar, and a pale workspace with white controls, independent of operating-system appearance. Only the Projects library uses the dot grid. Selected projects and Settings use plain canvases. The terminal has a separate, fixed dark theme and a native AppKit renderer. Density serves the current task: project and session tables, a fixed project identity, wide scoped settings, and maximum terminal space.

**Key Characteristics:**
- Real macOS shell affordances and keyboard commands.
- Inter UI type, a dark native window header, a solid dark sidebar, pale project and Settings canvases, white controls, and blue actions with light text.
- A dot grid only in the Projects library, plain selected-project and Settings canvases, and rounded white information groups.
- A white application name above its subtitle beside the existing icon, with Lucide icons beside both navigation labels.
- A project table in last-opened order, path tooltips, and a selected-project sidebar with active sessions.
- A fixed name-and-branch header with a single path-and-Brig-reference line and a scoped settings action.
- A centered setup action for unready projects; ready projects show usage totals, session states, and New Chat.
- Editable settings, prompt-level usage, plain status text, and explicit archive, Trash, restore, and removal controls.
- A native terminal with preferred Consolas, bundled JetBrains Mono fallback, and a separate dark toolbar.

**The Native Shell Rule.** Keep the native titlebar, window buttons, application menus, folder panel, and parented lifecycle dialogs. Do not simulate them in the WebView.

The owning sources are `index.html`, `src/style.css`, `src/main.ts`, `src/settings.ts`, `src/projects.ts`, `src/dashboard.ts`, `src/project-policy.ts`, `src/boundaries.ts`, and `src/operations.ts`. Native rules come from `src-tauri/tauri.conf.json`, `src-tauri/src/main.rs`, `src-tauri/build.rs`, `src-tauri/native/WindowAppearance.h`, `src-tauri/native/WindowAppearance.m`, `src-tauri/src/menu.rs`, `src-tauri/src/lifecycle.rs`, and the terminal files under `src-tauri/native/`. `PRODUCT.md` owns product truth. The Turkish `imp-plan.html` report has independent styling and is outside this application design system.

The classic native header implementation uses public AppKit APIs through a C bridge after the native window is built and before `frontend_ready` shows it. It removes `NSWindowStyleMaskFullSizeContentView`, assigns `NSAppearanceNameDarkAqua` to that window, and sets `titlebarAppearsTransparent` to `NO`. This keeps WebView content below the native titlebar. The opaque window background is #121212. Tauri retains its Dark window theme, visible titlebar, and native window buttons. The document root and color-scheme meta value are light; the body inherits that scheme and uses Pale Workspace with white controls. The sidebar retains its dark color scheme. Native controls and typography remain owned by macOS.

Earlier config-only and AppKit attempts still appeared light in the user's running app. The later transparent-titlebar attempt with a dark document root/meta failed, and the user reported that the bar was gone. Final captures after an authorized restart confirm the classic native titlebar is dark, visible, and above content at window widths of (1020px) and (1512px) in system Dark. The evidence is `artifacts/form-controls/project-standard.png`, `artifacts/form-controls/settings-final.png`, and the associated acceptance and finish-review records. Native verification at the (760px) minimum and a fresh capture of this titlebar in system Light are outside this pass's captured scope.

## Colors

The fixed composition pairs a dark native window header and solid dark sidebar with a pale workspace and white controls. Only the Projects library has a dot grid. Blue identifies primary actions, sidebar selection, focused field borders, and checkbox accents. Button keyboard focus uses text underlines or pale surfaces. The sidebar application name is white beside the existing icon. Frontmatter is normative. Shell color names map to CSS custom properties in `src/style.css`.

The user-confirmed palette comes from the ChatGPT conversation “Renk paleti oluşturma” (`6af337ab-0d84-83ed-a038-1e86c441b113`). It establishes the yellow, dark, and blue values represented by `terminal-foreground`, `sidebar`, and `accent`. The user's current choices assign pale neutral to the workspace canvas, light blue to its dot grid, blue to workspace buttons, light text to blue fills, and white to the sidebar application name. Yellow remains in the existing icon and the pinned terminal theme. Other neutrals, hover colors, borders, and text-selection tints are retained or derived support colors from the implementation. Their exact values are not attributed to that conversation.

### Primary

- **Action Blue** (`accent`) identifies the current primary workspace action. `accent-hover` provides the implemented hover state.
- **Interaction Blue** (`sidebar-selection`, `focus`) marks the current sidebar destination, focused field borders, and native checkbox accents. Sidebar keyboard focus underlines text in its inherited color.

### Neutral

- **Pale Workspace** (`background`) defines the main canvas and compact project header.
- **Workspace Dot** (`workspace-dot`) defines the explicitly requested dot grid only in the Projects library. Selected projects and Settings use the plain base canvas.
- **White Control Surface** (`control-background`) defines fields, secondary buttons, the removal dialog, and destructive-button text.
- **Settings Information Surface** (`settings-group`) fills the rounded General and host-tool information groups.
- **Settings Section Divider** (`settings-divider`) separates General information, the Brig form, and Mac checks.
- **Field Border** (`field-border`) defines the resting border of editable text, number, select, and multiline fields.
- **Support Surface** and **Workspace Divider** (`surface`, `line`) define project monograms, secondary controls inside dialogs, removal-preview backgrounds, table edges, and divided rows.
- **Dark Sidebar**, **Sidebar Divider**, **Sidebar Text**, and **Sidebar Hover** (`sidebar`, `sidebar-line`, `sidebar-text`, `sidebar-hover`) define the persistent navigation region. The terminal toolbar uses the terminal background with Sidebar Divider, Sidebar Text, and Sidebar Hover.
- **Brand White** (`sidebar-brand`) identifies the sidebar application name beside the existing icon.
- **Primary Text** and **Secondary Text** (`text`, `secondary`) distinguish labels from paths, explanations, values, and status text. Icon actions become Primary Text on hover, press, and keyboard focus.
- **Workspace Text Selection** (`selection`) uses the implemented pale blue tint for text selection and secondary or icon button feedback. Sidebar text selection uses Interaction Blue with `sidebar-selected-text`.
- **On Accent** and **Selected Sidebar Text** (`on-accent`, `sidebar-selected-text`) use the user's explicit light text choice on blue primary buttons, their hover state, current navigation, and sidebar text selection.
- **Error Text** (`error`) marks errors and destructive actions. A destructive primary button uses the error color as its fill with White Control Surface text.
- **Higher Contrast Divider** (`contrast-line`) replaces workspace dividers, sidebar dividers, and resting field borders when the system requests more contrast.
- **Terminal Canvas**, **Terminal Text**, **Terminal Cursor**, and **Terminal Selection** use the pinned `terminal-` tokens. Selection has its own foreground and background.
- **Terminal Divider** (`terminal-divider`) records the normal dark toolbar bottom line, which uses the sidebar divider role. The application has no terminal split.
- The sixteen indexed `terminal-ansi-0` through `terminal-ansi-15` colors retain the exact order from `TerminalTheme.h`. They are terminal protocol colors, not additional application action accents.

**The Fixed Composition Rule.** Keep the native window header and sidebar dark, the workspace pale, and controls white, independent of operating-system appearance. Keep the requested dot grid only in the Projects library. Selected-project views and Settings keep the pale base canvas. Keep the terminal view and its separate toolbar dark. Tauri's Dark theme and the explicit per-window AppKit Dark Aqua appearance own classic native chrome, with WebView content below the titlebar. Keep the document root/meta and body color scheme light for the pale workspace and white controls. Settings describes the composition as “Dark sidebar · Light workspace”.

**The Accent Text Rule.** Use On Accent light text on blue primary buttons in normal and hover states. Use Selected Sidebar Text light text on blue navigation and sidebar text selection. Preserve this explicit user choice; do not present this pairing as an AA contrast pass.

**The Terminal Palette Rule.** Preserve the pinned foreground, background, cursor, selection colors, and all sixteen ANSI entries. Do not derive them from shell colors.

## Iconography

**The Icon Stroke Rule.** All Lucide icons use `stroke-width="1.5"` in the `0 0 24 24` SVG viewBox. Apply this value to navigation icons, action icons, branch marks, activity indicators, close controls, and form chevrons. Keep `currentColor`, rounded caps, and rounded joins. New Lucide icons follow the same rule at every displayed size.

## Typography

The WebView UI uses the Inter stack in `typography.body`, including controls, labels, status text, policy documents, and removal previews. Inter is the locally bundled Google Fonts variable asset at `src/assets/fonts/Inter-Variable.ttf`. Its normal style supports weights (100–900), uses `font-display: swap`, and is preloaded by `index.html`. Root rendering uses `font-synthesis: none` and `text-rendering: optimizeLegibility`.

Native macOS titlebars, menus, folder panels, and lifecycle dialogs retain system typography. The WebView removal dialog uses Inter. Terminal glyphs use the separate native font preference below.

- **Title** serves Projects and Settings. **Project Title** serves the selected project, including its compact terminal header. Both use the established heading line height.
- **Body** serves navigation, button text, values, and ordinary status text. **Description** serves the Projects library explanation.
- **Brand** gives the white sidebar application name medium weight beside the existing icon. **Section Label** separates Settings sections below its unchanged Title heading. **Label** serves action labels, field labels, and disclosure summaries.
- **Field** serves text and number inputs, native selects, and multiline entry. **Field Hint** gives field-label explanations and network help a consistent readable rhythm. **Settings Term** gives information-row labels a medium weight below section headings.
- **Hint** serves supporting details and the compact terminal header path. Supporting paragraphs use a line height of (1.5).
- **Project Row Name** serves the library name column. **Project Meta** serves Git branches, the fixed path-and-reference line, and sidebar feedback. **Project Monogram** serves initials at the same size in library and sidebar rows. Sidebar project names use body size with label weight; saved session names use body size.
- **Table** and **Table Heading** serve project and session data, with tabular numbers. **Session Title** and **Session Date** distinguish each session name from its timestamp. **Session Tab** serves the Sessions, Archived, and Trash controls.
- **Usage Value** serves project totals. **Usage Percentage** keeps the cached-input percentage smaller and lighter than the project count. **Usage Session Title** carries the full conversation title inside the usage dialog. **Request Usage** serves exact per-request counts, model, rates, and missing-data notes. **Setup Action** serves the centered environment entry.
- **Document** serves policy JSON and removal previews in Inter at the frontmatter size. Preserve whitespace wrapping and allow long content to break.
- **Terminal** prefers installed Consolas at the pinned point size. When Consolas is unavailable, use the bundled Google Fonts JetBrains Mono asset at `resources/fonts/JetBrainsMono-Variable.ttf`. Both paths use (14pt). CoreText registers the bundled fallback for this process only; it does not install the font for the user. Consolas itself is not bundled. Terminal creation fails if neither font can load.

Inter and JetBrains Mono use the bundled OFL notices. Lucide icon licenses, font notices, immutable source references, and asset hashes are recorded in `resources/licenses/ui-components.json`.

Terminal cell height is based on font metrics, not CSS line height: `ceil((ascender - descender + leading) × 1.5)`. This adds the pinned (50%) height. CoreText renders terminal glyphs and applies terminal bold and italic styles.

**The Readable Identity Rule.** Keep the project heading, small branch label, and selectable path-and-reference line compact. Truncate overflow without increasing header height; preserve full project names, paths, and branches in tooltips.

**The Full Session Title Rule.** Keep saved session titles on one line with an ellipsis. Preserve the complete title in the native tooltip and accessible name.

## Layout

The full-height shell uses `components.sidebar` and a flexible main pane with a zero minimum track width. Its content minimum height is (400px). The native window opens at (1020 × 700px) and has a minimum of (760 × 500px).

Only the visible Projects library receives the dot grid over Pale Workspace. A radial gradient draws Workspace Dot circles with a (1px) radius on a repeating (20 × 20px) grid. Selecting a project or showing Settings removes the pattern. Keep the sidebar solid, controls white, and the native terminal on its pinned canvas.

The sidebar header places a (36 × 36px) application icon beside the white “Tugboat” name. The “Agent Sandbox” subtitle sits (3px) below the name and fits on one line at the minimum window width. The row centers the icon beside both lines with an (8px) gap and a margin of `0 0 20px 10px`. Each navigation destination pairs a Lucide icon (16 × 16px) with its label and an (8px) gap.

On launch, the main window maximizes to the screen's usable area, including its native titlebar. Menu bar and Dock space follow macOS screen settings.

The project area fills the sidebar's remaining space and scrolls vertically. Its minimum height is zero, top margin is (12px), and inner padding is (3px). Horizontal overflow stays hidden. The application header and navigation destinations do not shrink. Settings stays below the scrolling area. There is no sidebar add-project control.

The Projects header stays at the top of the scrolling workspace. Its right-aligned Add new project button has a leading plus icon. The library explanation and table sit in a full-width content region capped at (1300px), with `24px 32px 40px` padding. The table columns are Project (48%), Branch (32%), and Last opened (20%). An empty library keeps the same header and shows a centered explanation with `120px 12px` padding. Add new project remains in the header.

The selected-project header uses `components.workspace-header`: sticky at the top, a minimum height of (100px), and a bottom divider. Its first line pairs the one-line project name with a small branch label, separated by (16px). The branch takes at most (40%) of the row. Its second line contains only `folder path · Brig session reference`, with an (18px) line height and ellipsis overflow. The settings icon stays on the right. The selected project uses the same full-width content region as the library. An unready environment shows a centered setup action in an area at least (400px) tall.

Ready projects show five totals in a grid: total tokens, input, cached input, output, and total API cost. Four equal tracks precede a slightly wider cost track; the gap is (16px). The final value aligns right. A plain usage note follows. The session toolbar wraps with a (16px) gap and `32px 0 18px` margins. Session views stay left; New Chat and Return to chat or terminal stay right. The session table has a minimum width of (650px), with local horizontal scrolling when needed.

Project settings and session usage use wide HTML dialogs, bounded to `min(900px, calc(100vw - 64px))` and a maximum height of `calc(100vh - 64px)`. Each open dialog is a vertical flex column. Its header does not shrink, has `20px 28px` padding, and keeps the close action visible. Its body scrolls vertically with `12px 28px 28px` padding. CPU and memory share two flexible columns; image verification and network share another pair. Each pair uses a (16px) gap.

Showing the terminal keeps the project identity and settings icon, then places the independent dark toolbar and native terminal below it. The header contracts to a minimum height of (86px) with `16px 24px` padding. The ordinary project content is hidden except for errors. The terminal panel fills remaining height with a minimum of (160px); its surface has a minimum of (100px). Toolbar actions wrap separately from terminal text. AppKit sizing subtracts the pinned inset on every side before deriving grid dimensions. Resize the real terminal and pseudo-terminal together.

Global Settings uses a centered container capped at (670px), with `36px 40px` padding. Section headings use `32px 0 12px` margins. Two pale section rules use a (1px) top border and `20px 0` margins. A directly following heading has no top margin. The first direct field label inside each Settings fieldset has no top margin. Network help follows its select with an (8px) top margin. Action rows wrap with (8px) gaps. Policy documents have a scrollable maximum height of (260px). Removal previews stay inside a scrollable dialog bounded by the viewport.

At the compact desktop breakpoint (900px and below), use `components.sidebar-compact`. Header padding becomes `22px 24px`; ordinary project content padding becomes (24px). The totals become three columns with a (22px) row gap, and cost aligns left. Settings padding becomes `32px 28px`. The terminal header retains its compact geometry. Main content and sidebar lists scroll independently. This is a macOS window layout; no mobile layout is implemented.

**The Project Scope Rule.** Keep project setup, sign-in, policy, diagnostics, and environment controls in the scoped settings dialog. Keep Add new project in the Projects library header.

## Elevation & Depth

Tonal surfaces and one-pixel dividers provide normal hierarchy. Buttons have no shadow. Modal dialogs use the `dialog-modal` shadow and a dim backdrop; exact values live in the sidecar. Tables, settings groups, and ordinary project content remain flat. Native titlebars, folder panels, and close or quit dialogs retain system rendering.

No CSS transition duration or pressed movement is declared. Buttons change fill or text treatment without adding borders. The native cursor follows terminal focus and cursor style; its blink is suppressed when the system requests reduced motion.

**The Quiet Depth Rule.** Use shadow only for the implemented modal dialogs. Keep controls, tables, settings groups, and disclosures flat.

## Shapes

Use the frontmatter radii for navigation, icon and secondary buttons, primary buttons, tables, settings groups, setup actions, fields, and dialogs. Inputs and removal-preview backgrounds share the field radius. One-pixel borders define fields, table edges, settings groups, removal-dialog edges, and toolbar separation. Wide settings and usage dialogs have no edge border. Buttons have no resting, focus, or pressed border. Native window geometry remains owned by macOS.

## Components

### Buttons

Primary workspace actions use Interaction Blue, On Accent light text, label weight, and the established padding. Their minimum height is (36px). Hover and press change the fill. Disabled buttons use opacity (.65). Secondary buttons use White Control Surface, no border, the navigation radius, and a minimum height of (32px). Hover and keyboard focus use Workspace Text Selection. Inside dialog bodies, secondary controls rest on Support Surface. Destructive secondary actions use error text; removal confirmation uses an error fill and white text.

Buttons have no focus outline, box shadow, or pressed border. Keyboard focus underlines text with an offset of (3px). Icon buttons also use the pale selected surface and Primary Text on hover, press, and keyboard focus. Standard icon controls are (36 × 36px), with (8px) padding; session actions are (28 × 28px), with (6px) padding. Their icons keep full action names in tooltips and accessible labels.

Add new project uses a leading plus and opens the native folder picker. It appears only at the right of the Projects header. Its tooltip includes ⌘O. The native command is available in the library context. Selected projects expose a settings icon instead. New Chat uses the same primary plus-and-label pattern in the ready-project session toolbar. No demo-creation action is implemented.

### Sidebar and native menus

The sidebar header uses the icon from `src-tauri/icons/icon.png` and the white “Tugboat” name above “Agent Sandbox”. The icon has empty alternative text because the adjacent label names the application. Projects and Settings use the existing compact navigation buttons. Their inline SVG icons are Lucide Folder and Settings from (1.71.0), with `currentColor`, a (2px) stroke, rounded stroke caps and joins, `aria-hidden="true"`, and `focusable="false"`. Add new project uses an inline plus with the same size and stroke. Current page navigation uses Interaction Blue, Selected Sidebar Text light text, and `aria-current="page"`; the Projects and Settings buttons also use label weight. Hover uses Sidebar Hover. Sidebar text selection uses the same blue and light-text pair. Opening Settings moves focus to its heading. Returning to a visible terminal restores native first-responder focus.

Projects opens the library and lists registered projects in the scrollable sidebar. Selecting a project hides the other sidebar projects. The selected project remains above Sessions and Refresh; Projects always returns to the library. Sidebar project rows use a monogram, a one-line name, and the branch when available. Paths appear only in full name-and-path tooltips. The session list contains only active sessions. Loading, empty, and failure feedback stays inside the selected project's session area.

Saved session rows use `components.sidebar-session`. Their one-line title has an ellipsis, a full `title` tooltip, and the complete `aria-label`. A current saved session uses `aria-current="true"` and the shared blue selection treatment. Refresh uses `components.sidebar-text-button`. Keep the Sessions heading compact, with a (24px) top margin and its Refresh action aligned beside it. The list and status text do not displace Settings.

Keep native menu commands for Add new project (⌘O), Settings (⌘,), Return to project (⌘1), Return to terminal (⌘2), and Quit (⌘Q). Add new project belongs to the library context; the folder picker uses the same label. Return to terminal is enabled when a project terminal exists. Folder selection, project navigation, and session selection are disabled while the relevant setup action is busy. The real Edit and Window menus retain their platform actions.

### Settings groups and fields

Place Provider after General and before Records folder in application Settings. Show provider details and account controls only in this section. Project settings contain no provider details or sign-in and sign-out controls. Use the existing information surface, support text, full-width select, and wrapping action rows. Show the account, shared sign-in, Retry connection, sign-out, and pending sign-in controls. Saved account choices retain selection and focus during background updates. Project account errors use inline support text and an Open application settings action. This action opens Settings and focuses its Provider heading. Token values never enter the interface.


General and host-tool information use divided, rounded definition lists with Settings Information Surface fills and a line height of (1.5). Labels use Settings Term weight and do not shrink. Secondary values align right, allow a zero minimum width, and wrap long paths anywhere. Rows align on their text baselines. Appearance reads “Dark sidebar · Light workspace”. Settings retains its Title heading and uses Section Label for the section hierarchy.

Brig settings use editable, labeled text, number, select, checkbox, and multiline fields. Shared fields use White Control Surface, Field typography, their frontmatter padding, full width, a zero minimum width, a minimum height of (36px), and a (1px) Field Border. Enabled, unfocused hover mixes Field Border with (20%) Secondary Text in sRGB. On focus, the existing border becomes Interaction Blue and the outer outline is removed without a size change. Placeholders use Secondary Text at full opacity. Disabled fields use Secondary Text, Support Surface, and the divider border. User-invalid fields use Error Text for their border when unfocused. Increased contrast uses Higher Contrast Divider for the resting field border.

Native HTML selects retain native popup menus and keyboard behavior. Only the closed field removes platform appearance. Its right padding reserves room for the official Lucide Chevron Down from (1.71.0), displayed at (16 × 16px) and positioned (10px) from the right, vertically centered. The source asset is `src/assets/icons/chevron-down.svg`. Do not replace the native select with a custom popup. Checkboxes use Interaction Blue as their accent. Multiline fields resize vertically and have a minimum height of (90px).

Keep the implemented validation and hints: CLI auto-detection, profile length, CPU and memory limits, image verification, network preference, one allowed host per line, and telemetry status. Fields remain disabled if settings cannot load. Saving disables the save action and reports the result inline. Host checks, installation state, and doctor details remain grouped below the form.

### Project library and identity

Library rows live in a semantic table with scoped column headings. Each name button uses `components.recent-project`, a flat monogram, a one-line name, and a (12px) gap. The branch occupies its own column; unavailable branches and dates show an em dash. Paths are absent from the visible table and retained in row and name-button tooltips. Branch labels use a (14 × 14px) icon, a (6px) gap, one-line truncation, and a full tooltip. Table rows use divider lines and a pale hover fill.

Order every registered project by dedicated `lastOpenedAt`, independent of setup's `updatedAt`. Legacy records use `updatedAt × 1000` until the last-opened value is available. Do not reorder the library for setup changes alone. Selecting a project hides the library table and shows its fixed identity, settings icon, and dashboard. The project name and folder path retain exact tooltips. The second header line contains only the path and Brig reference.

### Scoped project settings

The header settings icon and the Review project settings action for unresolved configuration open the same wide project dialog. Normal project opening starts the saved environment in the background and shows inline progress; it does not open this dialog or focus a terminal. Running verified environments and starts already in progress are reused. The dialog has a static Project settings heading and an accessible close icon. Editable preferences include project name, CPU, memory, image verification, and network. Saved project preferences override app defaults for this environment. Stop the environment before changing resource options. Saving reports its result inside the dialog.

Preparation, guest terminal, ChatGPT sign-in, sharing plan, network policy, boundaries, diagnostics, tests, stop, and Brig-session removal stay inside this scope. Sharing plans use compact divided definition rows. Advanced tools use native disclosure summaries with label weight and divided sections. Status text stays near its action. Policy JSON uses Document type. Externally managed policy content stays read-only when editing is disabled. Boundary results name the outcome and show target, expected behavior, and observed behavior; color alone does not carry the outcome.

### Session table and usage

Ready projects show the semantic session table with Session, Input, Cached, Output, Total, API cost, and Actions columns. The active view is labeled Sessions; the other views are Archived and Trash. These controls use `aria-pressed` in a named group. New Chat remains a labeled primary action. Session titles stay on one line, with full tooltips and accessible names. Timestamps appear below them. Numeric columns align right and use tabular figures.

Usage appears first as project totals, then per session, then per prompt request in the usage dialog. The project summary always includes archived and deleted history. Cache reads are a subset of input. Display cached input as `count (cached / input percentage)` in all three places; use at most one decimal place and (0%) for zero input. At project totals, the parenthesized percentage uses the smaller, lighter Usage Percentage role. Standard-width wrapping of the count and percentage is accepted. Rows abbreviate counts of (10,000) or more with compact notation. Exact input, cached count and percentage, output, total, and API equivalent remain in numeric-cell tooltips.

Usage, Archive, Delete, and Restore are compact icon actions with explicit tooltips and accessible names. Active rows offer Usage, Archive, and Delete. Archived rows offer Usage, Restore, and Delete. Trash offers Usage and Restore. Archive hides a chat from active views. Delete moves it to Trash. Restore returns it to the active view. These states preserve source rollout files and historical totals. Unavailable session files retain usage and a visible status; their resume action stays disabled. End the project's terminal process before archiving or deleting.

The wide usage dialog has the static accessible heading Session usage. Its body displays the full session title, which can wrap, then one table per prompt with recorded requests. The table heading includes the prompt number and status. Each request occupies a row with the model, exact input, cached count and percentage, cache writes, output, and API equivalent. Show the recorded tier under the model and the rate date and any pricing explanation under the request cost. The footer gives the total API equivalent for the prompt. Use native HTML dialog behavior through `showModal()` and `close()`; do not replace it with a custom overlay role. The native terminal is hidden behind an open dialog and restored when it closes.

Label all USD amounts as API equivalents and state that ChatGPT subscription billing is separate. The bundled rate snapshot is dated (2026-11-10). Keep assumed tiers, missing cache information, and unpriced requests explicit. Unknown cached counts show Unknown or a known subtotal plus unknown; do not calculate a percentage from incomplete coverage. Fully unpriced sessions show Unavailable; partial totals retain `+ unpriced`. Do not treat missing data as zero or update stored request prices when the catalogue changes.

Saved sessions come from the selected Brig guest home's Codex index and rollout metadata. Include guest working directories inside the project and its subdirectories. Reading history does not start a virtual machine. Show sessions by most recently updated first, including while automatic environment startup is in progress. Keep loading, empty, and failure states inline. Retry environment restarts a failed background check; an explicitly stopped environment offers Start environment. New Chat and resume wait for verified readiness. Each project can run multiple Codex chats. Selecting an open chat restores its own terminal and scrollback. New Chat starts another process. End process affects the selected terminal. End that session's terminal before archiving or deleting it.

The sidebar's right-edge divider resizes it from a 280px minimum and keeps at least 480px for the workspace. Arrow keys adjust the focused divider in 16px steps. Home restores the minimum and End uses the maximum. Preserve the preferred width across launches and clamp it when the window narrows. The one-pixel divider changes to Accent on hover, keyboard focus, and drag. Live terminal bounds follow the resized workspace. A 14px rotating Lucide loader precedes the title of a working or thinking agent. Idle and ended sessions have no loader. Reduced motion uses the same static icon.

The shipped dashboard review accepted native captures at (1020px) and (1512px) logical widths at Retina (2×). Its evidence is in `artifacts/project-dashboard/acceptance.json`; the design detector reported no findings. The wide settings capture is from the first pass with unchanged modal layout. Archive and Trash captures from that pass establish behavior before percentages were added. The minimum (760px) layout and long-list scrolling were assessed in source only. Quit-warning text was observed through native accessibility; its blank image was discarded, and cancellation preservation was not independently established in this pass. Earlier genuine-session resume evidence belongs to the separate project-navigation review.

### Terminal header, toolbar, and surface

The pale compact project header preserves the name, small branch, path-and-Brig-reference line, and project settings icon. The independent dark toolbar shows purpose or process status, Return to project, and End process. Its caption can truncate to one line with an ellipsis, and its full text remains in the tooltip. The action group does not shrink and can wrap, so caption length does not hide its controls. Toolbar secondary buttons use the terminal background and Sidebar Text. Hover uses Sidebar Hover. Keyboard focus underlines the label. The toolbar retains its bottom divider. Toolbar text and controls use Inter; native terminal glyphs use the separate 14pt font preference. Status text uses the terminal cursor color. The toolbar remains a distinct terminal widget within the pale workspace.

The terminal itself is an AppKit `NSView` rendered through CoreText, above and outside the WebView. It uses public `libghostty-vt` (1.0.0) and a real Rust pseudo-terminal. The WebView only supplies layout bounds and the surrounding controls. Do not replace the terminal with a DOM text area or a rendered transcript.

Preserve native input, input-method composition, text selection, scrollback, clipboard actions, keyboard encoding, focus ownership, and the exposed accessibility text-area role and text values. The terminal inset comes from `spacing.terminal-inset` on each side. The cell-height metric and pinned palette are recorded above and in the sidecar. These implementation facts do not certify complete assistive-technology behavior.

Return to project hides the terminal without ending its process. Return to terminal restores it. End process directly ends the named terminal process. Closing the window or quitting with a live process uses the parented native warning with Cancel and End terminal processes; its message states that Brig environments, guest homes, and conversation history are preserved.

**The Terminal Space Rule.** Keep the project header compact and the toolbar independent. Give remaining space to the native terminal without changing its pinned metrics.

### Removal preview

Removal uses the implemented modal dialog, with the session reference, workspace, preview text, and external-session warning when needed. The user must type the exact session name before Remove session is enabled. The optional guest sign-out checkbox is enabled only when an environment exists. Errors remain inside the dialog. Cancel closes the dialog and returns focus to Preview removal. Keep this removal flow distinct from End process and Stop Brig environment.

## Do's and Don'ts

### Do:

- **Do** preserve real macOS windowing, menus, folder panels, and parented close or quit warnings.
- **Do** keep English application labels, locally bundled Inter UI typography, native system typography, and the fixed dark-sidebar/pale-workspace composition.
- **Do** preserve light text on blue fills, the library-only dot grid, plain selected-project and Settings canvases, white controls, and blue field borders on focus.
- **Do** retain the classic dark native window header above WebView content while keeping the document root/meta and body color scheme light.
- **Do** keep Settings information groups white and rounded, with pale section dividers, readable section headings, and wrapping values.
- **Do** use the shared Field typography, spacing, and border states while retaining native select popup and keyboard behavior.
- **Do** show “Tugboat” above “Agent Sandbox” beside the application icon.
- **Do** keep Add new project in the Projects header, project controls in the scoped settings dialog, and New Chat in the ready-project toolbar.
- **Do** keep every registered project in last-opened order and show only the selected project's saved sessions in its sidebar context.
- **Do** preserve full session titles in tooltips and accessible names while keeping list rows on one line.
- **Do** keep Settings below the scrollable project area and terminal controls visible beside a bounded caption.
- **Do** use `stroke-width="1.5"` for all Lucide icons, including Folder and Settings beside their navigation labels.
- **Do** preserve the fixed dark terminal palette, Consolas preference, bundled JetBrains Mono fallback at 14pt, native cell metrics, and equal terminal insets.
- **Do** retain underline or surface keyboard focus, current-navigation semantics, inline status, compact selectable paths, and full identity tooltips.
- **Do** keep policy, boundary, and session details grouped and preserve their actual enabled, disabled, and read-only states.
- **Do** preserve project totals through archive, Trash, and restore, with input, cached input, output, total, and API equivalent visible.
- **Do** show cached-input percentages in parentheses at totals, rows, and prompt requests; keep exact row counts in tooltips.
- **Do** retain the dated API-equivalent note and make assumed, unknown, and unpriced information explicit.
- **Do** retain standard HTML dialog behavior, static accessible headings, and the full session title in the usage body.

### Don't:

- **Don't** recreate native shell affordances as web chrome.
- **Don't** import the independent implementation report's styling or Turkish copy into the English application.
- **Don't** switch the sidebar or workspace colors with operating-system appearance.
- **Don't** apply the library dot grid to selected projects, Settings, or dialog examples.
- **Don't** add sidebar or selected-project Add new project controls, or a demo-creation flow.
- **Don't** add button focus rings, pressed borders, shadows, or movement.
- **Don't** apply Inter to terminal glyphs, bypass the Consolas/JetBrains Mono preference, recolor the terminal, or change its pinned geometry.
- **Don't** let long project identities expand the fixed header or replace full identity tooltips with visible path rows in the library.
- **Don't** show environment preparation or sign-in actions while project terminals run. New Chat remains available for parallel Codex chats.
- **Don't** present a split pane, DOM terminal, chip system, or mobile layout as an implemented component.
- **Don't** present API equivalents as ChatGPT subscription charges or missing rates as zero cost.
