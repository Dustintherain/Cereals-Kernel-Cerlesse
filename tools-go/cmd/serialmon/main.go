// serialmon 是 H-05「串口日志实时收集与分析」的 CLI：
// 以类似 tail -f 的方式跟随串口日志文件（或用 -once 分析快照），逐行分类并
// 实时打印带分类前缀的行，同时累积统计；文件结束/截断或收到中断信号时输出汇总。
//
// 用法：
//
//	go run ./cmd/serialmon -file build/orch/boot/serial.log        # 跟随（tail -f）
//	go run ./cmd/serialmon -file build/orch/boot/serial.log -once  # 快照分析后退出
//
// 标志：
//
//	-file   串口日志文件路径（必填）
//	-once   分析完整个文件后立即输出汇总并退出（默认持续跟随）
//	-allow-panic  允许 EXCEPTION/KERNEL PANIC（异常注入场景）
//	-expect 期望子串（可重复；-once 模式下缺失则退出码 1）
//	-interval 跟随模式的轮询间隔（默认 200ms）
//
// 退出码：0 正常（且无异常/无缺失期望），1 检测到异常或期望缺失，2 参数/IO 错误。
package main

import (
	"flag"
	"fmt"
	"io"
	"os"
	"os/signal"
	"strings"
	"syscall"
	"time"

	"cerlesse/tools/internal/serialparse"
)

func main() {
	file := flag.String("file", "", "serial log file to follow")
	once := flag.Bool("once", false, "analyze whole file then exit")
	allowPanic := flag.Bool("allow-panic", false, "allow EXCEPTION/KERNEL PANIC lines")
	interval := flag.Duration("interval", 200*time.Millisecond, "poll interval in follow mode")
	var expects expectFlags
	flag.Var(&expects, "expect", "expected substring (repeatable, checked against collected content)")
	flag.Parse()

	if *file == "" {
		fmt.Fprintln(os.Stderr, "serialmon: -file is required")
		flag.Usage()
		os.Exit(2)
	}

	analyzer := serialparse.New(*allowPanic)
	printer := func(line string) {
		if kind := serialparse.Classify(line); kind != serialparse.KindOther {
			fmt.Printf("[%s] %s\n", kind, line)
			return
		}
		fmt.Println(line)
	}

	var collected []byte
	var err error
	if *once {
		collected, err = readAll(*file, analyzer, printer)
	} else {
		collected, err = follow(*file, *interval, analyzer, printer)
	}
	if err != nil {
		fmt.Fprintf(os.Stderr, "serialmon: %v\n", err)
		os.Exit(2)
	}

	fmt.Println("---- serialmon summary ----")
	fmt.Print(analyzer.Summary())

	exit := 0
	if !analyzer.OK() {
		exit = 1
	}
	if len(expects) > 0 {
		missing := serialparse.CheckExpects(collected, expects)
		if len(missing) > 0 {
			fmt.Printf("missing expectations: %s\n", strings.Join(missing, ", "))
			exit = 1
		}
	}
	os.Exit(exit)
}

// readAll 一次性读取整个文件。
func readAll(path string, a *serialparse.Analyzer, fn func(string)) ([]byte, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	for _, line := range splitLines(string(data)) {
		fn(line)
		a.Feed(line)
	}
	return data, nil
}

// follow 以轮询方式跟随不断增长的文件，对每个完整行调用 fn；
// 收到 SIGINT/SIGTERM 时返回已收集内容。行缓冲保证跨 chunk 的行不会被切断；
// 文件被截断（日志重写）时从头重读。
func follow(path string, interval time.Duration, a *serialparse.Analyzer, fn func(string)) ([]byte, error) {
	sig := make(chan os.Signal, 1)
	signal.Notify(sig, os.Interrupt, syscall.SIGTERM)
	defer signal.Stop(sig)

	f, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	defer f.Close()

	var collected []byte
	pending := make([]byte, 0, 4096) // 未到达换行符的残缺行
	buf := make([]byte, 64*1024)
	for {
		// 读完当前可读的数据（EOF 表示暂无新数据）。
		for {
			n, rerr := f.Read(buf)
			if n > 0 {
				collected = append(collected, buf[:n]...)
				pending = append(pending, buf[:n]...)
				for {
					i := indexByte(pending, '\n')
					if i < 0 {
						break
					}
					clean := strings.TrimRight(string(pending[:i]), "\r")
					pending = pending[i+1:]
					if clean != "" {
						fn(clean)
						a.Feed(clean)
					}
				}
			}
			if rerr != nil {
				break
			}
		}

		// 文件截断检测：大小小于已读位置 → 从头重读。
		if pos, _ := f.Seek(0, io.SeekCurrent); pos >= 0 {
			if st, serr := f.Stat(); serr == nil && st.Size() < pos {
				if _, err := f.Seek(0, io.SeekStart); err != nil {
					return nil, err
				}
				collected = nil
				pending = pending[:0]
				continue
			}
		}

		select {
		case <-sig:
			return collected, nil
		case <-time.After(interval):
		}
	}
}

// indexByte 是 bytes.IndexByte 的小封装，避免额外导入 bytes。
func indexByte(b []byte, c byte) int {
	for i := range b {
		if b[i] == c {
			return i
		}
	}
	return -1
}

// splitLines 将字节流按行拆分（丢弃空行）。
func splitLines(s string) []string {
	var out []string
	for _, line := range strings.Split(s, "\n") {
		line = strings.TrimRight(line, "\r")
		if line != "" {
			out = append(out, line)
		}
	}
	return out
}

// expectFlags 收集可重复的 -expect 标志。
type expectFlags []string

func (e *expectFlags) String() string { return strings.Join(*e, ",") }

func (e *expectFlags) Set(v string) error {
	*e = append(*e, v)
	return nil
}
