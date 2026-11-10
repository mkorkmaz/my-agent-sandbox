# Brig Desktop

<!-- impeccable:product-schema 1 -->

## Platform

web

The frontend runs inside a Tauri desktop application for Apple silicon Macs with macOS 15 or later. Native macOS windowing, menus, folder dialogs, and keyboard behavior govern the interface. The app uses a fixed dark sidebar and light workspace.

## Stack

The implementation plan uses Tauri 2, Rust, Vite, and TypeScript. Verified versions are recorded in config/development-versions.json.

## Product Purpose

The user selects a project folder, configures Brig, and works in an interactive Codex session through an embedded libghostty terminal. An open conversation supports subsequent messages and approvals. Saved conversations can be resumed.

## Capabilities and Constraints

The application provides Brig settings and project actions based on the referenced Brig and Codex setup guide. Missing project configuration is prepared automatically. Each implementation item is completed and reviewed separately when the user requests it. imp-plan.html records progress, measured tokens, API equivalents, and comparisons with the original estimates.

## Operating Context

The primary workflow starts with a local folder. Brig runs from the selected external CLI. The app includes a pinned hull CLI with a patch for spaces in shared folder paths; config/hull-compat.json records its source and toolchain. The installed hvi and Vz runners remain external. Codex runs in the selected Brig environment. The UI language is English. The fixed palette uses #121212 for the sidebar, #FFE735 for primary actions and the sidebar brand, and #4D8DFF for selection and keyboard focus. The workspace and native window use light appearance regardless of system appearance. The terminal uses the user-specified dark Consolas theme.
