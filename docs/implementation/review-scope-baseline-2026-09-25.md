# 审查范围旧设计归档

2026-09-25 整理时保留的旧正文，仅作设计演进记录；不是当前范围。当前仅有代码审查、安全检查、性能分析三个入口；通用委派、盲评与候选比较另行规划。状态见 [当前工作清单](../CURRENT_WORK.md)。

## 原需求正文

### 11.3 子任务委派：Delegation

`@Agent` 或界面创建；独立 Session，不改变主任务当前 agent。默认只读（调研/分析/测试/审查）。**写入型委派遵循统一隔离规则**：Git 目录→独立 worktree；非 Git→自动复制临时目录；未指定工作目录不得执行写任务。结果以摘要/引用/附件/Diff/测试报告回流；主 agent 不自动采纳写入结果；支持独立取消/重试/换 agent。基础只读版随 M1 交付，写入版 P2。

### 11.4 指定评审：Review

**评审对象**：当前 Turn 回复、当前 diff、指定文件/文档、某个 Candidate、自指定基线以来全部修改。

**Review Package**：原始需求与验收标准、作者 agent 与模型、评审范围与排除项、相关文档/代码、git diff 与变更文件、已执行测试及结果、作者实现摘要、已知限制与待办。

**执行规则**：评审默认只读意图，禁止修改被评审的主工作区；代码评审固定到 Checkpoint / Candidate Result Commit / 版本化 Artifact（防评审期间漂移）。**安全性两个独立维度并在 UI 标明**：Agent Write Policy（NativeReadOnly / PromptConstrained / Unrestricted）× Workspace Isolation（ReadOnlySnapshot / DisposableWorktree / DisposableDirectory / MainWorkspace）。临时 worktree 只保证主工作区不被污染，不得表述为严格只读；期间产生的文件修改默认丢弃。文档评审针对版本化 Artifact。评审标准可选：通用/正确性/安全/性能/测试/架构/自定义；可选隐藏作者身份（盲评）。

**结构化输出（版本化）**：结论（通过/有条件通过/需要修改/不建议采纳）+ 问题列表（严重度、标题、依据、文件行号、建议）+ 优点 + 风险遗漏 + 建议补充测试。严重度命名体系见开放问题。

**闭环**：全部或选中意见可发回作者；作者修复产生新版本；同一评审者可复审；每轮评审与修复形成版本链不覆盖旧意见；评审意见是否正确由用户最终判断。


## 原设计正文

### 12.2 Delegation

- 子任务拥有独立 Session 和 ExecutionRun，不改变主 Task `preferred_executor`，也不取代 `active_primary_run`。
- M1 只读 Delegation 可使用 MainWorkspace，但必须优先使用 Agent NativeReadOnly；只有 PromptConstrained 时 UI 显示风险。
- 写入型 Delegation（M3）复用 Candidate Workspace，不直接写主目录。
- 结果回流为结构化 DelegationResult：结论、证据、Artifact、Diff/Test、未解决问题。
- 主 Agent收到的是用户选择后的结果引用；Hub 不自动把子任务输出当成事实或自动采纳代码。

### 12.3 Review

Review 必须固定对象版本，不能评审“正在变化的当前目录”。

```text
ReviewProfile
├── axes[]
│   ├── requirements_correctness
│   ├── engineering_quality
│   ├── architecture
│   ├── security
│   ├── performance
│   ├── tests
│   └── custom
├── severity_scheme
├── blind_author
├── scope / exclusions
└── output_schema_version
```

默认 Profile 分开呈现两条轴：

- **需求符合性**：缺失、部分实现、范围外行为、实现错误；每项引用需求证据。
- **工程质量**：项目规范、结构、错误处理、测试和可维护性；每项引用 Diff/文件证据。

两条轴可以交给同一 Reviewer，也可以并行交给不同 Reviewer；结果不自动合并评分，避免一条轴掩盖另一条。

首版结构化报告：

```text
ReviewReport v1
├── verdict: pass | conditional | changes_required | reject
├── findings[]
│   ├── severity: critical | high | medium | low | note
│   ├── axis
│   ├── title
│   ├── evidence_refs[]
│   ├── location?
│   └── recommendation
├── strengths[]
├── missing_tests[]
├── open_questions[]
└── reviewer_provenance
```

设计默认采用 `Critical/High/Medium/Low/Note`，UI 可显示 P0–P4 别名；这解决需求 Q9 而不把内部存储绑定某一种文案。

