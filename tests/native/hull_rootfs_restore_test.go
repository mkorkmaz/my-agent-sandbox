//go:build darwin

package main

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestDesktopRestoresOnlyTheReviewedUpper(t *testing.T) {
	store, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	const name = "brig-tugboat-project-fixture"
	const marker = "original writable root"
	digest := "sha256:" + strings.Repeat("a", 64)
	instance := filepath.Join(store, "instances", name)
	recovery := filepath.Join(store, "tugboat-image-recovery", name)
	for _, directory := range []string{instance, recovery} {
		if err := os.MkdirAll(directory, 0700); err != nil {
			t.Fatal(err)
		}
	}
	backup := filepath.Join(recovery, "root.ext4")
	if err := os.WriteFile(backup, []byte(marker), 0600); err != nil {
		t.Fatal(err)
	}
	if err := os.Truncate(backup, 16<<30); err != nil {
		t.Fatal(err)
	}
	t.Setenv("HULL_TUGBOAT_RESTORE_IMAGE", digest)
	t.Setenv("HULL_TUGBOAT_RESTORE_INSTANCE", name)
	for _, attempt := range []struct {
		instance string
		digest   string
		overlay  bool
	}{
		{"another-instance", digest, true},
		{name, "sha256:" + strings.Repeat("b", 64), true},
		{name, digest, false},
	} {
		if err := restoreTugboatUpper(instance, attempt.instance, attempt.digest, attempt.overlay); err == nil {
			t.Fatal("accepted a different instance, image, or disk format")
		}
	}
	if err := restoreTugboatUpper(instance, name, digest, true); err != nil {
		t.Fatal(err)
	}
	target := filepath.Join(instance, "rootfs-upper.ext4")
	disk, err := os.Open(target)
	if err != nil {
		t.Fatal(err)
	}
	defer disk.Close()
	data := make([]byte, len(marker))
	if _, err := disk.ReadAt(data, 0); err != nil || string(data) != marker {
		t.Fatalf("restored block contents = %q, error = %v", data, err)
	}
	if err := restoreTugboatUpper(instance, name, digest, true); err == nil {
		t.Fatal("replaced an existing upper disk")
	}
	if err := os.Remove(target); err != nil {
		t.Fatal(err)
	}
	original := filepath.Join(recovery, "original.ext4")
	if err := os.Rename(backup, original); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(original, backup); err != nil {
		t.Fatal(err)
	}
	if err := restoreTugboatUpper(instance, name, digest, true); err == nil {
		t.Fatal("restored a symbolic link")
	}
}
