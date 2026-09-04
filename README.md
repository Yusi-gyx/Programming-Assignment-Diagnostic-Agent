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
- 彩色标注错误、知识点与提示（输出重定向时自动关闭颜色）
- 集中导出 Markdown 报告，自动保存并恢复最近的历史会话
- 记录 Token 用量与成本，并通过预算避免超额调用
- 保存学习画像，显示掌握度、遗忘衰减和薄弱知识点

## 安装

需要本机已安装 Rust 工具链。

```bash
git clone <repo-url>
cd ProgrammingAssignmentDiagnosticAgent
cargo build --release
```

编译完成后，可直接运行生成的二进制文件：

```bash
./target/release/pada --help
```

如果希望在任意目录直接使用 `pada` 命令，需要将二进制所在目录加入 `PATH`：

```bash
export PATH="$(pwd)/target/release:$PATH"
```

以上设置只对当前终端会话生效。若要永久生效，请将这行命令加入 `~/.bashrc`、
`~/.zshrc` 或当前 Shell 对应的配置文件，然后重新打开终端或加载配置文件。

也可以将二进制复制到已经位于 `PATH` 中的目录，例如：

```bash
sudo cp target/release/pada /usr/local/bin/pada
pada --help
```

开发时无需修改 `PATH`，只需将下文的 `pada` 替换为 `cargo run --`。

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
| `config` | 打开模型配置向导，创建、更新或切换 profile |
| `save <文件名>` | 手动导出当前会话到统一目录 |
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

逐步模式会在编译、分析/测试和报告生成前说明该阶段的操作与用途。按 Enter 或 `c` 执行当前阶段，输入 `a` 连续执行本轮剩余阶段，`h` 查看说明，`q` 安全取消。执行 `recheck` 后会重新恢复逐步确认；逐步模式下不会同时绘制动态进度条，避免终端内容相互覆盖。

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

配置模型后，PADA 会把失败用例、题目和代码批量交给模型进行语义判断，并将结果限制映射到内置 Rust 知识点枚举；映射结果会显示在报告和 Level 3 提示中，并写入学习画像。没有配置模型时会显示“配置 LLM 后自动映射”。

### 导出报告与继续会话

```bash
pada diagnose --problem problem.md --code main.rs \
  --report report.md \
  --save session.json
```

报告和手动导出的会话不会散落在当前目录，而是分别保存到：

```text
~/.pada/reports/
~/.pada/sessions/exported/
```

每次诊断还会自动保存续聊记录（最多 20 条）到 `~/.pada/sessions/auto/`。列出最近记录并选择继续：

```bash
pada resume
# 也可以直接选择列表序号
pada resume 1
```

`resume` 使用的是自动记录，与 `save` 的手动导出互不混淆。仍可加载旧版或外部会话文件：

```bash
pada diagnose --problem problem.md --code main.rs \
  --history session.json
```

### 学习画像

学习画像用于记录“在哪些知识点上练习过、掌握度如何、距离上次练习多久”，并根据诊断结果与 `understood` / `notyet` 反馈更新。默认自动保存在 `~/.pada/learning/profile.json`，无需额外参数；在导师模式输入 `progress` 即可查看。

如果需要使用独立画像，可传入 `--memory <文件>`。同一份未修改提交被重新打开时不会重复计为失败证据；提交内容发生变化后才会记录新的诊断证据。

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

Level 5 会在配置模型后实际生成参考方案；没有 `--config` 时会明确提示如何配置，不再显示占位文本。

也可以先正常启动诊断，然后在导师模式输入 `config`。向导提供 DeepSeek、Ollama 和自定义 OpenAI 兼容接口预设，引导填写 profile、endpoint、API key、模型名、上下文长度、reasoning 和价格，确认后自动生成并立即启用配置。未通过 `--config` 指定文件时，向导默认保存到 `~/.pada/config.toml`，后续启动会自动加载该文件；显式传入 `--config` 时仍优先使用指定文件，原有手写 TOML 配置方式保持不变。Endpoint 可以填写服务根地址（如 `http://localhost:11434`），程序会自动补全 OpenAI 兼容路径。Ollama profile 不会发送其接口不兼容的布尔 `reasoning` 扩展字段。

## 数据目录与颜色

PADA 的默认数据根目录是 `~/.pada`。可以用全局参数 `--data-dir <目录>` 或环境变量 `PADA_HOME` 覆盖，例如：

```bash
pada --data-dir ./pada-data diagnose --problem problem.md --code main.rs
```

真实终端中，错误类型显示为红色、知识点为黄色、提示为蓝色、成功结果为绿色。设置 `NO_COLOR=1` 或把输出重定向到文件时，不输出 ANSI 颜色码。

## 主要参数

| 参数 | 说明 |
|------|------|
| `--problem <文件>` | Markdown 题目描述，必填 |
| `--code <文件>` | Rust 单文件提交 |
| `--project <目录>` | Cargo 项目，与 `--code` 二选一 |
| `--hint <1-5>` | 初始提示等级，默认 1 |
| `--tests <文件>` / `--test <文件>` | JSON 测试用例 |
| `--generate-tests` | 使用模型生成边界测试 |
| `--config <文件>` | 模型配置文件 |
| `--profile <名称>` | 使用指定模型配置 |
| `--budget <数量>` | 会话 Token 预算 |
| `--report <文件名>` | 导出 Markdown 报告到统一报告目录 |
| `--save <文件名>` | 手动导出会话到统一会话目录 |
| `--history <文件>` | 加载历史会话 |
| `--memory <文件>` | 使用自定义学习画像文件（默认自动使用统一画像） |
| `--step` | 逐阶段确认执行 |
| `--no-interactive` | 诊断一次后退出 |

查看完整参数：

```bash
pada diagnose --help
pada resume --help
```

## 开发与测试

```bash
cargo test --all-targets
```

项目架构、数据结构和开发计划参见 [DESIGN.md](DESIGN.md)，贡献规范参见 [AGENTS.md](AGENTS.md)。
