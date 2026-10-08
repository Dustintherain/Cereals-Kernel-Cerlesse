// Package serialparse 提供 H-05「串口日志实时收集与分析」的核心逻辑：
// 对内核串口输出逐行分类（心跳/按键回显/调度/异常/PANIC/自测结果），
// 跟踪 PIT 心跳 tick 的递增关系，并给出可复用的汇总与异常判定。
//
// 该包被 cmd/serialmon（独立实时分析工具）与 internal/orchestrate
// （H-04 编排器跑完场景后的日志复核）共同使用。
package serialparse

import (
	"bufio"
	"fmt"
	"io"
	"os"
	"regexp"
	"strconv"
	"strings"
)

// Kind 是串口日志行的分类。
type Kind int

const (
	KindOther       Kind = iota
	KindHeartbeat        // IRQ0_heartbeat tick=<n>
	KindKeyEcho          // KB <字符>
	KindSchedSwitch      // sched: switch pid=…
	KindException        // EXCEPTION: …
	KindPanic            // KERNEL PANIC
	KindTestPass         // … PASS
	KindTestFail         // … FAIL
	KindBootMarker       // Kernel started!
)

// String 返回分类的短名称（用于 serialmon 实时前缀）。
func (k Kind) String() string {
	switch k {
	case KindHeartbeat:
		return "hb"
	case KindKeyEcho:
		return "kb"
	case KindSchedSwitch:
		return "sched"
	case KindException:
		return "exception"
	case KindPanic:
		return "panic"
	case KindTestPass:
		return "pass"
	case KindTestFail:
		return "fail"
	case KindBootMarker:
		return "boot"
	default:
		return ""
	}
}

var (
	heartbeatRe = regexp.MustCompile(`IRQ0_heartbeat tick=(\d+)`)
	keyEchoRe   = regexp.MustCompile(`^KB (.*)$`)
	schedRe     = regexp.MustCompile(`^sched: switch\b`)
)

// Classify 对单行串口输出分类。
func Classify(line string) Kind {
	switch {
	case strings.Contains(line, "KERNEL PANIC"):
		return KindPanic
	case strings.Contains(line, "EXCEPTION:"):
		return KindException
	case heartbeatRe.MatchString(line):
		return KindHeartbeat
	case keyEchoRe.MatchString(line):
		return KindKeyEcho
	case schedRe.MatchString(line):
		return KindSchedSwitch
	case strings.Contains(line, "Kernel started!"):
		return KindBootMarker
	case strings.Contains(line, "FAIL"):
		return KindTestFail
	case strings.Contains(line, "PASS"):
		return KindTestPass
	default:
		return KindOther
	}
}

// HeartbeatTick 解析心跳行中的 tick 值；非心跳行返回 ok=false。
func HeartbeatTick(line string) (tick int, ok bool) {
	m := heartbeatRe.FindStringSubmatch(line)
	if m == nil {
		return 0, false
	}
	n, err := strconv.Atoi(m[1])
	if err != nil {
		return 0, false
	}
	return n, true
}

// Analyzer 累积串口日志统计并检测异常。
//
// 异常定义（默认严格模式）：
//   - 出现 KERNEL PANIC / EXCEPTION（allowPanic=false 时）；
//   - 心跳 tick 非递增（停摆或回退）；
//   - 出现自测 FAIL 行。
//
// allowPanic=true 用于异常注入类场景（test-exception / test-pagefault），
// 其中 panic 是预期结果。
type Analyzer struct {
	Lines         int
	Heartbeats    int
	KeyEchoes     int
	SchedSwitches int
	Exceptions    int
	Panics        int
	Passes        int
	Fails         int

	FirstTick int
	LastTick  int

	AllowPanic bool

	Anomalies []string
}

// New 创建分析器。
func New(allowPanic bool) *Analyzer {
	return &Analyzer{AllowPanic: allowPanic}
}

// Feed 消费一行串口输出。
func (a *Analyzer) Feed(line string) {
	a.Lines++
	switch Classify(line) {
	case KindHeartbeat:
		a.Heartbeats++
		if tick, ok := HeartbeatTick(line); ok {
			if a.Heartbeats == 1 {
				a.FirstTick = tick
			} else if tick <= a.LastTick {
				a.Anomalies = append(a.Anomalies,
					fmt.Sprintf("heartbeat tick 回退/停摆: %d -> %d", a.LastTick, tick))
			}
			a.LastTick = tick
		}
	case KindKeyEcho:
		a.KeyEchoes++
	case KindSchedSwitch:
		a.SchedSwitches++
	case KindException:
		a.Exceptions++
		if !a.AllowPanic {
			a.Anomalies = append(a.Anomalies, "出现 EXCEPTION 行")
		}
	case KindPanic:
		a.Panics++
		if !a.AllowPanic {
			a.Anomalies = append(a.Anomalies, "出现 KERNEL PANIC")
		}
	case KindTestPass:
		a.Passes++
	case KindTestFail:
		a.Fails++
		a.Anomalies = append(a.Anomalies, "出现 FAIL 行: "+strings.TrimSpace(line))
	}
}

// OK 表示日志无异常。
func (a *Analyzer) OK() bool { return len(a.Anomalies) == 0 }

// Summary 返回多行汇总文本。
func (a *Analyzer) Summary() string {
	var b strings.Builder
	fmt.Fprintf(&b, "lines=%d heartbeats=%d key=%d sched=%d pass=%d fail=%d panic=%d exception=%d\n",
		a.Lines, a.Heartbeats, a.KeyEchoes, a.SchedSwitches, a.Passes, a.Fails, a.Panics, a.Exceptions)
	if a.Heartbeats > 0 {
		fmt.Fprintf(&b, "ticks: first=%d last=%d\n", a.FirstTick, a.LastTick)
	}
	if len(a.Anomalies) == 0 {
		b.WriteString("anomalies: none\n")
	} else {
		for _, msg := range a.Anomalies {
			fmt.Fprintf(&b, "anomaly: %s\n", msg)
		}
	}
	return b.String()
}

// AnalyzeReader 扫描 reader 直到 EOF 并返回分析结果。
func AnalyzeReader(r io.Reader, allowPanic bool) (*Analyzer, error) {
	a := New(allowPanic)
	sc := bufio.NewScanner(r)
	// 扫描码/转储行可能较长，放宽缓冲上限。
	sc.Buffer(make([]byte, 0, 64*1024), 1024*1024)
	for sc.Scan() {
		a.Feed(sc.Text())
	}
	if err := sc.Err(); err != nil {
		return a, err
	}
	return a, nil
}

// AnalyzeFile 读取整个串口日志文件并分析。
func AnalyzeFile(path string, allowPanic bool) (*Analyzer, error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	defer f.Close()
	return AnalyzeReader(f, allowPanic)
}

// CheckExpects 在原始日志内容中核对所有期望子串，返回缺失列表。
func CheckExpects(content []byte, expects []string) []string {
	var missing []string
	for _, e := range expects {
		if !strings.Contains(string(content), e) {
			missing = append(missing, e)
		}
	}
	return missing
}
