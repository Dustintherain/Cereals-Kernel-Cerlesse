# tools-go/ — 主机侧 Go 工具

语言分工见 docs/architecture.md 第 2 节：Go 用于高并发主机侧服务，不进入内核。

- 阶段：v0.4 主机侧工具（已落地）
- 最后更新：2026-10-08
- 状态：✅ 已完成（H-04 测试编排器 + H-05 串口日志分析，`make go-test` / `make test-orch` 通过）

> 本文件顶部保留阶段 / 最后更新 / 状态字段，与项目文档维护约定对齐。

| 工具 | 用途 | 版本 | 状态 |
| ---- | ---- | ---- | ---- |
| `cmd/testorch`（H-04） | 并行调度多个 QEMU 测试场景 + 串口日志二次复核 | v0.4 | ✅ 已完成（`cmd/testorch` + `internal/orchestrator`） |
| `cmd/serialmon`（H-05） | 实时收集/分析串口日志（tail -f 式分类 + 心跳异常检测） | v0.4 | ✅ 已完成（`cmd/serialmon` + `internal/serialparse`） |

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

## 验收（v0.4，已通过）

- [x] `make go-test`（`go vet` + `go test`，2026-10-08 通过）
- [x] `make test-orch`（H-04 并行跑 6 个集成测试场景 + 串口日志二次复核，2026-10-08 通过：6 passed / 0 failed）

## 修订记录

| 日期 | 变更 |
| ---- | ---- |
| 2026-10-08 | 创建本文档，记录 H-04（testorch）/H-05（serialmon）仍待编写 |
| 2026-10-08 | 本次 docs 迭代统一文档顶部元信息（阶段 / 最后更新 / 状态）与修订记录，保持与 DEVELOPMENT.md 与索引文档描述一致 |
| 2026-10-08 | H-04/H-05 落地并验收：`make go-test`（vet + test）与 `make test-orch`（6 场景并行，6 passed / 0 failed）均通过，表格与验收项改为已完成 |
