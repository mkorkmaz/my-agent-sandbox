//go:build darwin

package main

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// Existing hull integration tests use these paths to decide whether disk tools
// are available. The desktop build resolves them beside its executable.
var mke2fsBin = desktopDiskToolFixture("mke2fs")
var debugfsBin = desktopDiskToolFixture("debugfs")

func desktopDiskToolFixture(name string) string {
	executable, err := os.Executable()
	if err != nil {
		panic(err)
	}
	return filepath.Join(filepath.Dir(executable), name)
}

func TestDesktopDiskToolsFollowHullPathWithSpaces(t *testing.T) {
	directory := filepath.Join(t.TempDir(), "Tugboat Test.app", "Contents", "MacOS")
	if err := os.MkdirAll(directory, 0700); err != nil {
		t.Fatal(err)
	}
	for _, name := range []string{"mke2fs", "debugfs", "e2fsck"} {
		path := filepath.Join(directory, name)
		if err := os.WriteFile(path, []byte("#!/bin/sh\n"), 0700); err != nil {
			t.Fatal(err)
		}
		found, err := diskToolPath(filepath.Join(directory, "hull"), name)
		if err != nil || found != path {
			t.Fatalf("disk tool %s: %s, %v", name, found, err)
		}
	}
}

func TestDesktopDiskToolMissingFailsWithToolName(t *testing.T) {
	directory := t.TempDir()
	for _, name := range []string{"mke2fs", "debugfs", "e2fsck"} {
		_, err := diskToolPath(filepath.Join(directory, "hull"), name)
		if err == nil || !strings.Contains(err.Error(), name) {
			t.Fatalf("missing %s did not return an explicit error: %v", name, err)
		}
	}
	path := filepath.Join(directory, "mke2fs")
	if err := os.WriteFile(path, []byte("#!/bin/sh\n"), 0600); err != nil {
		t.Fatal(err)
	}
	if _, err := diskToolPath(filepath.Join(directory, "hull"), "mke2fs"); err == nil {
		t.Fatal("non-executable disk tool was accepted")
	}
}
