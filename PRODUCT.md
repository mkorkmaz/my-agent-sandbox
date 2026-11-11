# Tugboat

**Agent Sandbox**

<!-- impeccable:product-schema 1 -->

## Platform

web

The frontend runs inside a Tauri desktop application for Apple silicon Macs with macOS 15 or later. Native macOS windowing, menus, folder dialogs, and keyboard behavior govern the interface. The app uses a fixed dark sidebar and light workspace.

The main window opens maximized to the screen's usable area with its native titlebar. All Lucide interface icons use `stroke-width="1.5"` as the shared design rule.

## Stack

The implementation plan uses Tauri 2, Rust, Vite, and TypeScript. Verified versions are recorded in config/development-versions.json.

## Product Purpose

The user selects a project folder, configures Brig, and works in an interactive Codex session through an embedded libghostty terminal. An open conversation supports subsequent messages and approvals. Saved conversations can be resumed.

Projects lists registered folders in a table, ordered by last opening, with current Git branches and path tooltips. Add new project, with a leading plus icon, appears only at the right of the Projects header. The sidebar lists projects until one is selected, then shows that project's active Codex sessions. Projects always returns to the library. Session titles remain on one line with an ellipsis and a full-title tooltip. The sidebar resizes from a 280px minimum using its right-edge divider or the keyboard. Its preferred width persists and the workspace retains at least 480px.

Each project supports multiple simultaneous Codex chats in its Brig environment. Selecting an open chat restores its own native terminal and scrollback while other chats continue. A spinning icon precedes the title while that agent is working or thinking. Reduced-motion preferences retain a static activity icon. New chats appear immediately in the sidebar and acquire their saved title once history is available. End process affects the selected terminal. Each root chat appears once in the table and sidebar; its usage includes all linked subagents, including nested subagents. Normal conversation forks remain separate chats. Subagents cannot be resumed or have their saved states changed independently. Unresolved relationships have a read-only Unlinked subagents recovery area.

The project header stays fixed, with its name and small branch label, then one line containing only the folder path and Brig session reference. The settings icon on the right opens a wide modal for project preferences, sign-in, policy, diagnostics, and environment actions. Projects that need preparation show a centered setup action. Ready projects show the session table, token and API-equivalent totals, and New Chat. Root chats can be archived, moved to Trash, and restored. These actions change the root's state without changing its subagents' recorded states, source rollout files, or historical project usage. Archive and deletion require the root's and subagents' terminal processes to be ended.

Saved sessions and token events come from the selected Brig guest home, filtered by the guest project path and its subfolders. Reading history does not start the environment. Exact source ownership and parent relationships support the observed guest Codex 0.167.0 format. Physical request identity combines the verified guest-home namespace, provider, origin thread ID, and response ID. Guest namespaces remain isolated; equal counters do not prove identity. Legacy records retain their usage with unknown source coverage until reliable evidence resolves ownership.

Session usage separates own, subagent, and combined amounts. Expandable agent sections retain their own turns and requests. User prompt counts remain separate from agent turn counts. Proven prompt attribution links to source requests or unmeasured turns; unassigned work remains explicit and included. A selected child's details include only that child and its descendants, excluding ancestors and siblings. Open details refresh when the selected subtree's records or relationships change, preserving disclosures, focus, and scroll position.

Cached input shows a percentage only when its relevant counts are fully known. Aggregate percentages divide summed cached input by summed input. A turn without request measurements has unknown usage; an exact zero-token request is measured zero. Complete, partial, or unknown coverage describes persisted source records, with no guarantee of complete account billing or every short-lived subagent record. Cache and pricing knownness remain separate. Locally stored requests retain immutable price snapshots from the catalogue verified on 2026-11-10. API equivalents label missing information and remain separate from ChatGPT subscription billing.

Usage ledgers use schema 2. The first migration write preserves a schema 1 ledger as a byte-for-byte, immutable `.json.v1.backup`. Migration preserves saved states, request keys, amounts, dated prices, and prior corrections. Verified source replay can attach ownership or correct amounts without counting requests again; amount and rate corrections retain separate evidence. Conflicting backups and unsupported schemas stop writes and preserve the files. Closing the application while an agent or terminal process runs requires confirmation.

## Capabilities and Constraints

The application provides Brig settings and project actions based on the referenced Brig and Codex setup guide. Missing project configuration is prepared automatically. Each implementation item is completed and reviewed separately when the user requests it. imp-plan.html records progress, measured tokens, API equivalents, and comparisons with the original estimates.

## Operating Context

The primary workflow starts with a local folder. Brig runs from the selected external CLI. The app includes a pinned hull CLI with a patch for spaces in shared folder paths; config/hull-compat.json records its source and toolchain. The installed hvi and Vz runners remain external. Codex runs in the selected Brig environment. The UI language is English. The interface uses locally bundled Google Fonts Inter, including controls and document previews. Native macOS titlebars, menus, and dialogs retain system typography. Lucide Folder and Settings icons accompany the sidebar labels.

The fixed palette uses #1E1E1E for the sidebar, #F1F1F1 for the workspace canvas, and #4D8DFF for primary workspace actions and sidebar selection. Only the Projects library has a dot grid with #A8C7FF dots, a 1px radius, and 20px spacing. Text on blue fills uses the user-specified #F1F1F1. The sidebar shows the application icon beside the white Tugboat name, with Agent Sandbox underneath. Fields, secondary buttons, and dialogs use white surfaces. Focused text fields use a 1px #4D8DFF border. Buttons have no focus or pressed border; keyboard focus uses an underline or a changed surface. The workspace uses light appearance and the native window header uses dark appearance, independent of system appearance.

Settings uses a plain #F1F1F1 canvas and white rounded information groups, with 1px #F9F9F9 separators between groups. Its type hierarchy separates the 24px page title, 16px section headings, 13px field labels, 14px control values, and 12px hints. Editable controls share a 36px minimum height and a light #B8B8B8 resting border. Native HTML selects use a Lucide chevron with the same control surface while retaining native option menus and keyboard behavior. The macOS window explicitly uses Dark Aqua appearance and an opaque native titlebar. Its content frame excludes the titlebar, and the document and form controls use light appearance.

The native terminal uses Consolas at 14pt. If Consolas is unavailable, it uses locally bundled Google Fonts JetBrains Mono. The dark terminal theme retains #FFE735 foreground text and the existing ANSI palette, cursor, selection, padding, and cell-height adjustment.
