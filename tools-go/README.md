# tools-go/ — 主机侧 Go 工具

语言分工见 docs/architecture.md 第 2 节：Go 用于高并发主机侧服务，不进入内核。

| 工具 | 用途 | 版本 | 状态 |
| ---- | ---- | ---- | ---- |
| `cmd/testorch`（H-04） | 并行调度多个 QEMU 测试场景 + 串口日志二次复核 | v0.4 | ✅ 已落地 |
| `cmd/serialmon`（H-05） | 实时收集/分析串口日志（tail -f 式分类 + 心跳异常检测） | v0.4 | ✅ 已落地 |

模块：`cerlesse/tools`（仅标准库，无外部依赖）。

## 用法

```bash
# 宿主侧单元测试（等价 make go-test）
cd tools-go && go vet ./... && go test ./...

# H-04：并行跑全部 6 个集成测试场景（等价 make test-orch）
# 场景定义见 tests/orchestrate.json；每场景独占 OVMF_VARS/串口日志/monitor socket，
# 磁盘镜像以 QEMU snapshot=on 共享，互不抢锁、不污染原镜像。
go run ./cmd/testorch -config ../tests/orchestrate.json -root ..
go run ./cmd/testorch -config ../tests/orchestrate.json -root .. boot,memory  # 只跑部分场景

# H-05：跟随串口日志（Ctrl-C 退出时打印汇总）
go run ./cmd/serialmon -file ../build/orch/keyboard/serial.log
# 快照分析 + 期望核对（退出码 0=通过 1=异常/缺失 2=用法或 IO 错误）
go run ./cmd/serialmon -file ../build/orch/keyboard/serial.log -once \
    -expect "KB a" -expect "IRQ0_heartbeat tick="
# 异常注入场景允许 PANIC/EXCEPTION
go run ./cmd/serialmon -file ../build/orch/exception/serial.log -once -allow-panic
```

## 包结构

- `internal/serialparse`：串口行分类（心跳/按键/调度/异常/PANIC/自测）、
  心跳 tick 递增检测、期望子串核对（H-05 核心，也用于 testorch 的日志复核）。
- `internal/orchestrator`：场景配置加载校验 + 并发执行（信号量并发、场景超时、
  WaitDelay 防孙进程挂住）、结果汇总。
