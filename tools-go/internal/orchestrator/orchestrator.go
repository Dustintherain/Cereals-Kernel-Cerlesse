// Package orchestrator 实现 H-04「测试编排服务」：按场景配置并行调度多个
// QEMU 集成测试（tests/test_boot.py / tests/test_keyboard.py），收集每个场景的
// 退出码与串口日志，并用 serialparse 对日志做二次复核（断言缺失 + 心跳/异常分析），
// 最后输出汇总并给出整体成败。
//
// 并行隔离约定（每个场景独立资源，避免互相踩踏）：
//   - OVMF_VARS 副本：build/orch/<name>/OVMF_VARS.fd（--ovmf-vars）
//   - 串口日志：      build/orch/<name>/serial.log（--serial-log）
//   - monitor sock：  build/orch/<name>/monitor.sock（--monitor，仅键盘场景）
//
// 磁盘镜像（disk.img / disk-exception.img / disk-pagefault.img）为只读共享：
// QEMU 以 raw 只读方式启动，多个场景可并发读取同一镜像。
package orchestrator

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"sort"
	"strings"
	"sync"
	"time"

	"cerlesse/tools/internal/serialparse"
)

// Scenario 描述一个 QEMU 集成测试场景。
type Scenario struct {
	Name string `json:"name"`
	// Cmd 是完整命令行（argv[0] 为解释器/可执行文件）。
	Cmd []string `json:"cmd"`
	// SerialLog 是该场景的串口日志输出路径（编排器注入，场景内无需自带）。
	SerialLog string `json:"serialLog"`
	// TimeoutSec 为场景整体超时（默认 120s）。
	TimeoutSec int `json:"timeoutSec,omitempty"`
	// Expect 是必须出现在串口日志中的子串（在脚本自身断言之外的二次复核）。
	Expect []string `json:"expect,omitempty"`
	// AllowPanic 允许日志出现 EXCEPTION/KERNEL PANIC（异常注入场景）。
	AllowPanic bool `json:"allowPanic,omitempty"`
	// SerialOnly 表示只分析已有串口日志、不启动命令（用于离线复核模式）。
	SerialOnly bool `json:"serialOnly,omitempty"`
}

// Result 是单场景运行结果。
type Result struct {
	Name     string
	OK       bool
	Err      error
	ExitCode int
	Duration time.Duration
	Missing  []string // 期望缺失
	Analysis string   // serialparse 汇总
	LogPath  string
}

// Summary 汇总一次编排运行。
type Summary struct {
	Results  []Result
	Passed   int
	Failed   int
	Duration time.Duration
}

// OK 表示全部场景通过。
func (s Summary) OK() bool { return s.Failed == 0 }

// Run 按并发度 limit 并行执行场景；limit<=0 时使用 min(GOMAXPROCS, 4)。
// 每个场景的工作目录为 workDir（仓库根），日志写入 workDir 下的相对路径。
func Run(ctx context.Context, scenarios []Scenario, workDir string, limit int, logw func(format string, args ...any)) Summary {
	if limit <= 0 {
		limit = 4
		if n := maxProcs(); n < limit {
			limit = n
		}
	}
	if limit > len(scenarios) {
		limit = len(scenarios)
	}
	if limit < 1 {
		limit = 1
	}

	start := time.Now()
	results := make([]Result, len(scenarios))
	sem := make(chan struct{}, limit)
	var wg sync.WaitGroup

	for i, sc := range scenarios {
		wg.Add(1)
		go func(i int, sc Scenario) {
			defer wg.Done()
			sem <- struct{}{}
			defer func() { <-sem }()
			results[i] = runOne(ctx, sc, workDir, logw)
		}(i, sc)
	}
	wg.Wait()

	sum := Summary{Results: results, Duration: time.Since(start)}
	for _, r := range results {
		if r.OK {
			sum.Passed++
		} else {
			sum.Failed++
		}
	}
	return sum
}

func runOne(ctx context.Context, sc Scenario, workDir string, logw func(format string, args ...any)) Result {
	res := Result{Name: sc.Name, LogPath: sc.SerialLog}
	timeout := time.Duration(sc.TimeoutSec) * time.Second
	if timeout <= 0 {
		timeout = 120 * time.Second
	}
	cctx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()

	start := time.Now()
	if !sc.SerialOnly {
		if len(sc.Cmd) == 0 {
			res.Err = fmt.Errorf("scenario has empty cmd")
			return res
		}
		cmd := exec.CommandContext(cctx, sc.Cmd[0], sc.Cmd[1:]...)
		cmd.Dir = workDir
		// 场景超时后，直接子进程被杀但孙进程（如脚本拉起的 QEMU）可能仍持有
		// 输出管道；WaitDelay 限定等待时间，避免编排器被挂住。
		cmd.WaitDelay = 1 * time.Second
		out, err := cmd.CombinedOutput()
		res.Duration = time.Since(start)
		if err != nil {
			if ee, ok := err.(*exec.ExitError); ok {
				res.ExitCode = ee.ExitCode()
			} else {
				res.ExitCode = -1
			}
			res.Err = fmt.Errorf("%v\n%s", err, tail(out, 2000))
			// 即使命令失败也继续做日志复核，便于定位是断言缺失还是崩溃。
		}
	}

	// 二次复核：串口日志存在性 + 期望子串 + serialparse 分析。
	if sc.SerialLog == "" {
		if res.Err == nil {
			res.Err = fmt.Errorf("scenario has no serialLog")
		}
		return res
	}
	logPath := sc.SerialLog
	if !filepath.IsAbs(logPath) {
		logPath = filepath.Join(workDir, logPath)
	}
	content, err := os.ReadFile(logPath)
	if err != nil {
		if res.Err == nil {
			res.Err = fmt.Errorf("read serial log: %w", err)
		}
		return res
	}
	missing := serialparse.CheckExpects(content, sc.Expect)
	if len(missing) > 0 {
		res.Missing = missing
		if res.Err == nil {
			res.Err = fmt.Errorf("missing expectations: %s", strings.Join(quoteAll(missing), ", "))
		}
	}
	analyzer, err := serialparse.AnalyzeReader(strings.NewReader(string(content)), sc.AllowPanic)
	if err != nil {
		if res.Err == nil {
			res.Err = fmt.Errorf("analyze serial log: %w", err)
		}
		return res
	}
	res.Analysis = analyzer.Summary()
	if !analyzer.OK() {
		if res.Err == nil {
			res.Err = fmt.Errorf("serial log anomalies: %s", strings.TrimSpace(strings.Join(analyzer.Anomalies, "; ")))
		}
	}

	res.OK = res.Err == nil
	if logw != nil {
		status := "PASS"
		if !res.OK {
			status = "FAIL"
		}
		logw("[%s] %s (%.1fs)", status, sc.Name, res.Duration.Seconds())
		if !res.OK {
			logw("       %v", res.Err)
		}
	}
	return res
}

// Format 将汇总渲染为多行文本。
func Format(sum Summary) string {
	var b strings.Builder
	names := make([]string, 0, len(sum.Results))
	for _, r := range sum.Results {
		names = append(names, r.Name)
	}
	sort.Strings(names)
	b.WriteString("---- orchestrator summary ----\n")
	for _, r := range sum.Results {
		status := "PASS"
		if !r.OK {
			status = "FAIL"
		}
		fmt.Fprintf(&b, "%-6s %-14s %6.1fs  %s\n", status, r.Name, r.Duration.Seconds(), r.LogPath)
		if r.Analysis != "" && !r.OK {
			for _, line := range strings.Split(strings.TrimRight(r.Analysis, "\n"), "\n") {
				fmt.Fprintf(&b, "       | %s\n", line)
			}
		}
	}
	fmt.Fprintf(&b, "total: %d passed, %d failed, %.1fs\n", sum.Passed, sum.Failed, sum.Duration.Seconds())
	return b.String()
}

func tail(b []byte, n int) string {
	if len(b) <= n {
		return strings.TrimRight(string(b), "\n")
	}
	return "..." + string(b[len(b)-n:])
}

func quoteAll(ss []string) []string {
	out := make([]string, len(ss))
	for i, s := range ss {
		out[i] = fmt.Sprintf("%q", s)
	}
	return out
}

func maxProcs() int {
	return runtime.NumCPU()
}
