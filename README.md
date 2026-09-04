# PADA — Rust 编程作业诊断 Agent

PADA（Programming Assignment Diagnostic Agent）是一个面向编程学习的 Rust CLI。它把编译器错误、程序测试结果和可选的大模型语义分析组合成分层提示，帮助学习者理解：

> 代码为什么错，以及自己还没有掌握什么知识。

PADA 默认不会替用户直接修好代码，而是先展示可验证的编译与测试证据，再逐步给出错误类别、位置、知识点、修改方向和参考方案。当前只支持 Rust，可处理单文件提交和 Cargo 多文件项目。

## 项目状态

项目处于早期开发阶段，核心 CLI 已可使用。当前实现包括：

- 使用 `rustc` 编译 Rust 单文件，使用 `cargo check` 检查 Cargo 项目
- 解析 rustc 诊断，分类错误并映射常见 Rust 知识点
- 对单文件程序执行 stdin/stdout JSON 测试并记录失败证据
- 五级教学提示，以及在同一会话中升级提示和重新诊断
- OpenAI Chat Completions 兼容模型接入、命名 profile 和配置向导
- 模型辅助的提示增强、失败测试知识点映射和边界测试生成
- Markdown 报告、会话轨迹、自动续聊记录和手动会话导出
- 单次调用与当前会话 Token/成本统计，以及会话 Token 预算
- 分阶段进度、逐步确认和交互式模型调用取消
- 学习画像、反馈记录、遗忘衰减和薄弱点展示；该能力属于提前落地的 V2 实验功能

当前边界：

- Cargo 项目目前只执行 `cargo check`，不会构建并运行项目，也不会应用外部 stdin/stdout 测试。
- 单文件测试按“退出码为 0，且 stdout 去除首尾空白后与 `expected_output` 相同”判定。
- 编译和学生程序子进程尚未接入超时及协作式取消；`q` / `cancel` 目前只用于交互式模型生成。
- CLI 已支持会话预算，但周期预算和跨会话累计用量尚未接入主流程。
- 生成测试依赖模型对题意和期望输出的理解；生成结果会经过 JSON 结构校验，但仍应由用户审阅。

## 设计原则

1. **确定性逻辑优先。** 编译、运行、测试判定、rustc 解析、状态变化、知识点枚举约束、掌握度和成本计算均由 Rust 完成。
2. **教学优先。** 默认从最少的信息开始，用户主动请求后才逐级展开；只有 Level 5 提供参考方案。
3. **证据优先。** 报告保留错误位置、错误码、失败用例、期望输出和实际输出，不让模型代替编译器或测试做事实判断。
4. **模型可选。** 未配置模型时，编译诊断、测试执行、基础分类和基础提示仍可使用。

## 环境要求

- 可构建本项目的稳定版 Rust 工具链
- `cargo` 和 `rustc` 可从 `PATH` 访问
- 使用云端模型时需要相应服务的 API Key 和网络连接
- 使用 Ollama 等本地模型时，需要先启动兼容 OpenAI Chat Completions 的本地服务

```bash
rustc --version
cargo --version
```

## 安装

在仓库根目录构建发布版本：

```bash
cargo build --release
./target/release/pada --help
```

也可以从当前源码目录安装到 Cargo 的二进制目录：

```bash
cargo install --path .
pada --help
```

开发期间可直接运行：

```bash
cargo run -- diagnose --problem problem.md --code main.rs
```

下文统一使用 `pada`。如果没有安装，请将它替换为 `cargo run --`。

## 快速开始

准备题目描述 `problem.md` 和学生代码 `main.rs`：

```bash
pada diagnose --problem problem.md --code main.rs
```

PADA 会编译代码并输出 Level 1 诊断。真实终端中默认继续进入导师模式：

```text
pada[1]> next
pada[2]> hint 3
pada[3]> recheck
```

推荐的学习流程是：阅读当前提示、自己修改源文件、运行 `recheck`，再根据新证据决定是否请求更高等级提示。

脚本或 CI 中可执行一次后退出：

```bash
pada diagnose \
  --problem problem.md \
  --code main.rs \
  --no-interactive
```

## 输入格式

### 题目

`--problem` 接收 UTF-8 文本文件，通常使用 Markdown。文件名（不含扩展名）会作为会话标题，全文会作为题目描述供报告和模型任务使用。

### 单文件提交

`--code` 指向一个 Rust 源文件。PADA 使用 Rust 2021 edition 调用 `rustc`；编译成功后生成临时可执行文件，必要时运行外部测试，诊断结束后删除临时文件。

```bash
pada diagnose --problem problem.md --code student.rs
```

### Cargo 项目

`--project` 指向含 `Cargo.toml` 的项目根目录。PADA 会在该目录执行 `cargo check` 并解析诊断：

```bash
pada diagnose --problem problem.md --project ./student_project
```

`--code` 与 `--project` 必须且只能提供一个。当前 Cargo 项目模式不运行 `cargo test`、项目二进制或外部 JSON 测试。

### JSON 测试

`--tests`（别名 `--test`）接收 JSON 数组，每项必须包含 `name`、`input` 和 `expected_output`：

```json
[
  {
    "name": "empty_input",
    "input": "",
    "expected_output": "0"
  },
  {
    "name": "normal_case",
    "input": "1 2 3\n",
    "expected_output": "6"
  }
]
```

运行单文件测试：

```bash
pada diagnose \
  --problem problem.md \
  --code main.rs \
  --tests tests.json
```

每个用例的 `input` 会写入程序标准输入。只有程序正常退出且实际标准输出与期望输出在去除首尾空白后完全相等，才算通过。当前报告不会把 stderr 作为实际输出展示，也不支持浮点误差或自定义比较器。

进入导师模式后，可以替换当前测试集并立即重新诊断：

```text
pada[2]> test tests.json
```

## 五级提示

| 等级 | 内容 | 默认生成方式 |
|---|---|---|
| 1 | 错误类别 | Rust 确定性生成 |
| 2 | 错误位置 | Rust 确定性生成 |
| 3 | 相关知识点 | Rust 映射；配置模型后可补充教学解释 |
| 4 | 修改方向 | Rust 基础方向；配置模型后可增强 |
| 5 | 参考方案 | 配置模型时生成结构化参考方案，否则使用基础提示 |

启动时可设置初始等级：

```bash
pada diagnose --problem problem.md --code main.rs --hint 3
```

Level 3 和 Level 4 的模型提示明确要求使用与本题不同的通用示例，不输出当前作业的完整答案。Level 5 才允许提供与错误相关的最小参考片段。

## 导师模式命令

命令前的反斜杠可以省略，例如 `next` 与 `\next` 等价。

| 命令 | 作用 |
|---|---|
| `next` | 进入下一级提示并重新显示报告 |
| `hint [1-5]` | 查看当前等级，或直接切换到指定等级 |
| `recheck` | 重新读取已修改的提交并开始下一轮诊断 |
| `show` | 再次显示当前等级的报告 |
| `test <文件.json>` / `tests <文件.json>` | 替换测试集并重新诊断 |
| `case` | 调用当前模型生成测试 JSON 文件 |
| `progress` / `process` | 查看学习画像、掌握度和薄弱点 |
| `understood` / `懂了` | 为当前诊断涉及的知识点记录“已经理解” |
| `notyet` / `还不会` | 为当前诊断涉及的知识点记录“还没理解” |
| `usage` | 查看当前会话的 Token 与成本统计 |
| `config` | 启动模型配置向导并立即应用选定 profile |
| `save <文件名>` | 手动导出当前会话 JSON |
| `help` / `?` | 显示命令帮助 |
| `exit` / `quit` | 保存当前状态并退出 |

## 常用工作流

### 逐阶段确认

```bash
pada diagnose --problem problem.md --code main.rs --step
```

逐步模式会在编译、分析/测试和报告生成前说明即将执行的操作：

- `Enter` 或 `c`：执行当前阶段
- `a`：连续执行本轮剩余阶段
- `h`：查看说明
- `q`：在阶段开始前安全取消

执行 `recheck` 后会重新启用逐步确认。非交互环境传入 `--step` 时会提示并按连续模式运行。逐步模式不会同时绘制动态进度条。

### 导出 Markdown 报告

```bash
pada diagnose \
  --problem problem.md \
  --code main.rs \
  --report result.md
```

`--report` 只取所给路径的文件名，并将报告写入统一数据目录的 `reports/`，不会写到参数中指定的其他父目录。

### 保存、加载与继续会话

手动导出会话：

```bash
pada diagnose \
  --problem problem.md \
  --code main.rs \
  --save rust-session.json
```

会话文件包含输入、工具调用摘要、工具输出摘要、Agent 决策、模型交互和用量记录。导师模式遇到同名导出文件会询问覆盖或更换名称；非交互模式拒绝静默覆盖。

每个导师会话还会自动保存，最多保留最近 20 条。列出并继续记录：

```bash
pada resume
pada resume 1
pada resume session_123456789
```

`resume` 使用自动会话记录恢复原题目、提交、测试、profile、提示等级和预算等启动上下文，并重新执行诊断。旧版会话如果没有恢复上下文，只能用 `--history` 加载轨迹：

```bash
pada diagnose \
  --problem problem.md \
  --code main.rs \
  --history exported-session.json
```

### 使用模型生成边界测试

启动时直接生成并运行附加测试：

```bash
pada diagnose \
  --problem problem.md \
  --code main.rs \
  --profile local \
  --generate-tests \
  --budget 20000
```

`--generate-tests` 生成的用例会加入本次内存中的测试集。导师模式中的 `case` 会要求模型生成 5–8 个用例，并将成功解析出的用例保存到题目文件所在目录：

```text
pada[2]> case
已生成 6 个测试用例并保存到: .../generated_tests.json
pada[2]> test .../generated_tests.json
```

文件名从 `generated_tests.json` 开始；存在同名文件时使用 `generated_tests_2.json` 等名称，不覆盖已有文件。

## 模型配置

模型不是基础诊断的必需项。需要模型增强、测试失败知识点映射或自动测试生成时，可在导师模式输入 `config`，也可手工创建数据目录下的 `config.toml`：

```toml
active_profile = "local"

[profiles.local]
endpoint = "http://localhost:11434"
api_key = ""
model_name = "qwen2.5-coder"
context_length = 32768
reasoning = false
input_price = 0.0
output_price = 0.0

[profiles.cloud]
endpoint = "https://api.example.com/v1/chat/completions"
api_key = "your-api-key"
model_name = "your-model"
context_length = 64000
reasoning = false
input_price = 1.0
output_price = 2.0
```

字段说明：

| 字段 | 说明 |
|---|---|
| `active_profile` | 未传 `--profile` 时使用的 profile |
| `endpoint` | OpenAI 兼容服务根地址、`/v1` 地址或完整 Chat Completions 地址 |
| `api_key` | Bearer API Key；本地服务可留空 |
| `model_name` | 请求体中的模型名称 |
| `context_length` | 模型上下文长度配置 |
| `reasoning` | 是否向兼容服务请求 reasoning；Ollama 默认端口不会发送该布尔扩展 |
| `input_price` | 每百万输入 Token 的价格 |
| `output_price` | 每百万输出 Token 的价格 |

服务根地址会自动补全为 `/v1/chat/completions`，以 `/v1` 结尾的地址会补全 `/chat/completions`。已有完整地址会原样使用。

API Key 以明文保存在 TOML 中，请限制配置文件权限，不要提交到版本控制。价格单位由用户自行统一，例如全部填写人民币或美元；PADA 只按同一单位计算数值，不做货币转换。

选择指定 profile：

```bash
pada diagnose \
  --problem problem.md \
  --code main.rs \
  --profile cloud
```

如果不存在配置文件，PADA 不调用模型；显式指定不存在的 profile 会报错。模型响应中的 `usage.prompt_tokens` 和 `usage.completion_tokens` 用于统计 Token，PADA 不在本地估算缺失用量。

## Token、成本与取消

`--budget <数量>` 设置当前进程内的会话总 Token 上限。每次模型调用前，PADA 检查已经记录的输入与输出 Token；达到上限后阻止后续调用：

```bash
pada diagnose --problem problem.md --code main.rs --budget 10000
```

预算不是单次请求的预留上限，因此最后一次已开始的调用可能使累计值超过预算。导师模式输入 `usage` 可查看当前会话记录和按配置价格计算的成本。

交互式生成 Level 3–5 提示或使用 `case` 生成测试时，可输入 `q` 或 `cancel` 并回车。PADA 会关闭当前流式 HTTP 响应，丢弃未完成结果并返回导师命令行。启动参数 `--generate-tests` 当前使用非交互请求，不能通过该命令取消。

## 学习画像

学习画像默认保存在数据目录的 `learning/profile.json`。它根据诊断结果和 `understood` / `notyet` 反馈更新知识点掌握度，并以 30 天时间常数应用遗忘衰减。相同提交产生的同类自动诊断通过稳定证据键去重，避免仅重新打开会话就重复扣分。

```text
pada[2]> progress
```

如需隔离画像，可传入自定义文件：

```bash
pada diagnose \
  --problem problem.md \
  --code main.rs \
  --memory ./profiles/learner-a.json
```

画像摘要会注入模型任务，用于优先解释薄弱点。掌握度、置信度、衰减与薄弱点排序都由 Rust 计算。该功能属于 V2 实验能力，模型分值不应被视为正式学习评价。

## 数据目录

数据根目录按以下优先级确定：

1. 全局参数 `--data-dir <目录>`
2. 环境变量 `PADA_HOME`
3. `$HOME/.pada`
4. 无法取得 HOME 时使用当前目录 `.pada`

```bash
pada --data-dir ./pada-data diagnose --problem problem.md --code main.rs
PADA_HOME=./pada-data pada resume
```

目录布局：

```text
~/.pada/
├── config.toml
├── learning/
│   └── profile.json
├── reports/
├── sessions/
│   ├── auto/       # 自动续聊记录，最多 20 条
│   └── exported/   # save / --save 手动导出
```

`--report` 和 `--save` 只使用传入路径的文件名，分别落到 `reports/` 和 `sessions/exported/`。

## 输出与颜色

真实终端中，错误类别、知识点、提示和成功状态使用语义颜色。stdout 不是终端或设置 `NO_COLOR` 时自动输出纯文本：

```bash
NO_COLOR=1 pada diagnose --problem problem.md --code main.rs
```

模型调用显示独立的开始、完成、失败或取消状态；基础诊断显示编译、分析/测试和报告三个阶段的进度。

## CLI 参数

顶层命令：

```text
pada [--data-dir <目录>] diagnose [选项]
pada [--data-dir <目录>] resume [会话序号或 ID] [--no-interactive]
```

`diagnose` 参数：

| 参数 | 必需 | 说明 |
|---|---:|---|
| `--problem <文件>` | 是 | 题目描述 |
| `--code <文件>` | 二选一 | Rust 单文件提交 |
| `--project <目录>` | 二选一 | 含 `Cargo.toml` 的 Cargo 项目 |
| `--tests <文件>` / `--test <文件>` | 否 | stdin/stdout JSON 测试 |
| `--generate-tests` | 否 | 调用模型生成测试并加入本次测试集 |
| `--hint <1-5>` | 否 | 初始提示等级，默认 1 |
| `--profile <名称>` | 否 | 覆盖配置中的活动 profile |
| `--budget <数量>` | 否 | 当前会话 Token 预算 |
| `--report <文件名>` | 否 | 导出 Markdown 报告 |
| `--history <文件>` | 否 | 加载已有会话 JSON |
| `--save <文件名>` | 否 | 手动导出会话 JSON |
| `--memory <文件>` | 否 | 使用独立学习画像 |
| `--step` | 否 | 逐阶段确认 |
| `--no-interactive` | 否 | 单次诊断后退出 |

以当前二进制输出为准查看帮助：

```bash
pada --help
pada diagnose --help
pada resume --help
```

## 诊断机制

PADA 的确定性诊断链路如下：

1. 读取题目、提交和可选测试。
2. 单文件调用 `rustc`；Cargo 项目调用 `cargo check`。
3. 编译失败时解析诊断头、错误码、主位置和 note/help。
4. 将已知错误码映射到 `Ownership`、`Borrowing`、`Lifetime`、`Trait` 等内置知识点。
5. 单文件编译成功且存在测试时，运行每个用例并比较输出。
6. 配置模型后，对失败测试进行批量语义映射，并把输出限制在内置知识点枚举内。
7. 按当前等级生成提示，更新会话轨迹和学习画像，输出报告。

无法确定的知识点会保持为空或回退为基础逻辑错误分类，而不是让模型创造新的分类值。

## 故障排查

- **提示找不到 `rustc` / `cargo`**：确认 Rust 工具链已安装且命令位于 `PATH`。
- **Cargo 项目提示找不到文件**：`--project` 必须指向包含 `Cargo.toml` 的目录。
- **测试 JSON 解析失败**：确认顶层是数组，且每项三个字段都是字符串。
- **模型 profile 不存在**：检查数据目录中的 `config.toml` 和 `active_profile`，或在导师模式运行 `config`。
- **模型接口返回 404**：检查 endpoint；通常可以填写服务根地址，PADA 会补全标准路径。
- **模型响应无法解析**：自动测试要求 JSON 数组，测试知识点映射要求受限 JSON；服务若额外输出说明文字可能导致失败。
- **`resume` 没有记录**：确认使用了同一个 `--data-dir` 或 `PADA_HOME`，且此前启动过导师会话。
- **输出包含 ANSI 颜色码**：设置 `NO_COLOR=1`。

## 开发

核心业务使用 Rust 实现，`src/main.rs` 负责 CLI 参数、流程编排和输出，领域逻辑位于库模块：

```text
src/
├── agent/       # 交互、模型任务、提示增强、导出与进度
├── analysis/    # rustc 解析、分类、知识点映射和基础提示
├── config/      # 模型 profile 与配置向导
├── history/     # 会话、轨迹和序列化
├── memory/      # 学习画像、衰减与反馈
├── telemetry/   # Token、成本与预算
├── tools/       # 编译、运行和测试生成
├── models.rs    # 核心领域类型
├── report.rs    # 文本、彩色文本和 Markdown 报告
├── storage.rs   # 数据目录与文件生命周期
└── main.rs      # CLI 入口和工作流编排
```

运行完整测试：

```bash
cargo test --all-targets
```

提交前建议同时运行：

```bash
cargo fmt --check
cargo clippy --all-targets --all-features
```

测试覆盖编译器调用、rustc 解析、分类映射、提示等级、Runner、报告、配置、会话历史、存储、模型任务、Token/预算、交互命令和学习画像。真实 Rust 样例位于 `tests/fixtures/rust/`，可手动体验的作业位于 `homework/`。

架构与实现约束参见 [DESIGN.md](DESIGN.md)，开发约定参见 [AGENTS.md](AGENTS.md)。

## 后续重点

当前优先完成 V1，而不是扩展 Web UI、IDE 插件或其他语言。剩余重点包括：

- 为编译和学生程序子进程补充超时、取消与资源回收
- 将跨会话累计用量、周期预算和持久化接入 CLI
- 完善历史轨迹的查看与回放入口
- 提升 Cargo 项目的构建、运行和测试能力
- 扩充 rustc 错误码与知识点映射覆盖率
