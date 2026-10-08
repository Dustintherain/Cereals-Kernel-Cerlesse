// testorch 是 H-04「测试编排服务」的 CLI：读取场景配置，按并发度并行调度
// QEMU 集成测试，逐场景输出结果，最后打印汇总；任一场景失败则退出码非 0。
//
// 用法：
//
//	go run ./cmd/testorch -config ../tests/orchestrate.json -parallel 2
//
// 标志：
//
//	-config    场景配置 JSON（必填）
//	-parallel  并发度（覆盖配置中的 concurrency，默认 0=自动）
//	-root      仓库根目录（命令的工作目录，默认取配置文件所在目录的上一级）
package main

import (
	"context"
	"flag"
	"fmt"
	"os"
	"os/signal"
	"path/filepath"

	"cerlesse/tools/internal/orchestrator"
)

func main() {
	configPath := flag.String("config", "", "scenario config JSON path")
	parallel := flag.Int("parallel", 0, "override concurrency (0 = config/auto)")
	root := flag.String("root", "", "repository root (default: parent dir of config)")
	flag.Parse()

	if *configPath == "" {
		fmt.Fprintln(os.Stderr, "testorch: -config is required")
		flag.Usage()
		os.Exit(2)
	}
	cfg, err := orchestrator.LoadConfig(*configPath)
	if err != nil {
		fmt.Fprintf(os.Stderr, "testorch: %v\n", err)
		os.Exit(2)
	}

	workDir := *root
	if workDir == "" {
		workDir = filepath.Dir(*configPath)
		workDir = filepath.Dir(workDir) // tests/ → 仓库根
	}
	if abs, err := filepath.Abs(workDir); err == nil {
		workDir = abs
	}

	scenarios := cfg.Scenarios
	if nameList := flag.Arg(0); nameList != "" {
		filtered := filterByName(scenarios, nameList)
		if len(filtered) == 0 {
			fmt.Fprintf(os.Stderr, "testorch: no scenarios match %q\n", nameList)
			os.Exit(2)
		}
		scenarios = filtered
	}

	limit := *parallel
	if limit <= 0 {
		limit = cfg.Concurrency
	}

	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt)
	defer stop()

	fmt.Printf("testorch: %d scenario(s), root=%s\n", len(scenarios), workDir)
	sum := orchestrator.Run(ctx, scenarios, workDir, limit, func(format string, args ...any) {
		fmt.Printf(format+"\n", args...)
	})
	fmt.Print(orchestrator.Format(sum))
	if !sum.OK() {
		os.Exit(1)
	}
}

// filterByName 按逗号分隔的名字列表过滤场景。
func filterByName(scenarios []orchestrator.Scenario, nameList string) []orchestrator.Scenario {
	want := map[string]bool{}
	for _, n := range splitComma(nameList) {
		want[n] = true
	}
	var out []orchestrator.Scenario
	for _, sc := range scenarios {
		if want[sc.Name] {
			out = append(out, sc)
		}
	}
	return out
}

func splitComma(s string) []string {
	var out []string
	start := 0
	for i := 0; i <= len(s); i++ {
		if i == len(s) || s[i] == ',' {
			if i > start {
				out = append(out, s[start:i])
			}
			start = i + 1
		}
	}
	return out
}
