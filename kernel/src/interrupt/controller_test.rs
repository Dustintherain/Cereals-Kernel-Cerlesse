//! 中断控制器单元断言（主机侧可测试版本）
//!
//! kernel crate 本身是 `no_std` + 自定义 `panic_handler` 的二进制目标，
//! 因此不能直接在它上面运行 `cargo test`（会拉入 std 并触发 E0152）。
//! 本文件提供一个等价的宿主侧断言入口，后续可直接用 `cargo test -p kernel-tests`
//! 或整合进 tools-go/ 的回归脚本中复用相同的断言语义。
//!
//! TODO(v0.4): 这一文件当前仍作为文档性占位；若要真正跑通宿主侧单元测试，
//! 需要新增一个可 std + 可测试的 crate（例如 kernel-tests），并在其中依赖 kernel
//! 的公开中断接口（若 kernel 把 controller 暴露为公开 API）。
//!
//! 当前更实际的做法是：先保证 kernel 的 `cargo check --workspace` 干净，
//! 再通过 QEMU 集成测试（串口断言）覆盖中断路径——这与 DEVELOPMENT.md 的测试方案一致。

// 占位：暂不声明实际测试函数，避免引入不需要的依赖/构建目标。
// 后续决定宿主侧测试方案后，再在此填充等价断言。
