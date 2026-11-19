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
	"time"

	"github.com/brig-sh/hull/pkg/store"
)

func desktopPackagedMonitor(t *testing.T) string {
	t.Helper()
	monitor := filepath.Join(t.TempDir(), "Tugboat Dev.app", "Contents", "MacOS", "hvi")
	if err := os.MkdirAll(filepath.Dir(monitor), 0700); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink("/bin/sleep", monitor); err != nil {
		t.Fatal(err)
	}
	return monitor
}

func TestDesktopPackagedMonitorKeepsItsInstanceRunning(t *testing.T) {
	monitor := desktopPackagedMonitor(t)
	pid, _ := startProcess(t, monitor, "30")
	instance := storeWithInstance(t, "desktop-fixture", &store.InstanceState{
		ID:        "desktop-fixture",
		PID:       pid,
		Status:    "running",
		StartTime: time.Now(),
		CmdLine:   []string{monitor, "30"},
	})
	if row := strings.Fields(psRow(t, instance, "desktop-fixture")); len(row) < 2 || row[1] != "running" {
		t.Fatalf("live monitor did not have running status: %v", row)
	}
	state, err := instance.GetInstance("desktop-fixture")
	if err != nil {
		t.Fatal(err)
	}
	if state.Status != "running" || state.PID != pid {
		t.Fatalf("live monitor record changed: status=%q pid=%d", state.Status, state.PID)
	}
}

func TestDesktopMonitorChecksRecordedArgumentBoundaries(t *testing.T) {
	command, arguments, _ := stubVMM(t)
	pid := command.Process.Pid
	state := store.InstanceState{
		PID:       pid,
		StartTime: time.Now().Add(-time.Hour),
		CmdLine:   arguments,
	}
	if !processIsAVMM(pid, &state) {
		t.Fatal("monitor with matching arguments was rejected")
	}
	first, remainder, _ := strings.Cut(arguments[2], " ")
	state.CmdLine = []string{arguments[0], arguments[1], first, remainder}
	if processIsAVMM(pid, &state) {
		t.Fatal("monitor with a stale timestamp and different arguments was accepted")
	}
}

func TestDesktopStopRecoversOnlyTheSelectedMonitor(t *testing.T) {
	monitor := desktopPackagedMonitor(t)
	_, orphanExited := startProcess(t, monitor, "30")
	siblingPID, siblingExited := startProcess(t, monitor, "31")
	instance := storeWithInstance(t, "damaged", &store.InstanceState{
		ID:        "damaged",
		Status:    "stopped",
		StartTime: time.Now(),
		ExitedAt:  time.Now(),
		CmdLine:   []string{monitor, "30"},
	})
	if _, err := instance.CreateInstance("sibling"); err != nil {
		t.Fatal(err)
	}
	if err := instance.SaveInstance(&store.InstanceState{
		ID:      "sibling",
		PID:     siblingPID,
		Status:  "running",
		CmdLine: []string{monitor, "31"},
	}); err != nil {
		t.Fatal(err)
	}
	psRow(t, instance, "damaged")
	if !stillRunning(orphanExited) {
		t.Fatal("listing instances stopped an unrecorded monitor")
	}
	if err := stopInstanceIn(instance, "damaged", 1); err != nil {
		t.Fatal(err)
	}
	if stillRunning(orphanExited) || !stillRunning(siblingExited) {
		t.Fatal("stop did not end only the selected monitor")
	}
	if err := stopInstanceIn(instance, "damaged", 1); err != nil {
		t.Fatalf("could not repeat a completed stop: %v", err)
	}
}

func TestDesktopStopReportsAmbiguousMonitor(t *testing.T) {
	monitor := desktopPackagedMonitor(t)
	_, firstExited := startProcess(t, monitor, "30")
	_, secondExited := startProcess(t, monitor, "30")
	instance := storeWithInstance(t, "damaged", &store.InstanceState{
		ID:      "damaged",
		Status:  "stopped",
		CmdLine: []string{monitor, "30"},
	})
	if err := stopInstanceIn(instance, "damaged", 1); err == nil || !strings.Contains(err.Error(), "multiple monitor processes") {
		t.Fatalf("expected an ambiguous monitor error, got %v", err)
	}
	if !stillRunning(firstExited) || !stillRunning(secondExited) {
		t.Fatal("an ambiguous monitor was stopped")
	}
}

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
