# PADA — Rust 编程作业诊断 Agent

PADA 是一个面向编程学习的命令行工具。它不会一上来就替你改好代码，而是像导师一样，通过编译、测试和逐级提示，帮助你弄清楚：

> 代码为什么错，以及自己还没有掌握什么知识。

目前支持 Rust 单文件和 Cargo 项目。

## 特色功能

- 自动调用 `rustc` / `cargo check`，整理难读的编译错误
- 将错误映射到所有权、借用、生命周期、Trait 等 Rust 知识点
- 提供五级提示，从错误类别逐步深入到修改方向和参考方案
- 在同一个会话中修改代码、重新检查并继续获得指导
- 运行自定义测试，也可调用大模型生成边界测试
- 导出 Markdown 诊断报告，保存和继续历史会话
- 记录 Token 用量与成本，并通过预算避免超额调用
- 保存学习画像，显示掌握度、遗忘衰减和薄弱知识点

## 安装

需要本机已安装 Rust 工具链。

```bash
git clone <repo-url>
cd ProgrammingAssignmentDiagnosticAgent
cargo build --release
```

之后可以使用 `target/release/pada`。开发时也可以将下文的 `pada` 替换为 `cargo run --`。

## 快速开始

准备题目文件 `problem.md` 和自己的 Rust 代码 `main.rs`：

```bash
pada diagnose --problem problem.md --code main.rs
```

PADA 会先给出一级提示：

```text
[编译错误] main.rs:6:20  E0382 (borrow of moved value: `s`)
  知识点 : 所有权 / Move
  提示   : 这是一个编译错误
```

在真实终端中，诊断完成后会自动进入导师模式：

```text
pada[1]> next
pada[2]> next
pada[3]> recheck
```

你可以先阅读提示、自行修改代码，再输入 `recheck` 重新诊断。

## 常用交互命令

| 命令 | 作用 |
|------|------|
| `next` | 查看下一级提示 |
| `hint [1-5]` | 查看或切换提示等级 |
| `recheck` | 修改代码后重新诊断 |
| `show` | 再次显示当前诊断 |
| `progress` | 查看知识点掌握度和薄弱点 |
| `understood` | 记录“已经理解” |
| `notyet` | 记录“还没理解” |
| `usage` | 查看 Token 用量与成本 |
| `save <文件>` | 保存当前会话 |
| `help` | 查看命令帮助 |
| `exit` | 退出 |

命令也兼容 `\next`、`\hint` 等带反斜杠的写法。

## 提示等级

PADA 默认从最少的信息开始，避免直接泄露答案。

| 等级 | 提示内容 |
|------|----------|
| 1 | 错误类别 |
| 2 | 错误位置 |
| 3 | 相关知识点 |
| 4 | 修改方向 |
| 5 | 参考方案 |

可以在启动时指定等级：

```bash
pada diagnose --problem problem.md --code main.rs --hint 3
```

## 常见使用方式

### 诊断 Cargo 项目

```bash
pada diagnose --problem problem.md --project ./my_project
```

### 逐步确认每个阶段

```bash
pada diagnose --problem problem.md --code main.rs --step
```

用于脚本或 CI 时，可以关闭导师模式：

```bash
pada diagnose --problem problem.md --code main.rs --no-interactive
```

### 运行自定义测试

测试文件是一个 JSON 数组：

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

运行测试：

```bash
pada diagnose --problem problem.md --code main.rs --tests tests.json
```

### 导出报告与继续会话

```bash
pada diagnose --problem problem.md --code main.rs \
  --report report.md \
  --save session.json
```

之后可以加载原会话继续诊断：

```bash
pada diagnose --problem problem.md --code main.rs \
  --history session.json
```

### 保存学习画像

```bash
pada diagnose --problem problem.md --code main.rs \
  --memory learning.json
```

PADA 会根据诊断结果和 `understood` / `notyet` 反馈更新掌握度。再次使用相同的画像文件，就能延续学习记录。

## 使用大模型生成边界测试

PADA 支持 OpenAI 兼容接口，例如 DeepSeek 或本地 Ollama。创建 `config.toml`：

```toml
active_profile = "local"

[profiles.local]
endpoint = "http://localhost:11434/v1/chat/completions"
api_key = ""
model_name = "qwen2.5-coder"
context_length = 32768
reasoning = false
input_price = 0.0
output_price = 0.0
```

生成并运行边界测试：

```bash
pada diagnose --problem problem.md --code main.rs \
  --config config.toml \
  --profile local \
  --generate-tests \
  --budget 20000
```

`--budget` 是本次会话的 Token 上限。达到预算后，PADA 会阻止后续模型调用。

## 主要参数

| 参数 | 说明 |
|------|------|
| `--problem <文件>` | Markdown 题目描述，必填 |
| `--code <文件>` | Rust 单文件提交 |
| `--project <目录>` | Cargo 项目，与 `--code` 二选一 |
| `--hint <1-5>` | 初始提示等级，默认 1 |
| `--tests <文件>` | JSON 测试用例 |
| `--generate-tests` | 使用模型生成边界测试 |
| `--config <文件>` | 模型配置文件 |
| `--profile <名称>` | 使用指定模型配置 |
| `--budget <数量>` | 会话 Token 预算 |
| `--report <文件>` | 导出 Markdown 报告 |
| `--save <文件>` | 保存会话 |
| `--history <文件>` | 加载历史会话 |
| `--memory <文件>` | 加载并保存学习画像 |
| `--step` | 逐阶段确认执行 |
| `--no-interactive` | 诊断一次后退出 |

查看完整参数：

```bash
pada diagnose --help
```

## 开发与测试

```bash
cargo test --all-targets
```

项目架构、数据结构和开发计划参见 [DESIGN.md](DESIGN.md)，贡献规范参见 [AGENTS.md](AGENTS.md)。
