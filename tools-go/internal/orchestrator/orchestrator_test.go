package orchestrator

import (
	"context"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"
)

// echoCmd 返回一个跨平台可用的“打印并成功退出”命令。
func echoCmd(text string) []string {
	if runtime.GOOS == "windows" {
		return []string{"cmd", "/c", "echo " + text}
	}
	return []string{"sh", "-c", "echo " + text}
}

func failCmd() []string {
	if runtime.GOOS == "windows" {
		return []string{"cmd", "/c", "exit 1"}
	}
	return []string{"sh", "-c", "exit 1"}
}

func writeLog(t *testing.T, dir, name, content string) string {
	t.Helper()
	path := filepath.Join(dir, name)
	if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
		t.Fatal(err)
	}
	return path
}

func TestRunPassAndFail(t *testing.T) {
	dir := t.TempDir()
	goodLog := writeLog(t, dir, "good.log", "Kernel started!\nIRQ0_heartbeat tick=100\n")
	badLog := writeLog(t, dir, "bad.log", "nothing here\n")

	scenarios := []Scenario{
		{
			Name:       "good",
			Cmd:        echoCmd("hello"),
			SerialLog:  goodLog,
			Expect:     []string{"Kernel started!"},
			TimeoutSec: 20,
		},
		{
			Name:       "missing-expect",
			Cmd:        echoCmd("hello"),
			SerialLog:  badLog,
			Expect:     []string{"Kernel started!"},
			TimeoutSec: 20,
		},
	}

	sum := Run(context.Background(), scenarios, dir, 2, nil)
	if sum.Passed != 1 || sum.Failed != 1 {
		t.Fatalf("passed=%d failed=%d, want 1/1\n%s", sum.Passed, sum.Failed, Format(sum))
	}
	if sum.OK() {
		t.Fatal("summary should not be OK")
	}
	if sum.Results[0].Analysis == "" {
		t.Fatal("passing scenario should carry analysis summary")
	}
	if len(sum.Results[1].Missing) != 1 {
		t.Fatalf("missing expect not recorded: %+v", sum.Results[1])
	}
}

func TestRunCommandFailure(t *testing.T) {
	dir := t.TempDir()
	logPath := writeLog(t, dir, "fail.log", "Kernel started!\n")
	scenarios := []Scenario{
		{Name: "exit1", Cmd: failCmd(), SerialLog: logPath, Expect: []string{"Kernel started!"}, TimeoutSec: 20},
	}
	sum := Run(context.Background(), scenarios, dir, 1, nil)
	if sum.Failed != 1 {
		t.Fatalf("command failure should fail scenario: %s", Format(sum))
	}
	if sum.Results[0].Err == nil {
		t.Fatal("expected error recorded")
	}
}

func TestRunSerialOnly(t *testing.T) {
	dir := t.TempDir()
	logPath := writeLog(t, dir, "panic.log", "EXCEPTION: divide error\nKERNEL PANIC\n")
	scenarios := []Scenario{
		{Name: "injected", Cmd: nil, SerialLog: logPath, AllowPanic: true, SerialOnly: true, TimeoutSec: 5},
		{Name: "strict", Cmd: nil, SerialLog: logPath, AllowPanic: false, SerialOnly: true, TimeoutSec: 5},
	}
	sum := Run(context.Background(), scenarios, dir, 2, nil)
	if !sum.Results[0].OK {
		t.Fatalf("allowPanic scenario should pass: %v", sum.Results[0].Err)
	}
	if sum.Results[1].OK {
		t.Fatal("strict scenario should flag injected panic")
	}
}

func TestRunTimeout(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("sleep scenario uses POSIX sleep")
	}
	dir := t.TempDir()
	logPath := writeLog(t, dir, "slow.log", "Kernel started!\n")
	scenarios := []Scenario{
		{Name: "slow", Cmd: []string{"sh", "-c", "sleep 5"}, SerialLog: logPath, TimeoutSec: 1},
	}
	start := time.Now()
	sum := Run(context.Background(), scenarios, dir, 1, nil)
	if sum.Failed != 1 {
		t.Fatalf("timeout should fail scenario: %s", Format(sum))
	}
	if elapsed := time.Since(start); elapsed > 4*time.Second {
		t.Fatalf("timeout not enforced, took %v", elapsed)
	}
}

func TestFormatContainsSummaryLine(t *testing.T) {
	dir := t.TempDir()
	logPath := writeLog(t, dir, "s.log", "Kernel started!\n")
	scenarios := []Scenario{{Name: "s", Cmd: echoCmd("x"), SerialLog: logPath, Expect: []string{"Kernel started!"}, TimeoutSec: 20}}
	sum := Run(context.Background(), scenarios, dir, 1, nil)
	out := Format(sum)
	if !strings.Contains(out, "total: 1 passed, 0 failed") {
		t.Fatalf("format missing total line:\n%s", out)
	}
}
