package runtime

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestDesktopOverlayUsesBundledMke2fs(t *testing.T) {
	executable, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	resolved, err := filepath.EvalSymlinks(executable)
	if err != nil {
		t.Fatal(err)
	}
	if actual, expected := bundledMke2fs(), filepath.Join(filepath.Dir(resolved), "mke2fs"); actual != expected {
		t.Fatalf("mke2fs = %q, want %q", actual, expected)
	}
}

func TestDesktopOverlayDoesNotFallBackWhenBundledMke2fsIsMissing(t *testing.T) {
	previous := hullMke2fs
	t.Cleanup(func() { hullMke2fs = previous })
	hullMke2fs = filepath.Join(t.TempDir(), "tool directory", "mke2fs")
	missing := hullOverlayRoot()
	if missing.Available || !strings.Contains(missing.Why, hullMke2fs) {
		t.Fatalf("missing tool accepted: %+v", missing)
	}
	if err := os.MkdirAll(filepath.Dir(hullMke2fs), 0700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(hullMke2fs, []byte("#!/bin/sh\nexit 0\n"), 0700); err != nil {
		t.Fatal(err)
	}
	if result := hullOverlayRoot(); !result.Available {
		t.Fatalf("bundled tool rejected: %+v", result)
	}
}
