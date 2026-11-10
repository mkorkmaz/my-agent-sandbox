# Tugboat

**Agent Sandbox**

<!-- impeccable:product-schema 1 -->

## Platform

web

The frontend runs inside a Tauri desktop application for Apple silicon Macs with macOS 15 or later. Native macOS windowing, menus, folder dialogs, and keyboard behavior govern the interface. The app uses a fixed dark sidebar and light workspace.

## Stack

The implementation plan uses Tauri 2, Rust, Vite, and TypeScript. Verified versions are recorded in config/development-versions.json.

## Product Purpose

The user selects a project folder, configures Brig, and works in an interactive Codex session through an embedded libghostty terminal. An open conversation supports subsequent messages and approvals. Saved conversations can be resumed.

Projects lists registered folders in a table, ordered by last opening, with current Git branches and path tooltips. Add new project, with a leading plus icon, appears only at the right of the Projects header. The sidebar lists projects until one is selected, then shows that project's active Codex sessions. Projects always returns to the library. Session titles remain on one line with an ellipsis and a full-title tooltip. The sidebar resizes from a 280px minimum using its right-edge divider or the keyboard. Its preferred width persists and the workspace retains at least 480px.

Each project supports multiple simultaneous Codex chats in its Brig environment. Selecting an open chat restores its own native terminal and scrollback while other chats continue. A spinning icon precedes the title while that agent is working or thinking. Reduced-motion preferences retain a static activity icon. New chats appear immediately in the sidebar and acquire their saved title once history is available. End process affects the selected terminal. Archive and deletion require that session's terminal to be ended.

The project header stays fixed, with its name and small branch label, then one line containing only the folder path and Brig session reference. The settings icon on the right opens a wide modal for project preferences, sign-in, policy, diagnostics, and environment actions. Projects that need preparation show a centered setup action. Ready projects show the session table, token and API-equivalent totals, and New Chat. Sessions can be archived, moved to Trash, and restored. These states preserve source rollout files and all historical project usage.

Saved sessions and token events come from the selected Brig guest home, filtered by the guest project path and its subfolders. Reading history does not start the environment. Cached input shows its percentage of input in parentheses wherever it appears. Per-prompt requests and immutable price snapshots are stored locally and updated after responses. API equivalents use a dated verified price catalogue, label missing information, and remain separate from ChatGPT subscription billing. Closing the application while an agent or terminal process runs requires confirmation.

## Capabilities and Constraints

The application provides Brig settings and project actions based on the referenced Brig and Codex setup guide. Missing project configuration is prepared automatically. Each implementation item is completed and reviewed separately when the user requests it. imp-plan.html records progress, measured tokens, API equivalents, and comparisons with the original estimates.

## Operating Context

The primary workflow starts with a local folder. Brig runs from the selected external CLI. The app includes a pinned hull CLI with a patch for spaces in shared folder paths; config/hull-compat.json records its source and toolchain. The installed hvi and Vz runners remain external. Codex runs in the selected Brig environment. The UI language is English. The interface uses locally bundled Google Fonts Inter, including controls and document previews. Native macOS titlebars, menus, and dialogs retain system typography. Lucide Folder and Settings icons accompany the sidebar labels.

The fixed palette uses #1E1E1E for the sidebar, #F1F1F1 for the workspace canvas, and #4D8DFF for primary workspace actions and sidebar selection. Only the Projects library has a dot grid with #A8C7FF dots, a 1px radius, and 20px spacing. Text on blue fills uses the user-specified #F1F1F1. The sidebar shows the application icon beside the white Tugboat name, with Agent Sandbox underneath. Fields, secondary buttons, and dialogs use white surfaces. Focused text fields use a 1px #4D8DFF border. Buttons have no focus or pressed border; keyboard focus uses an underline or a changed surface. The workspace uses light appearance and the native window header uses dark appearance, independent of system appearance.

Settings uses a plain #F1F1F1 canvas and white rounded information groups, with 1px #F9F9F9 separators between groups. Its type hierarchy separates the 24px page title, 16px section headings, 13px field labels, 14px control values, and 12px hints. Editable controls share a 36px minimum height and a light #B8B8B8 resting border. Native HTML selects use a Lucide chevron with the same control surface while retaining native option menus and keyboard behavior. The macOS window explicitly uses Dark Aqua appearance and an opaque native titlebar. Its content frame excludes the titlebar, and the document and form controls use light appearance.

The native terminal uses Consolas at 14pt. If Consolas is unavailable, it uses locally bundled Google Fonts JetBrains Mono. The dark terminal theme retains #FFE735 foreground text and the existing ANSI palette, cursor, selection, padding, and cell-height adjustment.
