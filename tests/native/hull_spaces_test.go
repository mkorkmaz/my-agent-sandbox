//go:build darwin

package main

import (
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"testing"
)

func TestDesktopSharePathsWithSpaces(t *testing.T) {
	host := filepath.Join(t.TempDir(), "parent folder", "sample project")
	if err := os.MkdirAll(host, 0700); err != nil {
		t.Fatal(err)
	}
	guest := "/work/sample project"
	for _, backend := range []string{"hvi", "vz"} {
		for _, mode := range []string{"rw", "ro"} {
			shares, files, err := parseShares([]string{host + ":" + guest + ":" + mode}, nil, backend)
			closeShareFiles(files)
			if err != nil || len(shares) != 1 || shares[0].host != host || shares[0].guest != guest || shares[0].readOnly != (mode == "ro") {
				t.Fatalf("path share (%s/%s): %+v, %v", backend, mode, shares, err)
			}
			fd := openDirFd(t, host)
			shares, files, err = parseShares(nil, []string{strconv.Itoa(fd) + ":" + guest + ":" + mode}, backend)
			_ = syscall.Close(fd)
			closeShareFiles(files)
			if err != nil || len(shares) != 1 || shares[0].guest != guest || shares[0].readOnly != (mode == "ro") {
				t.Fatalf("descriptor share (%s/%s): %+v, %v", backend, mode, shares, err)
			}
		}
	}
}

func TestDesktopShareMountKeepsThePathLiteral(t *testing.T) {
	// Capture the actual shell argv. Spaces, globbing and command substitution
	// must remain part of one path rather than become another argument or code.
	guest := "/work/sample project;$(echo UNEXPECTED)*"
	for _, virtiofs := range []bool{true, false} {
		share := hostShare{guest: guest, tag: "share0", readOnly: true}
		script := "mkdir() { printf 'mkdir'; printf '<%s>' \"$@\"; printf '\\n'; }; mount() { printf 'mount'; printf '<%s>' \"$@\"; printf '\\n'; };\n" + shareMountCommand(share, virtiofs)
		output, err := exec.Command("/bin/sh", "-c", script).CombinedOutput()
		if err != nil {
			t.Fatalf("mount script: %s: %v", output, err)
		}
		text := string(output)
		if !strings.HasPrefix(text, "mkdir<-p><"+guest+">\n") || !strings.Contains(text, "><"+guest+">\n") || strings.Count(text, guest) != 2 {
			t.Fatalf("path was split or evaluated: %q", text)
		}
		if !strings.Contains(text, "<-o><") {
			t.Fatalf("read-only mount lost its options: %q", text)
		}
	}
}

func TestDesktopGuestPathRejectsRecordSeparators(t *testing.T) {
	for _, path := range []string{"/work/sample\tproject", "/work/sample\nproject", "/work/sample\rproject", "/work/sample\x00project", "/work/sample'project", "/work/sample\"project", "relative"} {
		if validateGuestPath(path) == nil {
			t.Errorf("accepted an unsafe guest path: %q", path)
		}
	}
}
