# PADA — 编程作业诊断 Agent

一个使用 **Rust** 实现的**编程作业诊断 Agent**。它不以「直接把代码修好」为目标，而是像一位导师那样，通过编译、测试、错误分析、知识点映射和分层提示，帮助用户理解：

> **代码为什么错，以及自己没有掌握什么知识。**

## 当前状态

V1 已完成，支持 Rust 编程作业诊断。核心能力：

- 读取题目与 Rust 代码
- 调用 `rustc` / `cargo check` 编译
- 解析 rustc 错误信息（错误码 / 位置 / 附注）
- 错误分类与 Rust 知识点映射
- 运行测试用例并判定通过 / 失败
- 五级分层提示（错误类别 → 错误位置 → 知识点 → 修改方向 → 参考方案）
- 诊断报告输出（控制台文本 + Markdown 导出）
- LLM 接入（OpenAI 兼容，支持 DeepSeek / 本地 Ollama 等）
- 自动生成边界测试用例
- 模型配置与 profile 切换（R3）
- 进度渲染与协作式取消（R4）
- 会话历史与轨迹持久化（R5）
- Token 用量统计与预算控制（R6）

## 安装

```bash
git clone <repo-url>
cd PADA
cargo build --release
```

编译产物位于 `target/release/pada`。

## 快速开始

### 1. 基本诊断

准备一个题目文件（Markdown）和一份学生代码：

```bash
cargo run -- diagnose --problem problem.md --code main.rs
```

输出示例：

```
[编译错误] main.rs:6:20  E0382 (borrow of moved value: `s`)
  知识点 : 所有权 / Move
  提示   : 这是一个编译错误
```

### 2. 指定提示等级

提示分为 5 个等级，默认从 Level 1 开始：

```bash
cargo run -- diagnose --problem problem.md --code main.rs --hint 3
```

| 等级 | 名称 | 内容 |
|------|------|------|
| 1 | 错误类别 | 告诉用户这是什么类型的错误 |
| 2 | 错误位置 | 指出错误的文件、行、列 |
| 3 | 相关知识点 | 映射到具体的 Rust 知识点 |
| 4 | 修改方向 | 给出通用的修改建议 |
| 5 | 参考方案 | 参考代码（需 LLM 生成） |

### 3. 导出 Markdown 报告

```bash
cargo run -- diagnose --problem problem.md --code main.rs --report report.md
```

### 4. 设置 Token 预算

防止 LLM 调用超支：

```bash
cargo run -- diagnose --problem problem.md --code main.rs --budget 20000
```

## CLI 参数

```
pada diagnose [OPTIONS] --problem <PROBLEM>

Options:
      --problem <PROBLEM>  题目描述文件（Markdown）
      --code <CODE>        单文件学生代码
      --project <PROJECT>  Cargo 多文件项目目录
      --hint <HINT>        初始提示等级 1-5，默认 1 [default: 1]
      --profile <PROFILE>  模型 profile 名称（R3）
      --budget <BUDGET>    本次会话 token 预算（R6）
      --report <REPORT>    导出 Markdown 诊断报告路径
      --history <HISTORY>  加载历史会话上下文（R5）
      --save <SAVE>        保存当前会话到指定路径（R5）
      --config <CONFIG>    配置文件路径
  -h, --help               Print help
```

| 参数 | 说明 |
|------|------|
| `--problem <file>` | 题目描述（Markdown） |
| `--code <file>` | 单文件学生代码 |
| `--project <dir>` | Cargo 多文件项目目录 |
| `--hint <level>` | 初始提示等级 1–5，默认 1 |
| `--profile <name>` | 使用指定模型 profile（R3） |
| `--budget <n>` | 本次会话 token 预算（R6） |
| `--history <file>` | 加载历史会话上下文（R5） |
| `--save <file>` | 保存当前会话到文件（R5） |
| `--report <file>` | 导出 Markdown 诊断报告 |
| `--config <file>` | 指定配置文件路径 |

## 使用示例

### 诊断编译错误

```bash
cargo run -- diagnose \
  --problem problem.md \
  --code main.rs \
  --hint 3 \
  --report report.md \
  --save session.json
```

输出：

```
[编译错误] main.rs:6:20  E0382 (borrow of moved value: `s`)
  知识点 : 所有权 / Move
  提示   : 知识点：所有权 / Move

诊断报告已导出: report.md
会话已保存: session.json
```

### 诊断 Cargo 项目

```bash
cargo run -- diagnose --problem problem.md --project ./my_project
```

### 加载历史会话继续诊断

```bash
cargo run -- diagnose \
  --problem problem.md \
  --code main.rs \
  --history session_01.json
```

输出：

```
已加载历史会话: [session_xxx] 所有权练习 (3 步, 0 条用量记录)
[编译错误] main.rs:6:14  E0499 (cannot borrow `s` as mutable more than once at a time)
  知识点 : 借用 / Borrow
  提示   : 位置：main.rs:6:14
```

## 模型配置（R3）

PADA 支持通过 TOML 配置文件管理多组模型 profile，可在云端模型与本地模型之间切换。

### 配置文件格式

```toml
active_profile = "deepseek"

[profiles.local]
endpoint = "http://localhost:11434/v1/chat/completions"
api_key = ""
model_name = "qwen2.5-coder"
context_length = 32768
reasoning = false
input_price = 0.0
output_price = 0.0

[profiles.deepseek]
endpoint = "https://api.deepseek.com/v1/chat/completions"
api_key = "sk-xxx"
model_name = "deepseek-chat"
context_length = 64000
reasoning = false
input_price = 1.0
output_price = 2.0
```

### 使用配置

```bash
cargo run -- diagnose --problem problem.md --code main.rs --config config.toml --profile deepseek
```

### 配置项说明

| 字段 | 说明 |
|------|------|
| `active_profile` | 当前激活的 profile 名称 |
| `endpoint` | API endpoint URL（OpenAI 兼容） |
| `api_key` | API Key（本地模型留空） |
| `model_name` | 模型名称 |
| `context_length` | 上下文长度（token 数） |
| `reasoning` | 是否启用推理链 |
| `input_price` | 输入 token 单价（每百万 token） |
| `output_price` | 输出 token 单价（每百万 token） |

## 诊断报告格式

### 控制台文本

遵循 DESIGN.md §4.1 格式：

```
[编译错误] main.rs:7:5  E0382 (borrow of moved value)
  知识点 : 所有权 / Move
  提示   : 知识点：所有权 / Move

[测试失败] test_case_03
  期望输出 : 3 2 1
  实际输出 : 1 2 3
  知识点   : 待分析
  提示     : 这是一个逻辑错误（测试未通过）
```

### Markdown 导出

```markdown
# 诊断报告

## 编译诊断

### 编译错误 `E0382`
- **位置**: `main.rs:6:20`
- **消息**: borrow of moved value: `s`
- **知识点**: 所有权 / Move
- **提示**: 知识点：所有权 / Move
```

## 错误码 → 知识点映射

PADA 内置常见 rustc 错误码到 Rust 知识点的映射表：

| 错误码 | 知识点 | 修改方向 |
|--------|--------|----------|
| E0382 | 所有权 / Move | 在移动值之前克隆，或重新设计所有权结构 |
| E0507 | 所有权 / Move | — |
| E0499 | 借用 / Borrow | 确保同一时间只有一个可变借用 |
| E0502 | 借用 / Borrow | 避免同时存在可变与不可变借用 |
| E0106 | 生命周期 / Lifetime | 为引用参数添加生命周期标注 |
| E0597 | 生命周期 / Lifetime | 检查被引用数据的生命周期是否足够长 |
| E0277 | Trait | 为类型实现所需 trait 或调整 trait bound |

未在表中的错误码返回低置信度，交由 LLM 后续推断。

## 会话历史与轨迹（R5）

每次诊断会记录完整的工作流轨迹，包括：

- 每一步的用户输入
- 工具调用及参数（编译、测试、LLM 调用等）
- 工具输出
- Agent 决策依据

会话保存为 JSON 文件，可加载回放：

```bash
# 保存当前会话
cargo run -- diagnose --problem problem.md --code main.rs --save session.json

# 加载历史会话继续诊断
cargo run -- diagnose --problem problem.md --code main.rs --history session.json
```

## Token 用量与预算（R6）

PADA 精确统计每次 LLM 调用的 token 用量（直接取自 API 响应），支持：

- 单次调用用量
- 当前会话累计用量
- 历史累计用量
- 实时成本换算
- 会话 / 周期预算控制

设置预算后，达到上限会自动阻止后续 LLM 调用：

```bash
cargo run -- diagnose --problem problem.md --code main.rs --budget 20000
```

输出用量摘要：

```
=== Token 用量 ===
  本次会话: 输入 97 / 输出 216 / 共 313 token / 成本 0.000529
  会话预算: 20000 / 已用 313 / 剩余 19687
```

## LLM 调用示例

项目附带一个 LLM 调用示例，可直接调用 DeepSeek API 体验 LLM 诊断：

```bash
export DEEPSEEK_API_KEY=sk-xxx
cargo run --example llm_demo
```

输出示例：

```
========== 响应内容 ==========
你的代码中，`s` 是一个 `String` 类型。在 Rust 中，`String` 不是 `Copy` 类型，
所以当执行 `let t = s;` 时，`s` 的所有权被移动到了 `t`...
========== 用量与耗时 ==========
模型        : deepseek-v4-flash
输入 tokens : 97
输出 tokens : 216
成本(元)    : 0.000529
耗时        : 2.44s
```

## 项目结构

```
src/
├── lib.rs              # 库入口
├── main.rs             # CLI 入口（clap 参数解析 + 诊断流程编排）
├── error.rs            # 统一错误类型
├── models.rs           # 核心数据结构
├── report.rs           # 诊断报告格式化（文本 + Markdown）
├── tools/
│   ├── compiler.rs     # 编译工具（rustc / cargo check）
│   ├── runner.rs       # 程序运行器与测试运行器
│   └── test_gen.rs     # 自动测试用例生成（LLM + 确定性解析）
├── analysis/
│   ├── error_parser.rs # rustc 错误解析器
│   ├── classifier.rs   # 错误分类与知识点映射
│   └── hint.rs         # 分层提示生成
├── agent/
│   ├── llm.rs          # LLM 客户端（OpenAI 兼容）
│   └── progress.rs     # 进度渲染与协作式取消（R4）
├── config/
│   └── model.rs        # 模型配置与 profile 管理（R3）
├── history/
│   └── mod.rs          # 会话历史与轨迹持久化（R5）
└── telemetry/
    └── mod.rs          # Token 用量统计与预算控制（R6）
```

## 测试

```bash
# 运行全部测试（185 个）
cargo test

# 运行特定测试
cargo test --test compiler_tests
cargo test --test e2e_tests
```

测试覆盖：

| 测试文件 | 数量 | 覆盖内容 |
|----------|------|----------|
| compiler_tests | 5 | rustc 编译调用 |
| runner_tests | 6 | 程序运行与测试判定 |
| integration_tests | 3 | 编译→运行完整流程 |
| diagnostic_tests | 19 | rustc 错误解析 |
| classifier_tests | 14 | 错误分类与知识点映射 |
| hint_tests | 23 | 分层提示 |
| config_tests | 15 | 模型配置与 profile |
| llm_tests | 14 | LLM 请求构造与响应解析 |
| telemetry_tests | 19 | Token 用量与预算 |
| test_gen_tests | 16 | 自动测试生成 |
| report_tests | 13 | 诊断报告格式化 |
| progress_tests | 14 | 进度与取消 |
| history_tests | 15 | 会话历史持久化 |
| e2e_tests | 9 | 端到端完整工作流 |

## 运行测试

```bash
cargo test
```

## 相关文档

- [DESIGN.md](DESIGN.md) — 设计文档（产品定位、架构、数据结构、开发计划）
- [AGENTS.md](AGENTS.md) — 开发规范（编码要求、测试要求、实现约束）
