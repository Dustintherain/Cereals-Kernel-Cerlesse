package serialparse

import (
	"strings"
	"testing"
)

func TestClassify(t *testing.T) {
	cases := []struct {
		line string
		want Kind
	}{
		{"IRQ0_heartbeat tick=100", KindHeartbeat},
		{"KB a", KindKeyEcho},
		{"KBIRQ ENTRY", KindOther},
		{"sched: switch pid=1 -> 2 name=task_a", KindSchedSwitch},
		{"EXCEPTION: divide error", KindException},
		{"KERNEL PANIC", KindPanic},
		{"Kernel started!", KindBootMarker},
		{"frame: stress PASS (1024 frames)", KindTestPass},
		{"sched: round-robin wrap FAIL", KindTestFail},
	}
	for _, c := range cases {
		if got := Classify(c.line); got != c.want {
			t.Errorf("Classify(%q) = %v, want %v", c.line, got, c.want)
		}
	}
}

func TestHeartbeatTick(t *testing.T) {
	tick, ok := HeartbeatTick("IRQ0_heartbeat tick=300")
	if !ok || tick != 300 {
		t.Fatalf("HeartbeatTick = (%d, %v), want (300, true)", tick, ok)
	}
	if _, ok := HeartbeatTick("KB a"); ok {
		t.Fatal("HeartbeatTick(KB a) should not parse")
	}
}

func TestAnalyzerHeartbeatRegression(t *testing.T) {
	a := New(false)
	for _, line := range []string{"IRQ0_heartbeat tick=100", "IRQ0_heartbeat tick=200", "IRQ0_heartbeat tick=200"} {
		a.Feed(line)
	}
	if a.OK() {
		t.Fatal("expected anomaly for non-increasing heartbeat tick")
	}
	if a.Heartbeats != 3 || a.FirstTick != 100 || a.LastTick != 200 {
		t.Fatalf("stats wrong: heartbeats=%d first=%d last=%d", a.Heartbeats, a.FirstTick, a.LastTick)
	}
}

func TestAnalyzerAllowPanic(t *testing.T) {
	strict := New(false)
	strict.Feed("EXCEPTION: divide error")
	strict.Feed("KERNEL PANIC")
	if strict.OK() {
		t.Fatal("strict mode should flag panic/exception")
	}

	lenient := New(true)
	lenient.Feed("EXCEPTION: page fault")
	lenient.Feed("KERNEL PANIC")
	if !lenient.OK() {
		t.Fatalf("allowPanic mode should accept injected panic, anomalies=%v", lenient.Anomalies)
	}
}

func TestAnalyzerFailLine(t *testing.T) {
	a := New(true)
	a.Feed("sched: round-robin wrap FAIL")
	if a.OK() || a.Fails != 1 {
		t.Fatalf("FAIL line should be anomaly, fails=%d ok=%v", a.Fails, a.OK())
	}
}

func TestCheckExpects(t *testing.T) {
	content := []byte("Cerlesse kernel v0.4\nKernel started!\ncontext: switch PASS\n")
	missing := CheckExpects(content, []string{"Kernel started!", "context: switch PASS"})
	if len(missing) != 0 {
		t.Fatalf("unexpected missing: %v", missing)
	}
	missing = CheckExpects(content, []string{"KB a"})
	if len(missing) != 1 || missing[0] != "KB a" {
		t.Fatalf("want [KB a], got %v", missing)
	}
}

func TestAnalyzeReader(t *testing.T) {
	log := strings.Join([]string{
		"PIT 100Hz + PIC IRQ0/IRQ1 unmasked; IF enabled",
		"IRQ0_heartbeat tick=100",
		"KB a",
		"sched: switch pid=1 -> 2 name=task_a",
		"frame: stress PASS",
	}, "\n")
	a, err := AnalyzeReader(strings.NewReader(log), false)
	if err != nil {
		t.Fatal(err)
	}
	if a.Lines != 5 || a.Heartbeats != 1 || a.KeyEchoes != 1 || a.SchedSwitches != 1 || a.Passes != 1 {
		t.Fatalf("wrong stats: %+v", a)
	}
	if !a.OK() {
		t.Fatalf("unexpected anomalies: %v", a.Anomalies)
	}
	if !strings.Contains(a.Summary(), "anomalies: none") {
		t.Fatalf("summary wrong: %s", a.Summary())
	}
}
