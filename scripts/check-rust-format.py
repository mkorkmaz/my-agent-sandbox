#!/usr/bin/env python3
"""Require rustfmt on added Rust lines without changing existing format debt."""

import argparse
import difflib
import re
import subprocess
import sys


def git(*arguments):
    return subprocess.check_output(["git", *arguments], text=True, timeout=30)


def changedFiles(base, head):
    entries = iter(git("diff", "--find-renames", "--name-status", "-z", "--diff-filter=AMR", base, head, "--", "src-tauri").split("\0"))
    for status in entries:
        if not status:
            break
        path = next(entries)
        if status.startswith("R"):
            yield path, next(entries)
        else:
            yield None if status == "A" else path, path


def changedLines(original, source):
    comparison = difflib.SequenceMatcher(None, original.splitlines(), source.splitlines(), autojunk=False)
    return {line for action, _, _, first, last in comparison.get_opcodes()
            if action != "equal" for line in range(first + 1, last + 1)}


def formatDifferences(source, formatted):
    comparison = difflib.SequenceMatcher(None, source.splitlines(keepends=True), formatted.splitlines(keepends=True), autojunk=False)
    for action, first, last, _, _ in comparison.get_opcodes():
        if action != "equal":
            yield from range(max(1, first + 1), max(first + 2, last + 1))


def check(base, head):
    manifest = git("show", f"{head}:src-tauri/Cargo.toml")
    edition = re.search(r'^edition\s*=\s*"(\d+)"', manifest, re.MULTILINE).group(1)
    failures = []
    checked = 0
    for previous, file in changedFiles(base, head):
        if not file.endswith(".rs"):
            continue
        source = git("show", f"{head}:{file}")
        original = git("show", f"{base}:{previous}") if previous and previous.endswith(".rs") else ""
        additions = changedLines(original, source)
        if not additions:
            continue
        formatted = subprocess.run(
            ["rustfmt", "--edition", edition, "--emit", "stdout", "--config", "skip_children=true"],
            input=source, text=True, capture_output=True, timeout=30, check=True,
        ).stdout
        checked += 1
        for line in sorted(additions.intersection(formatDifferences(source, formatted))):
            failures.append(f"{file}:{line}: added line needs rustfmt")
    if failures:
        print("\n".join(failures), file=sys.stderr)
        return 1
    print(f"Rust formatting: checked {checked} changed files; existing format differences are separate.")
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("base", help="Base commit for the pull request")
    parser.add_argument("head", nargs="?", default="HEAD")
    arguments = parser.parse_args()
    try:
        base = git("rev-parse", "--verify", "--end-of-options", f"{arguments.base}^{{commit}}").strip()
        head = git("rev-parse", "--verify", "--end-of-options", f"{arguments.head}^{{commit}}").strip()
        return check(base, head)
    except subprocess.CalledProcessError as error:
        print(error.stderr or str(error), file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
