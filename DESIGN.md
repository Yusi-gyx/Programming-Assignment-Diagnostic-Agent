# 编程作业诊断 Agent 设计文档

## 1. 产品定位

本项目实现一个面向 **编程学习** 的编程作业诊断 Agent。

用户输入：

* 编程题目
* 自己编写的代码
* 可选测试样例

Agent 不以“直接把代码修好”为主要目标，而是通过：

* 编译代码
* 运行测试
* 分析错误
* 定位知识点
* 提供分层提示

帮助用户理解：

> **代码为什么错，以及自己没有掌握什么知识。**

V1 只支持 Rust，后续再考虑扩展其他语言。

---

## 2. 用户痛点与通用 Agent 的不足

对话式大模型和claude、codex等AI辅助编程工具均能为用户找到代码中的问题，但是它们无法像一位导师一般指导用户逐步解决自己的错误，也没有对用户学习进度的记忆。

本项目解决的是教学场景中的问题：

* 用户代码虽然被修好，但并没有深入理解错误原因
* 用户出现错误后需要手动查找官方教程、文档网址进行学习
* 对于可读性较差、逻辑较复杂的代码编程 Agent 无法总是自动为用户指出
* 用户缺乏主动构造边界测试的意识
* 同类错误可能反复出现，但通用 Agent 不会持续分析知识薄弱点
* 通用 Agent 无法记忆用户的学习进度，每次都会从头开始事无巨细地为用户讲解一遍

通用 Agent 通常采用：

```text
发现 Bug
→ 修改代码
→ 测试通过
```

本项目采用：

```text
发现问题
→ 收集编译器/测试证据
→ 判断错误类型
→ 映射知识点
→ 分层提示
→ 用户自行修改
→ Agent根据修改结果继续指导
→ 循环
......
→ Agent为用户总结薄弱点
```

---

## 3. Agent 基础能力要求

本节描述 Agent 自身必须具备的基础能力。这些能力与具体的 Rust 诊断场景相对独立，是支撑整个系统运行、并保证可用性与可观测性的底层要求。R3 ~ R6 为强制实现项，应在 V1 阶段随核心功能一并交付。

### 3.1 可自定义模型配置（R3）

Agent 必须允许用户自由修改大模型相关配置，包括但不限于：

* API Endpoint
* API Key
* 上下文长度
* 思考模式（是否启用推理链 / reasoning）
* API 价格（输入 token 单价、输出 token 单价）

配置方式必须同时支持：

* 配置文件（`.env` 或 `config.toml`），适合持久化与脚本化
* 用户友好的 UI / CLI 设置入口，适合交互式修改

典型场景：用户可以在 DeepSeek 云端模型与本地部署模型之间一键切换，而无需改动代码。多组配置应能以命名 profile 形式保存与切换。

### 3.2 实时进度渲染与打断（R4）

对于执行时间可能超过 3 秒的任务，UI / CLI 必须实时渲染进度，并允许用户随时打断。

进度展示应体现具体阶段与计数，而非笼统的「处理中」，例如：

* 图片批处理：「已处理 45/120 张」
* 编译诊断：「正在运行第 3/8 组测试 ...」

实现方式：

* Web 端：SSE / WebSocket 推送进度事件
* CLI 端：进度条库（如 `indicatif`）

任务必须支持协作式取消（cooperative cancellation）。打断后需安全释放子进程、文件句柄等资源，并尽可能保留已完成的部分结果，便于后续续跑或诊断。

### 3.3 上下文历史管理（R5）

Agent 必须管理多轮对话与任务状态的完整历史记录。用户能够：

* 查看历史任务列表
* 查看单次会话的完整工作流程，即 Agent 实际的思考、工具调用与中间结果轨迹，而不是把 Agent 当作黑盒（可参考 DeepSeek Harness 的轨迹展示）
* 将某次会话的完整上下文保存为文件（如 JSON）
* 从文件加载历史上下文，继续工作或复盘
* 自动保存每次诊断会话，并通过 `resume` 列出和继续最近会话

轨迹应至少记录：每一步的输入、调用的工具与参数、工具输出、Agent 决策依据，以便用户与开发者审计和回放。

自动续聊记录与用户手动导出的会话分开存储；自动记录最多保留最近 20 条，超出后删除最旧记录。会话保存恢复诊断所需的题目、提交、测试、模型 profile 与提示等级等上下文。

### 3.4 Token 用量与价格统计（R6）

系统必须精确统计每次 API 调用：

* 输入 token 数
* 输出 token 数

这两项直接取自 API 响应，不需要本地估算。

系统应基于配置的模型价格（见 R3）实时换算成本，并在界面上清晰展示：

* 单次调用用量与花费
* 当前会话累计用量与花费
* 历史累计用量与花费

此外必须支持：

* 设置 token 预算（按会话或按周期）
* 用量达到预算时自动中断后续调用，避免超支

---

## 4. 场景定制

### 4.1 Rust 错误诊断

结合 `rustc` / `cargo` 的错误信息与程序运行结果进行诊断，覆盖编译错误与运行时 / 逻辑错误。

编译错误示例（错误码 → 知识点）：

```text
E0382 → Ownership / Move
E0499 → Mutable Borrow
E0502 → Borrowing Conflict
```

诊断结果统一按以下格式输出，使每个失败点都包含用例、期望输出、实际输出与提示：

```text
[编译错误] main.rs:7:5  E0382 (borrow of moved value)
  知识点 : Ownership / Move
  提示   : <按当前 HintLevel 给出>

[测试失败] test_case_03
  输入     : vec![1, 2, 3]
  期望输出 : [3, 2, 1]
  实际输出 : [1, 2, 3]
  知识点   : Iterator
  提示     : <按当前 HintLevel 给出>
```

系统利用编译器和程序实际运行结果，将复杂的报错与失败用例转化为清晰的格式化改错指导。

### 4.2 自动诊断测试

除题目已有测试外，Agent 可以生成额外测试，例如：

* 作业用例之外的多组测试
* 空输入
* 单元素
* 重复元素
* 负数
* 边界值
* 大规模输入

测试用于判断代码在哪类情况下失败。

### 4.3 分层提示

Agent 默认不直接给完整答案。

提示分为多个等级，在每次进行诊断、指导前由用户选择：

1. 错误类别
2. 错误位置
3. 相关知识点
4. 修改方向
5. 参考方案

用户可以逐步请求更详细的提示。

### 4.4 学习进度记忆化

Agent 能记忆用户各知识点的学习进度，并基于历史诊断给出个性化指导。核心是一个**可计算、可衰减、可注入**的知识掌握度模型。所有数值计算（衰减、更新、置信度）由 Rust 完成，LLM 不参与数值判断，仅负责把画像摘要自然语言化。

#### 4.4.1 掌握度数据模型

为每个 `KnowledgePoint` 维护：

```rust
struct Mastery {
    point: KnowledgePoint,
    score: f32,          // 原始掌握度 [0,1]，由历史证据累计
    confidence: f32,     // 置信度，随样本量上升
    last_seen: DateTime, // 最近一次练习时间，用于遗忘衰减
    history: Vec<MasteryEvent>,
    last_diagnostic_key: Option<String>, // 最近一次提交内容/诊断指纹，防止重复记分
}

enum MasteryEvent {
    Diagnostic { pass: bool, ts: DateTime },
    UserFeedback { understood: bool, ts: DateTime },
}
```

CLI 进度条展示（格式后续可美化，每个‘#’表示5%）：

```
Ownership: [############--------] 60% 
```

#### 4.4.2 掌握度更新与遗忘衰减

掌握度随时间衰减，体现「学过 ≠ 掌握」。采用 Ebbinghaus 遗忘曲线：

```text
effective_mastery = score * exp(-Δt / τ)
```

* `Δt`：距上次练习（`last_seen`）的时间间隔
* `τ`：衰减时间常数，可按知识点难度或用户差异调整（默认值后续标定）

所有用于诊断、注入的实际掌握度取 `effective_mastery`，而非原始 `score`。

更新规则（新证据到达时）：

* 指数移动平均（EMA）：
  `score_new = α * evidence + (1 - α) * score_old`
  * `evidence ∈ [0,1]`：诊断通过取高、失败取低；用户反馈「已掌握」取 1、「未掌握」取 0
  * `α`：学习率，置信度低时较大、高时较小
* 也可采用贝叶斯更新（Beta 分布），用成功/失败次数驱动，天然产出置信度（具体方案后续选型）

#### 4.4.3 记忆写入时机

* 每次诊断结束：以错误分类 + 测试结果作为证据写入
* 用户显式反馈：「我懂了 / 还不会」
* 重复错误检测：同知识点反复出错时降低掌握度并提升置信度

同一份未修改提交产生的相同自动诊断只记为一次证据，避免关闭后重新打开会话时重复扣分；提交内容变化后才视为新的练习证据。距上次练习时间按秒、分钟、小时或天显示，而不是只取整到天。

#### 4.4.4 记忆读取与注入

* 新会话开始时，由 Rust 组装用户画像摘要（薄弱点列表、偏好起始提示级别）
* 将摘要注入 LLM 的 system prompt，避免重复讲解已掌握内容，并对薄弱点主动提示
* 符合「确定性优先」原则：画像摘要由程序组装，LLM 仅负责自然语言化

---

## 5. 核心交互流程与CLI参数

```text
用户提交题目和代码
        ↓
模型读取并解析输入
        ↓
调用 rustc / cargo check
        ↓
编译失败？
   ┌────┴────┐
   │         │
   是        否
   │         │
分析编译错误 运行测试
   │         │
   └────┬────┘
        ↓
    生成额外测试
        ↓
通过所有测试？
   ┌────┴────┐
   |         |
   ↓未通过    ↓ 通过
错误分类    更新掌握度
    ↓
映射 Rust 知识点
    ↓
生成诊断结果
    ↓
输出分层提示
```

V1 采用 CLI 交互。

启动参数示例：

```bash
# 基本诊断
pada diagnose --problem problem.md --code main.rs

# 指定初始提示等级与模型 profile（R3）
pada diagnose --problem problem.md --code main.rs --hint 2 --profile local

# Cargo 多文件项目（V1 增强）
pada diagnose --problem problem.md --project ./my_project

# 加载历史会话继续诊断（R5）
pada diagnose --problem problem.md --code main.rs --history session_01.json

# 设置本次会话 token 预算（R6）
pada diagnose --problem problem.md --code main.rs --budget 20000

# 导出诊断报告为 Markdown
pada diagnose --problem problem.md --code main.rs --report report.md

# 列出最近自动保存的会话并选择继续
pada resume
pada resume 1
```

启动参数：

| 参数 | 说明 |
|------|------|
| `diagnose` | 诊断子命令 |
| `--problem <file>` | 题目描述（Markdown） |
| `--code <file>` | 单文件用户代码 |
| `--project <dir>` | Cargo 多文件项目目录（V1 增强） |
| `--hint <level>` | 初始提示等级 1–5，默认 1 |
| `--profile <name>` | 使用指定模型 profile（R3） |
| `--budget <n>` | 本次会话 token 预算（R6） |
| `--history <file>` | 加载历史会话上下文（R5） |
| `--report <file>` | 导出 Markdown 诊断报告 |

全局 `--data-dir <dir>` 可覆盖默认的 `~/.pada` 数据根目录，也可使用 `PADA_HOME` 环境变量。

交互式命令（会话内）：

| 命令 | 说明 |
|------|------|
| `\next` | 请求下一级提示 |
| `\hint [level]` | 查看 / 调整提示等级 |
| `\process` | 输出当前学习进度 |
| `\config` | 查看 / 修改模型配置（R3） |
| `\usage` | 查看 token 用量与花费（R6） |
| `\save <file>` | 保存当前会话上下文（R5） |
| `\new` | 新建对话，清除上下文 |
| `\help` | 显示帮助 |
| `\exit` | 退出 |

所有诊断会话自动保存到 `sessions/auto/`，供顶层 `resume` 命令使用；`save`/`--save` 是独立的手动导出操作，保存到 `sessions/exported/`。Markdown 报告统一保存到 `reports/`，学习画像默认保存到 `learning/profile.json`。每次保存后 CLI 输出完整路径。

真实终端采用语义颜色：错误类型红色、知识点黄色、提示蓝色、成功结果绿色；非终端输出或设置 `NO_COLOR` 时保持纯文本。

为降低命令记忆成本，命令前的反斜杠可省略。真实终端默认进入导师模式；
`--no-interactive` 可用于 CI / 脚本，`--step` 会在编译、测试生成与分析阶段等待确认。
用户修改源文件后执行 `recheck` 即可在同一会话继续诊断。

V1 增强参数：

| 参数 | 说明 |
|------|------|
| `--tests <file>` | 加载 JSON 格式的输入/期望输出测试 |
| `--generate-tests` | 使用所选模型生成边界测试并实际运行 |
| `--step` | 逐阶段确认执行 |
| `--no-interactive` | 单次执行后退出 |

V2 学习画像使用 `--memory <file>` 持久化。会话内 `progress` 查看掌握度，
`understood` / `notyet` 记录对当前诊断知识点的用户反馈。画像更新、置信度、
遗忘衰减和薄弱点排序均由 Rust 完成；画像摘要仅作为上下文注入 LLM。

`\process` 统一输出格式：

```text
知识点掌握度：
  Ownership : [############--------] 60%   (上次练习: 3 天前)
  Borrowing : [################----] 80%   (上次练习: 1 天前)
  Lifetime  : [####----------------] 20%   (上次练习: 10 天前)

薄弱点: Lifetime, Ownership
```

进度条固定 20 格，每个 `#` / `-` 代表 5%，显示百分比与距上次练习的时间，末行汇总薄弱点。

---

## 6. 功能优先级

> R3 ~ R6 属于 Agent 基础设施，必须在 V1 阶段随核心功能一并落地，不延后到 V2。

### V1 核心功能

第一阶段实现：

* 读取题目和 Rust 代码
* 调用 `rustc` / `cargo check`
* 获取并解析编译错误
* 运行测试用例
* 记录测试结果
* 错误分类
* Rust 知识点映射
* 分层提示
* 输出诊断报告

### V1 增强功能

第二阶段实现：

* 自动生成边界测试
* 多轮提示交互
* Markdown 报告导出
* Cargo 多文件项目支持

### V2

第三阶段实现：

* 保存历史诊断
* 建立用户知识掌握模型
* 识别反复出现的薄弱知识点
* 支持多种编程语言，用户可选

当前 V2 迭代已实现前三项中的学习画像、历史证据与薄弱点识别；多语言后端仍作为
后续独立里程碑，当前 CLI 保持只接受 Rust，避免在没有相应编译器错误映射与测试
判定能力时宣称不完整的语言支持。

---

## 7. 核心数据结构

```rust
struct Assignment {
    title: String,
    description: String,
}

struct Submission {
    source_code: String,
    test_results: Vec<TestResult>,
}

struct TestResult {
    name: String,
    passed: bool,
    actual_output: String,
}

struct Diagnostic {
    category: ErrorCategory,
    knowledge_points: Vec<KnowledgePoint>,
    confidence: f32,
}
```

主要错误类别：

```rust
enum ErrorCategory {
    CompileError,
    RuntimeError,
    LogicError,
    BoundaryCondition,
    AlgorithmError,
}
```

Rust 知识点：

```rust
enum KnowledgePoint {
    Ownership,
    Borrowing,
    Lifetime,
    Trait,
    Generic,
    Iterator,
    Option,
    Result,
    PatternMatching,
    Collection,
    ErrorHandling,
    AlgorithmLogic,
}
```

提示级别：

```rust
enum HintLevel {
    Category,
    Location,
    Concept,
    Direction,
    Solution,
}
```

知识掌握度（见 §4.4）：

```rust
struct Mastery {
    point: KnowledgePoint,
    score: f32,
    confidence: f32,
    last_seen: DateTime,
    history: Vec<MasteryEvent>,
}

enum MasteryEvent {
    Diagnostic { pass: bool, ts: DateTime },
    UserFeedback { understood: bool, ts: DateTime },
}
```

---

## 8. 技术设计

项目核心使用 Rust 实现。

预计使用：

```text
clap
→ CLI

tokio
→ 异步运行时

serde / serde_json
→ 数据序列化

thiserror / anyhow
→ 错误处理

std::process 或 tokio::process
→ 调用 rustc / cargo
```

初步模块划分：

```text
PADA/
├── Cargo.toml
├── README.md
├── DESIGN.md
├── AGENTS.md
│
├── src/
│   ├── lib.rs
│   ├── main.rs
│   ├── agent/
│   ├── analysis/
│   ├── tools/
│   ├── memory/          # 学习进度记忆化（掌握度 / 遗忘衰减 / 画像注入）
│   ├── config/          # R3 模型配置（endpoint / key / 价格 / profile）
│   ├── history/         # R5 会话与轨迹持久化
│   ├── storage.rs       # 报告、自动/手动会话、学习画像的统一目录
│   ├── telemetry/       # R6 token 用量与成本统计、预算控制
│   └── ...
│
├── tests/
│   ├── compiler_tests.rs
│   ├── diagnostic_tests.rs
│   ├── integration_tests.rs
│   │
│   └── fixtures/
│       ├── rust/
│       │   ├── ownership/
│       │   ├── borrowing/
│       │   ├── option/
│       │   ├── iterator/
│       │   ├── runtime/
│       │   └── logic/
│       │
│       └── README.md
│
└── examples/
```

LLM 主要负责：

* 理解题意
* 辅助生成测试
* 将诊断结果转换为自然语言提示

Rust 程序负责：

* 编译
* 运行
* 测试
* 错误解析
* 状态管理
* 知识点映射
* 提示级别控制

原则：

> **确定性逻辑由 Rust 完成，LLM 只处理需要语义理解的部分。**

---

## 9. 开发计划

### V1

```text
 1. 建立项目结构
 2. 设计核心数据结构
 3. 实现 CompilerTool
 4. 实现 Runner / TestRunner
 5. 解析常见 rustc 错误
 6. 实现错误分类与知识点映射
 7. 实现分层提示
 8. 模型配置模块（R3：profile / endpoint / key / 价格）
 9. 接入 LLM
10. Token 用量统计与预算控制（R6）
11. 加入自动测试生成
12. 完善 CLI 与诊断报告
13. 进度渲染与任务打断（R4：进度事件 + 协作式取消）
14. 会话历史与轨迹持久化（R5：保存 / 加载 / 回放）
```

V1 完成标准：

> 给定一道 Rust 编程题和学生代码，Agent 能够通过编译器和测试发现问题，判断主要错误类型及相关知识点，并逐级提供提示，而不是默认直接给出完整答案。

### V2

在 V1 基础上加入：

```text
历史诊断
→ 知识掌握模型
→ 薄弱点识别
```
