# Tugboat

**Agent Sandbox**

<img src="src-tauri/icons/icon.png" alt="Tugboat icon" width="128" height="128">

A macOS desktop app for running Codex and Claude Code in separate Brig environments, with a native terminal and resumable sessions.

Choose a local project folder and an agent. Each agent uses its own environment, policy, and guest home. The terminal supports interactive conversations and approval requests.

## Features

- Add project folders, see their Git branches, and return to projects in last-opened order.
- Browse saved Codex and Claude Code sessions together, with each agent named beside its chats.
- Prepare project policies and session bindings automatically, or use a matching Brig session.
- Connect ChatGPT and Claude once from Settings, before adding a project.
- Run multiple conversations per project and switch between their live terminals in the sidebar.
- Set CPU and memory limits, image verification, and allowed network hosts.
- Check Git status, review changes, and run project tests inside the guest environment.
- Reopen completed test and diagnostic output from **Terminals** in the sidebar. The app keeps up to 32 completed terminal views for the current app session; older views are closed. **Close output** discards a view. The separate limit of 16 live processes excludes these completed views.
- Inspect Brig diagnostics, logs, and file and network boundaries.
- Use native macOS menus and keyboard shortcuts, a fixed dark sidebar, and a light workspace.

The interface uses bundled Google Fonts Inter and Lucide sidebar icons. The native terminal uses Consolas at 14pt, with bundled Google Fonts JetBrains Mono when Consolas is unavailable. Fonts load locally.

## Requirements

- An Apple silicon Mac with macOS 15 or later.
- Internet access for the first download of guest images and boot assets.
- The `codex` Brig profile for Codex, or the bundled `tugboat-claude` profile for Claude Code.
- An account with access to the selected agent. Codex uses the shared ChatGPT account; Claude Code uses the shared Claude account.

The app includes Brig v0.5.2, a patched hull CLI, signed hvi/Vz runners, cosign, and the ext4 disk tools. Brig and hull use the bundled disk tools. The hull patch also supports shared folder paths that contain spaces. Homebrew is optional.

Tugboat uses a bundled, empty registry configuration for public guest images. It does not read the host Docker credential store. Use standalone Brig for private registry sign-in. Image signature verification stays required by default.

Brig response fixtures were captured with Brig v0.5.2 and the `brig.sh/v1alpha1` format. Version records are in [config/development-versions.json](config/development-versions.json).

## Build and install

The `v0.1.0-build.1` and `v0.1.0-build.2` releases contain ad hoc signed applications. Gatekeeper rejected both the unsigned build 2 disk image and the application inside it. Once a Developer ID signed and Apple-notarized DMG is available on [GitHub Releases](https://github.com/mkorkmaz/my-agent-sandbox/releases), open it and drag Tugboat into Applications. The source build instructions below also work for local development.

The trunk workflow builds an ad hoc signed app and verifies the packaged runtime. The separate Developer ID release script uses six secrets, `APPLE_CERTIFICATE` (base64 `.p12`), `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY` (`Developer ID Application: …`), `APPLE_API_KEY`, `APPLE_API_ISSUER`, and `APPLE_API_KEY_CONTENT` (the App Store Connect `.p8` private key). That script signs the owned helpers before recording their hashes, preserves the upstream runner signatures, and notarizes and staples the app and DMG. The release verifier checks signatures, tickets and Gatekeeper from a read-only mount. See [Tauri's macOS signing guide](https://v2.tauri.app/distribute/sign/macos/).

To build from source, install Node.js and npm, Rust and Cargo, and Xcode with its command line tools. Supported Node.js versions are 22.22.2 or later in the 22.x series, 24.15.0 or later in the 24.x series, and 26.0.0 or later. The recorded development environment uses Node.js 26.12.1 and Rust 1.99.1.

Run these commands from the project root:

```sh
npm ci
npm run tauri -- build --target aarch64-apple-darwin --bundles app
```

The build prepares the native dependencies automatically. The first build downloads pinned Ghostty, Brig, hull and e2fsprogs sources, Zig, Go, signed runners, cosign and dependency source notices. Downloaded archives are checked against their recorded SHA-256 hashes. Brig and hull release checksums also receive Sigstore signature checks. The complete host runtime is staged in `.build/brig-runtime/`. Disk tools target macOS 15 and link only to macOS system libraries.

Open the built application:

```sh
open "src-tauri/target/aarch64-apple-darwin/release/bundle/macos/Tugboat.app"
```

You can also copy `Tugboat.app` to `/Applications`.

### Releases from trunk

Every push to `trunk` runs the frontend, Rust, native terminal, and hull tests. After they pass, GitHub Actions builds the ARM64 DMG using the `version` in `src-tauri/tauri.conf.json` and attaches it to a release for the tested commit. Each push gets its own tag, such as `v0.2.0-build.42`. Rerunning the same workflow run keeps its original tag.

## Get started

1. Open **Settings** with **⌘,**.
2. Open **Providers**. Select **Sign in with ChatGPT** or **Sign in with Claude**.
3. Complete browser authorization. Enter the ChatGPT device code shown in Settings when requested.
4. Review **System Checks**, the resource limits, image verification, and network settings when needed.
5. Open **Projects**. Select **Add new project** or **⌘O**.
6. Choose **Codex** or **Claude Code** beside **New Chat** inside the project.
7. Review the project's **Sharing and execution plan** when requested.
8. Tugboat starts the selected environment and checks the guest agent. Already verified running environments are reused.
9. Select **New Chat**, or select a saved session from the project table or sidebar.

An empty **Brig CLI path** selects the copy inside Tugboat. System Checks shows the effective source, version and tool paths. A saved compatible custom path remains available. Clear that field to return to bundled Brig. Replace the app if a bundled tool is missing or fails its hash check.

To verify a packaged runtime, run `node scripts/release/verify-bundled-runtime.mjs /absolute/path/Tugboat.app`. The app also supports a read-only `Contents/MacOS/tugboat --runtime-check` command.

Run `npm run test:bundled-runtime -- /absolute/path/Tugboat.app` on an Apple silicon Mac to test both guests with bundled tools and system paths. Offline preflight blocks Homebrew reads and execution. Guest commands allow hvi to apply its own sandbox. The test uses a fresh private home, empty guest caches and an isolated runtime store. It checks offline tool discovery, signature verification, ext4 disks, project sharing, stop/restart and cleanup. Evidence under `artifacts/bundled-runtime/` identifies the actual host and test limits. Add `--clean-mac` to require Homebrew absence on the test host. Run `node scripts/release/package-clean-mac-test.mjs` to prepare the standalone test kit for that host.

When several Brig sessions match a folder, select the session to use. Project actions also provide a guest terminal, Git commands, test commands, and session controls.

**Projects** shows registered folders in a table, ordered by last opening, with current Git branches when available. Folder paths appear in hover tooltips. Add projects only from this page. Selecting a project replaces the sidebar project list with its active Codex and Claude Code sessions. Select **Projects** to return to the library. Session names stay on one line; hover to see the full name. Drag the sidebar's right edge to resize it from its 280px minimum. The divider also supports arrow keys, Home, and End. The app saves the preferred width and keeps at least 480px for the workspace.

Each project has a fixed header with its name, branch, and `folder path · Brig session` line. Choose the agent beside **New Chat** to select its environment. Its settings icon opens a wide modal for project name, CPU, memory, image verification, network, policy, and environment actions. Environment options apply to the selected agent and override the app defaults. Stop that environment before changing them. The dot grid appears only in the Projects library.

### Shared ChatGPT account

Manage accounts in **Settings → Providers**. Sign in before creating or selecting a project. Tugboat opens the official ChatGPT device page and shows its code in Settings. Codex environments use the connected account automatically. Each environment keeps its own policy, configuration, guest home, and chat history. Claude environments do not receive this account. A separate Mac Codex installation and a running guest are not required for sign-in.

Tugboat can use a saved project sign-in. If saved sign-ins belong to different users or workspaces, select the source project explicitly. End its existing Codex processes first. An older Codex daemon can retain credentials after its terminal closes; stop and reopen that Brig environment if Tugboat reports this condition. New chats run without this shared daemon.

The private account record is `<records folder>/chatgpt/session.json`. Its folder uses permission `0700`; the file uses `0600`. Only Tugboat renews the shared session. Guests receive access copies without the refresh token. The shared record is never mounted in a guest. Records must stay outside project folders and Brig guest homes. Custom `CODEX_HOME` locations and profile file or volume mappings are not supported for shared sign-in.

**Check connection** renews the shared session and updates its managed guest copies. **Retry connection** retries a failed transfer or renewal. Temporary failures preserve the account. If a renewal request was sent but its result is unknown, Tugboat requires another shared sign-in instead of reusing a possibly rotated refresh token. A sign-in cancelled before commitment preserves the previous account. A crash before a completed login is confirmed cancels that unfinished login on restart.

**Sign out of Tugboat** requires running Codex chats, ChatGPT sign-in processes, and Codex guest terminals to end first. It removes managed sign-in copies and reports whether remote revocation was confirmed. Removing or forgetting one project preserves the shared account for other projects. Project histories remain separate and are not deleted by account actions. Guest network policies still control model connections. Offline environments do not receive or renew access copies.

### Shared Claude account

Open **Settings → Providers** and select **Sign in with Claude**. The browser returns authorization through a temporary local callback. The same account serves managed Claude environments across projects. Sign-in does not need a project, a running guest, or a Mac Claude installation.

The private renewable record is `<records folder>/claude/session.json`, with directory permission `0700` and file permission `0600`. It moves with the other records. Tugboat owns renewal. The managed `tugboat-claude` profile receives an access copy in guest memory, with no refresh token. The stock guest's Python 3 applies that copy atomically and preserves independent connector credentials. Host Claude credentials are not imported.

Stopping an environment clears its memory copy. The shared account remains connected. Starting a chat restores the copy after checking the account and network policy. Transcripts remain in that environment's guest home. **Check connection** renews the shared account. **Sign out of Tugboat** requires Claude chats and guest terminals to end. It clears managed copies and reports unconfirmed remote revocation. Removing one environment preserves the shared account.

Claude uses the Brig boundary and manual permission requests. The Codex sandbox option does not apply to Claude. Provider authorization follows the Codex 0.167.0 and Claude Code 2.1.318 protocols. Retest browser authorization and access copies when updating guest agents.

### Chats and environments

Configured projects show saved sessions and known Codex usage while the selected environment starts in the background. **New Chat** and resume wait for that environment, guest agent, and account to be verified. Reopening a stopped project starts its selected environment automatically. Saved resource limits, policies, guest homes, and transcripts remain available. Startup failures offer **Retry environment** without repeating configuration. An environment that is already starting is reused. Explicit **Stop Brig environment** stays stopped until the user starts it or reopens the project.

**New Chat** starts another process in the selected agent's environment. Selecting an open chat restores its existing terminal and scrollback. Other chats continue running. Claude new chats receive a UUID from Tugboat. Resume uses that same native ID without creating a fork. Equal IDs in different environments remain separate chats. The activity icon uses Codex's working status; Claude does not use that status parser. **End process** closes only the selected terminal.

Each root chat appears once in the session table and sidebar. Each agent’s table totals include its own usage and all linked subagents, including nested subagents. The row shows the subagent count; normal conversation forks remain separate chats. **Archived** keeps a root chat out of the active list. **Delete** moves it to **Trash**, where **Restore** returns it to the active list. These actions target that environment's chat and preserve the project total. Source Codex rollout files and Claude transcripts remain intact. End the root chat's and subagents' terminal processes before archiving or deleting the root chat. Subagents cannot be resumed, archived, deleted, or restored independently. Agents with unresolved relationships appear under **Unlinked subagents**, with read-only usage details.

### Usage and API cost

Claude rows show recorded main-session and linked subagent tokens and offer **Usage** details. Input includes ordinary input, cache reads, and cache creation. Stable message IDs prevent repeated transcript records from multiplying usage. Claude coverage remains partial because missing transcripts, unverified relationships, and unrecorded work limit the recorded total. User prompt attribution is unavailable. Supported Claude models retain Anthropic token API equivalents with dated rates and explicit billing assumptions. Mixed-project totals include recorded usage from both agents. Missing or invalid counts show **Unknown**, or a known subtotal plus **unknown**, without an exact cache percentage.

Tugboat reads Codex token events and Claude message usage from their separate guest homes. Background readers check observed environments every two seconds. One `usage.sqlite3` in the verified records folder stores both agents’ session state, guest variants, turns, requests, historical prices, corrections, and parser cursors. Changed requests and cursor progress commit together. Dashboard reads use derived summaries; details read the selected family. Full prompts, answers, credentials, and raw events are not copied into the database. Chat identity includes the project, environment, and native session ID. Saved usage survives missing or unreadable transcripts. Claude subagent paths and immediate parent metadata follow the [Anthropic Agent SDK session reader](https://github.com/anthropics/claude-agent-sdk-python/blob/main/src/claude_agent_sdk/_internal/sessions.py). Each verified root/agent pair has a stable internal child UUID. Missing or invalid parent evidence retains measured usage under **Unlinked subagents**. Claude cache counts follow the additive input fields described in [Anthropic prompt caching](https://platform.claude.com/docs/en/build-with-claude/prompt-caching).

The usage icon opens **Session usage**. For a root chat with subagents, **Main agent**, **Subagents**, and **Combined** separate the root's own usage from delegated work. Expand a subagent to inspect its own agent turns and requests. User prompt counts and agent turn counts are separate. Proven prompt attribution links to the source request or unmeasured turn. Work without proven attribution stays under **Unassigned to a user prompt** and remains in the selected total. A selected child's details cover that child and its descendants only; ancestors and siblings are excluded. While the dialog is open, changes in the selected subtree refresh its details and preserve open disclosures, focus, and scroll position.

Exact source ownership and parent relationships are supported for the observed guest Codex 0.167.0 format. A physical request uses the verified guest-home namespace, model provider, origin thread ID, and response ID. Different guest homes stay separate. Matching titles, token amounts, or project paths do not establish request identity. Legacy counter records retain their saved usage with unknown source coverage until reliable evidence resolves ownership; they are not reset or deduplicated by equal amounts.

The bundled price snapshot was verified on 2026-11-10 against [OpenAI API pricing](https://developers.openai.com/api/docs/pricing), [GPT-6.1 Sol pricing](https://developers.openai.com/api/docs/models/gpt-6.1-sol), and [prompt caching](https://developers.openai.com/api/docs/guides/prompt-caching). It covers GPT-6 Astra, GPT-6.1 Sol, GPT-6.1 Terra, and GPT-6 Luna. Supported recorded service tiers apply their published multiplier; missing tiers use Standard and are labelled as assumed. Sol requests above 272K input tokens use the published full-request long-context rates. Other unverified models, tiers, or long-context rates remain unpriced.

The Anthropic catalogue was checked on 2026-11-13 against [Claude pricing](https://platform.claude.com/docs/en/about-claude/pricing), [model IDs](https://platform.claude.com/docs/en/about-claude/models/model-ids-and-versions), [prompt caching](https://platform.claude.com/docs/en/build-with-claude/prompt-caching), and [fast mode](https://platform.claude.com/docs/en/build-with-claude/fast-mode). Explicit supported IDs cover Fable/Mythos 5, 5.1, and 5.2; Opus 4.5, 4.6, 4.7, 4.8, 5, and 5.5; Sonnet 4.5, 4.6, 5, and 5.5; and Haiku 4.5 and 5.5. Only the verified dated 4.5 snapshots and aliases are accepted. Haiku 5.5 uses its higher input and output prices above 100,000 total input tokens, including cache reads and writes. Unverified legacy long-context prices remain unpriced.

Claude ordinary input, cache reads, 5-minute writes, 1-hour writes, and inclusive output use separate token rates. Recorded standard/batch service tier, supported fast speed, and supported US inference apply Anthropic modifiers. Priority Tier and unsupported modifiers remain unpriced. Missing or nullable billing metadata uses a labelled standard-speed, standard-tier, global estimate, with 5-minute writes when the lifetime split is unavailable. Malformed non-null metadata stays unpriced and persists as invalid across restarts. Later reliable metadata can resolve assumptions using the saved tariff, while correction evidence is retained. Existing unpriced Claude records can be backfilled from saved counts without starting a guest or making an API call.

Input, cache reads, and cache writes use separate rates. Reasoning already included in output is counted once. Missing cache counts, cache-write lifetimes, and incomplete counter coverage remain explicit; unpriced requests do not become zero-cost requests. Cached input shows its share of total input in parentheses in the project total, session table, and request details. Aggregated percentages divide summed cached input by summed input; they do not average request percentages. A percentage appears only when the relevant counts are fully known. Otherwise, the UI shows **Unknown** or a known subtotal plus **unknown**. The table abbreviates large token counts, with exact counts in hover tooltips. Archived, deleted, and temporarily unavailable source sessions retain their cost history.

An agent turn without recorded request usage has unknown token counts and API cost. A recorded zero-token request is measured zero, and a history with no turns remains empty. **Recorded source coverage** can be complete, partial, or unknown. It describes the persisted records in the selected scope, not complete account billing or a guarantee that every short-lived subagent left a readable record. Pricing and cache information can remain unknown even when source coverage is complete.

These USD amounts are token API equivalents, separate from subscription charges, account discounts, server-tool charges, and taxes. They are not an invoice. Recorded supported Anthropic inference geography is included. Imported history uses the bundled price snapshot on first pricing. Stored request prices do not change when a future build updates the catalogue. The reader records whether a tier came from a request, applied session settings, the turn context, or an assumption. Older ledgers are replayed once to recover recorded tiers. A correction uses the original dated price snapshot and retains the previous rates and cost as evidence. It does not count the request again. Missing source files or historical rates remain explicit.

SQLite uses schema 1. Legacy JSON ledger schemas 1 and 2 are separate import formats. Startup imports raw ledgers into a private staging database without changing prices. Typed field parity, source hashes, receipts, and SQLite integrity checks must pass before activation. The database is published first; the `.tugboat-data.json` marker changes from version 1 to version 2 last. Only then can the SQL writer start. The database uses permission `0600` inside a private `0700` records folder.

Before importing a schema 1 ledger, Tugboat creates its byte-for-byte, immutable `.v1.backup` beside the source. Existing schema 2 ledgers can have older backups with different content; those backups remain unchanged. Original JSON ledgers and backups stay at their existing paths and are read-only to the new usage code. No dual writing or retention cleanup runs. Migration preserves saved states, map slots, request order, token amounts, dated prices, correction evidence, and complete Codex cursor state. A conflicting backup, unknown field, unsupported version, or unsafe candidate stops activation and preserves the inputs.

After activation, saved Claude pricing policy version 1 runs before the first dashboard, details, or live usage read for that environment. It also runs offline. It processes selected and historical records, including unavailable and excluded records, without starting a guest or reading a transcript. Changed prices, correction evidence, summaries, and the policy version commit together. A failed commit leaves the policy pending and publishes no new price. Repeated reads skip a completed policy. Valid historical tariffs remain in use. Later source replay can attach proven identities or correct proven amounts without counting a request again. Amount corrections remain separate from rate and provider corrections.

The project registry also migrates to schema 2 and retains an immutable `projects.json.v1.backup`. Existing Codex session references, policies, guest homes, ledger paths, and prior ledger backups are preserved.

**Run project tests** detects configured npm, Composer, Cargo, or pytest tests. It opens a separate terminal process while chat terminals remain available.

### Keyboard shortcuts

| Shortcut | Action |
| --- | --- |
| ⌘O | Add new project from Projects |
| ⌘, | Open Settings |
| ⌘1 | Return to the project view |
| ⌘2 | Return to the terminal |
| ⌘Q | Quit the application |

## Files and network access

The selected project folder is shared with the guest with write access. Changes made inside that folder affect the local project.

New settings use these defaults:

| Setting | Default |
| --- | --- |
| CPU count | 4 |
| Memory | 4096 MB |
| Image signature verification | Required |
| Network | Access allowed by policy |
| Allowed hosts | `auth.openai.com`, `chatgpt.com`, `api.openai.com` |
| Brig usage telemetry | Off |

Brig network policies control connections made by guest commands and deny other destinations by default. Enter exact host names, one per line. Wildcards are not supported. Add hosts required by your project or sign-in flow.

The allowed hosts in Settings apply to new policies. Use **Project network policy** to inspect or change an existing project's policy. Stop the environment before editing its allowed hosts.

**Offline** mode disables guest network access. Shared provider sign-in uses the Mac network. Online chats require isolated guest networking.

Hosted Codex tools, including web search, have a separate scope from guest command traffic. Web search can return content from an OpenAI-maintained index. Tugboat does not read or set the effective `web_search` mode and does not label it as enabled or disabled. A result from a hosted tool does not show that a guest command reached that destination. See the [official Codex configuration reference](https://learn.chatgpt.com/docs/config-file/config-reference).

New and resumed Codex chats start with `--sandbox danger-full-access --ask-for-approval never`. Codex uses Brig's file sharing and network boundaries without an additional Codex sandbox. The app also provides optional checks for the Brig boundary.

**Use the Brig boundary with the agent sandbox disabled** is selected by default. Clear it to use Codex's `workspace-write` sandbox with command network access disabled. The approval policy remains `never` in both modes.

Boundary checks target the selected environment. They select comparison hosts from its effective guest network mode and resolved policy. Offline and deny-all modes expect blocked connections. CIDR, wildcard or advanced policies remain inconclusive when a suitable comparison cannot be verified. Brig/hull host rules authorize resolved addresses, so services on shared IP addresses can have a wider scope than their host names. A reachable comparison with possible address overlap also remains inconclusive. Boundary checks report diagnostic results and do not change the selected Codex sandbox mode. See the [pinned hull network policy behavior](https://github.com/brig-sh/hull/blob/7e50ac340d69be387312755c1f07ef6b0e9fec52/docs/network-egress.md).

Tugboat stores its records in `~/Library/Application Support/Tugboat` by default. The first launch migrates the earlier `sh.brig.desktop` records automatically. In **Settings → Records folder**, choose an empty folder, then select **Restart Tugboat**. Settings, project records, sidebar preferences, installation progress, boundary checks, the shared ChatGPT record, the closed `usage.sqlite3`, retained JSON ledgers, and immutable backups move together. The old files are removed only after copying, verification, and committing the new location.

`Application Support/Tugboat/storage-location.json` remembers the selected folder and any pending move; a separate lock file prevents concurrent instances from changing the same records. When a custom folder is active, other Tugboat records reside there. **Use default folder** schedules a move back. Missing drives never create an empty replacement dataset. Connect the drive and restart, or use **Locate records folder…** to find the same records at a new path. Failed and interrupted moves retain the original records and retry on a later launch. Partial copies cannot be selected as active records.

Earlier directory markers are upgraded using the saved location transaction. The committed active folder remains available without changing project or settings files. Unselected older copies still require proof of completion before they can be used as active records.

The location pointer and directory marker remain outside SQLite. Settings, the project registry, profiles, installation records, and authentication storage keep their existing formats. Native transcripts and guest homes stay in their agent-owned locations. A records move stops writers, saves final usage, and closes the database before copying it. Source and target database hashes, dataset IDs, integrity, and foreign keys must match before the location pointer changes. SQLite recovers a hot rollback journal before copying; the app never deletes that journal by hand. Failed final scans, synchronization, or close operations block restart and preserve the pending move. A verified Locate operation can reconnect the same dataset at a new path and save final usage there before restart.

If an activated database is missing, damaged, replaced, or bound to another dataset, usage writes and records moves stop. The app does not create an empty replacement or reimport retained JSON. Keep the entire records folder, including `usage.sqlite3-journal` if present. Reconnect the original folder through Settings, or restore a verified matching SQLite snapshot offline with a SQLite-capable build. Never copy an open database.

After an I/O or uncertain commit error, the next usage access opens a new connection to the same verified database. Identity, integrity, and committed parser progress must verify before an idempotent scan can continue. Detected corruption blocks later writes through the existing shared store.

Before activation, the unchanged JSON files are the rollback point. After activation, SQL state can be newer than every retained ledger. Do not downgrade the directory marker or run a legacy writer against those ledgers. Repair the SQLite store or restore a verified matching snapshot. A lossless downgrade requires a separately verified reverse export; this migration does not provide one.

Brig manages guest homes and conversation history in its existing locations. Closing or restarting the app while a process runs asks for confirmation. Cancel keeps the app and process open. Confirming ends open terminal processes and saves final usage before a restart applies a pending records move.

Git commands, tests, diagnostics, policy checks, boundary checks, and **Stop Brig environment** target the selected environment. Stopping one environment preserves the other agent's environment.

**Preview environment removal…** targets the selected environment. **Preview all environments…** targets every agent environment for that project. The preview names the affected sessions, guest homes, bindings, and policies before removal. Type the displayed confirmation name to continue. Project files remain intact. If removal is interrupted, reopen the preview to continue recorded steps. Normal setup stays paused for environments with incomplete removal.

Tugboat checks project folders automatically on startup. An unavailable folder stays in the library. Connect its drive; Tugboat checks again every eight seconds while Projects is visible, and when the app regains focus. Checks pause in Settings and selected project views and stop when all folders are available. The scan preserves project records, Brig environments, saved usage, opening order, and keyboard focus. Tugboat uses the volume UUID when available to recognize a disk after a restart. Older records can recover a changed device number when the folder's inode and known creation time still match. A replaced folder or a different known volume remains unavailable.

**Forget…** removes only the Tugboat record and stops background monitoring; it preserves project files, Brig environments, guest homes and saved usage. Use **Add new project** to reconnect a moved folder instead.

## Development

Start the desktop application in development mode:

```sh
npm ci
npm run tauri -- dev
```

The Tauri command prepares native dependencies before it starts Vite. `npm run dev` starts only the frontend server; native application features require Tauri.

### Checks

Run the frontend tests and production build:

```sh
npm test
npm run build
```

Prepare native dependencies before running Rust and native terminal tests:

```sh
npm run native:prepare
cargo test --manifest-path src-tauri/Cargo.toml --locked
npm run test:terminal
npm run test:hull
```

`npm test` covers frontend flows, Brig response fixtures, and development report calculations. The report tests use the local `reports/imp-plan.html` file and skip when it is absent.

`npm run test:terminal` checks the Ghostty terminal engine and native text composition. `npm run test:hull` checks the patched hull handling of shared folder paths.

Pull requests run frontend tests and the production web build on Ubuntu. Native changes also run Rust and native tests on the pinned Mac toolchain. This includes changes to native fixtures, runtime inputs and build scripts. These checks have read-only GitHub permissions and no publication steps. The trunk workflow retains installer publication.

Pull-request preparation uses `npm run native:prepare -- --no-sign`. This mode skips explicit signing commands and preserves vendor signature verification. Compilers can still add local ad hoc signatures. Default preparation and the Tauri build hooks retain their existing signing behavior.

The separate formatting job checks added Rust lines against `rustfmt`. It leaves existing format differences unchanged. Run `python3 scripts/check-rust-format.py <base-commit>` to check committed changes against a base commit. Optional local report checks and unavailable disk-tool checks give their skip reasons in the test output.

### SQLite usage verification

The SQLite acceptance and performance checks use generated records and native transcripts. They do not launch Tugboat or use account data. Keep the frozen P01 JSON source and its manifest for the comparison build. The local results are linked from `reports/sqlite-usage-migration-plan.html`.

The macOS signature test in `npm test` signs a temporary copy of `/usr/bin/true`. Skip that test when signing is prohibited. Native dependency preparation and ordinary Tauri packaging also have signing steps; do not use them in a no-sign verification run. Set `P06_TEST_BINARY` and `P01_TEST_BINARY` to the recorded current and frozen Rust test executables. Set `P06_CARGO_TARGET` to the recorded temporary Cargo target directory. With verified native dependencies already cached, use:

```sh
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo test --manifest-path src-tauri/Cargo.toml --locked
node --experimental-vm-modules --test --test-skip-pattern='^codesign evaluates the Developer ID requirement against an ad hoc signature$' tests/*/*.test.mjs
npm run build
python3 scripts/check-sqlite-usage-acceptance.py --binary "$P06_TEST_BINARY" --old-binary "$P01_TEST_BINARY" --output reports/sqlite-usage-evidence/p06/append-remediation/acceptance/final
python3 scripts/compare-sqlite-frozen-contracts.py --sql-cache "$P06_CARGO_TARGET" --output reports/sqlite-usage-evidence/p06/append-remediation/acceptance/frozen-build-parity
python3 scripts/benchmark-sqlite-usage.py --warmups 5 --repetitions 100 --fixture-parent /private/tmp --evidence-dir reports/sqlite-usage-evidence/p06/append-remediation/performance
python3 scripts/check-sqlite-usage-volume.py
```

The volume check creates and removes only its own temporary APFS disk image. The performance check compares optimized Rust workers on the same local volume. It records every timed sample, parsing and storage phases, fixture counts, query plans, SQLite settings, and binary hashes. Initial import, saved-Claude preparation, and later opens are separate measurements. Generated subprocesses test interrupted commits and publication, busy writers, SQLite full errors, failed synchronization, and corruption.

The frozen-build check injects its shared export adapter only into disposable source copies. Both builds read the same generated native files, paths, and inodes. It compares complete dashboard, details, event, resume, quiet-poll, and state-transition results for Codex and all three Claude contracts. It fixes only the transport sequence. It also compares typed ledgers, prices, and ordered corrections.

For unsigned native verification, use the recorded P06 build route with `--no-sign --no-binary-patching`, a null signing identity, and the final linker’s `-no_adhoc_codesign` option. Verify that the app has no `_CodeSignature` directory and its main binary has no `LC_CODE_SIGNATURE`. Do not launch the app against active records as part of these checks.


Run the isolated, real Brig CLI environment check after preparing native dependencies:

```sh
node scripts/acceptance/check-agent-environments.mjs
```

This check creates temporary Codex and Claude environments, verifies separate homes and policies, and checks stop and removal scope. It writes `artifacts/claude-code/step-9-cli-environments.json` and records cleanup results. It can download guest images. It does not test the native interface, accounts, model requests, or interactive resume.

Claude integration browser captures use synthetic data at 1280px and 760px in `.impeccable/review/claude-integration/`. They establish frontend layout only. The isolated CLI check passed six checks and cleaned its temporary resources. Tugboat 0.3.0 passed all 46 recorded native acceptance checks. These covered real replies, permission approval and denial, resume after environment restart and sign-in, saved states, migrated and custom Codex profiles, shared accounts, and records and disk recovery. Removal recovery passed after one controlled CLI failure following a real stop. It removed only the approved extra Claude environment, guest home, and owned policy. Both Codex projects and the main Claude chat continued. Successful shared ChatGPT sign-out also preserved a new reply in the same open Claude session.

The original records location was restored exactly. All 18 normal record files remained byte-for-byte unchanged after the latest app reopened. Four test guests were stopped, five unrelated inventory entries remained unchanged, and the test disk was detached. The temporary CLI wrapper was removed. Other test homes, project files, the disk image, and the custom profile remain available for review. The initial Codex setup retry and stock Claude auto-update warning have no established cause. Evidence is in `artifacts/claude-code/step-9-native-session.json`.

### Application icon

The logo source is [src-tauri/icons/brig-desktop.svg](src-tauri/icons/brig-desktop.svg). Rebuild the native macOS icon assets with:

```sh
npm run icon:build
```

This command requires Xcode with Icon Composer. Rebuild the app bundle to include the updated assets.

### Project layout

| Path | Purpose |
| --- | --- |
| [src/](src/) | TypeScript interface. `main.ts` connects the features; `app/` owns startup and application state. |
| [src/projects/](src/projects/), [src/providers/](src/providers/), [src/settings/](src/settings/) | Project selection and operations, provider accounts, and application preferences |
| [src/dashboard/](src/dashboard/), [src/usage/](src/usage/), [src/terminal/](src/terminal/), [src/sidebar/](src/sidebar/) | Dashboard state, usage details, terminal controls, and sidebar behavior. [src/ui/](src/ui/) holds shared interface helpers. |
| [src-tauri/src/](src-tauri/src/) | Rust application. `main.rs` registers commands. `desktop/` owns startup, settings, menus, and shutdown. `platform/` runs and inspects host tools. |
| [projects](src-tauri/src/projects/), [environment](src-tauri/src/environment/), [agents](src-tauri/src/agents/), [agent_sessions](src-tauri/src/agent_sessions/), [providers](src-tauri/src/providers/), [terminal](src-tauri/src/terminal/) | Project records and policy, environment preparation, agent commands and launches, provider accounts, and native terminal sessions |
| [conversations](src-tauri/src/conversations/), [usage](src-tauri/src/usage/), [usage_store](src-tauri/src/usage_store/), [storage](src-tauri/src/storage/) | Conversation history, usage calculation, SQLite persistence and import, and record files and their location |
| `src-tauri/native/` | Native macOS terminal view and input handling |
| `src-tauri/icons/` | Logo sources and application icon assets |
| [scripts/](scripts/) | Node tools are grouped under `runtime/`, `acceptance/`, `licenses/`, `release/`, and `reports/`. |
| `config/` | Dependency versions and hull build configuration |
| [tests/](tests/) | Node suites are grouped by feature. Shared inputs remain in `fixtures/`. Rust unit tests live with their owning modules. |
| `resources/` | Runtime patches, bundled assets, and manually maintained license notices |

[PRODUCT.md](PRODUCT.md) describes the product scope. [DESIGN.md](DESIGN.md) records the interface rules.

## Third-party notices

The build generates `Tugboat.app/Contents/Resources/licenses/THIRD-PARTY-NOTICES.txt`. Its index maps each component and version to its full notice texts. Components with identical texts share one copy.

The adjacent `sources/e2fsprogs/` folder contains the exact source archive, build instructions, build script, and version information for the bundled disk tools. The Brig patch and its source information are in `sources/brig/`.

Tauri generates these files before each bundle. To regenerate them separately, run `npm run native:prepare` and then `npm run licenses:bundle`. Output goes to `.build/licenses/`. Git keeps the [curated notice inputs](resources/license-supplements/) and the build scripts.

The hull modification is recorded in [config/hull-compat.json](config/hull-compat.json). Its patch is in [resources/runtime/hull-spaces.patch](resources/runtime/hull-spaces.patch).

## Technologies

| Technology | Role |
| --- | --- |
| Tauri 2 | Desktop application framework and frontend-to-native commands |
| Rust | Application backend, process control, settings, and Brig integration |
| TypeScript | Frontend behavior and typed application commands |
| Vite | Frontend development server and production build |
| HTML and CSS | Application interface and system appearance |
| Objective-C, AppKit, and Core Text | Native terminal view, text rendering, input, and clipboard integration |
| libghostty-vt | Terminal emulation and terminal state |
| Brig, hull, and hvi/Vz | Guest environments, execution plans, and network policies |
| Node.js and npm | Frontend dependencies, build scripts, and tests |
| Zig and Go | Pinned toolchains for building Ghostty and hull |
| Swift and Icon Composer | Native macOS icon generation |
| jsdom and the Node.js test runner | Frontend and report tests |
