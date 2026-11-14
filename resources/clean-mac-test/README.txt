Tugboat bundled runtime test

Use an Apple silicon Mac with macOS 15 or later.
Keep the internet connection available for the first guest downloads.

1. Open the supplied Tugboat disk image.
2. Copy Tugboat.app to Applications.
3. Extract this test kit into a writable folder, such as Downloads.
4. Open Run Clean Mac Check.command in Finder.
5. Keep its Terminal window open until it finishes.
6. Return the JSON evidence file and the check log from this kit.

The runner downloads a pinned official Node.js runtime into a private temporary
folder. It checks the archive SHA-256 before execution. It removes that temporary
runtime when the test finishes.

The test checks the bundled tools offline, then boots Codex and Claude Code in
separate temporary environments. It checks their ext4 disks, project file sharing
and persistent files after stop and restart. It uses temporary projects and guest
homes. The first guest boot can take up to 30 minutes per agent.

The test uses command-line version checks for both agents. Account sign-in and
model replies need a separate application check after the command-line test.

The offline preflight checks Homebrew path denial. Guest commands then run with
the bundled tools and macOS system tools on their command path. This lets hvi
apply its own sandbox. The runner detects Homebrew and records its presence.
When Homebrew is absent, it also selects clean Mac mode. The report records
these checks separately.

Evidence files appear under artifacts/bundled-runtime inside this kit. The runner
also creates a check log next to this file. Return these files if any step fails.
The evidence contains local file paths, tool versions and command output.

For an app in a different folder, open Terminal and run the command file with the
absolute app path as its first argument.

Official Node.js source
https://nodejs.org/dist/v26.13.1/node-v26.13.1-darwin-arm64.tar.gz

Official archive checksums
https://nodejs.org/dist/v26.13.1/SHASUMS256.txt
