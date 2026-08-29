# 测试 Fixtures 说明

本目录存放用于测试的 Rust 源代码样例。每个子目录对应一类场景：

| 目录 | 用途 |
|------|------|
| `valid/` | 合法程序，验证编译成功路径 |
| `ownership/` | 触发所有权相关错误（如 E0382） |
| `borrowing/` | 触发借用相关错误（如 E0499 / E0502） |
| `option/` | Option 相关样例（后续补充） |
| `iterator/` | Iterator 相关样例（后续补充） |
| `runtime/` | 运行时 panic 样例（后续补充） |
| `logic/` | 逻辑错误样例（后续补充） |
| `runner/` | 供 Runner / TestRunner 测试使用的可运行程序 |

这些 fixtures 被编译期测试（`tests/`）引用，路径通过
`CARGO_MANIFEST_DIR/tests/fixtures/rust/...` 定位。
