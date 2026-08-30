# 子代理团队功能 v2 设计（团队编排优化）

- Date: 2026-08-30
- Status: accepted
- 关联计划: `docs/superpowers/plans/2026-08-30-team-v2.md`

## 背景

现有团队功能是**纯 UI 侧的 prompt 契约**（无引擎改动，符合桌面端架构铁律"UI 不建立网络连接/引擎不感知 UI"的分工）：设置页配置角色（名称+职责+技能绑定），发送任务时 `teamPreamble.ts` 生成编排指令前缀注入用户消息，主模型作为协调者用引擎的 **Agent 工具**（入参 `description/prompt/name`）分派子代理，UI 已有子代理实时 feed（subagent_tool/subagent_text/child_session）与后台完成通知（task_notification）。

## 现状问题（本次必修）

1. **队列消息丢失团队前缀**：`useComposer.send()` 只在直发路径拼 `finalText`，忙时入队的 `QueueItem.text` 不带前缀，队列补投（flush effect）原样发送——团队模式对排队指令静默失效。
2. **剥离器与生成器格式不匹配**：`teamPreamble.ts` 生成 `[团队协调] …` 开头（无结束标记），`LogList.stripTeamPreamble` 剥的是 `[mc-team]…[/mc-team]` ——正则永不匹配，**用户气泡会外显整段编排指令**。
3. 编排指令过于简陋（只有一行"拆解并分派"），没有验收、上下文传递、冲突避免等关键约定。

## 参考的开源模式 → 落地映射

| 模式 | 来源 | 落地 |
|---|---|---|
| 委派决策靠 name+description 注入协调者系统提示 | Claude Code subagents（VoltAgent/awesome-claude-code-subagents、Anthropic 官方课程） | 每个角色：名称 + 职责一句话；preamble 让协调者"按职责匹配成员" |
| role / goal / backstory 三件套 | CrewAI（Crafting Effective Agents） | 角色卡 = 名称(role) + 职责(goal) + 技能绑定；backstory 不单设字段，并入职责文本 |
| hierarchical manager：委派**且验收**，manager 失败模式分析 | CrewAI Hierarchical Process（含 TowardsDataScience 失败模式分析） | 编排契约强制"先验收后汇总"；完成标准里写明验收动作 |
| 子代理看不到父会话上下文，会"盲飞" | Claude Code 实践共识（HN 讨论/官方课程） | 委派模板强制 prompt 自带完整上下文（目标/工作区/相关文件/约束/验收标准） |
| SOP 流水线（产品→架构→工程→QA） | MetaGPT | 可选**工作流步骤**：阶段名 + 参与角色，按序推进 |
| speaker 选择 / 并行 / 防冲突 | AutoGen（Selector Group Chat、round_robin） | 编排契约：独立任务并行、有依赖串行；成员只改各自负责的文件 |
| 终止条件 / 完成判定 | AutoGen termination conditions | 明确完成标准 + 汇总格式（做了什么/谁做的/关键文件/遗留风险） |

## 设计

### 1. 消息格式统一（修 bug 1/2）

- 生成器与剥离器统一用 `[mc-team]…[/mc-team]` 标签（剥离正则已按此写好）；strip 函数**搬到 `teamPreamble.ts`**（单一出处）并导出，`LogList` 引用。
- 兼容历史消息：strip 同时识别旧格式（`[团队协调]` 开头到行尾块）。

### 2. 编排契约 v2（teamPreamble 重写）

结构（无工作流时省略对应段；无角色返回空串）：

```
[mc-team]
你是本会话的任务协调者。你的职责是拆解、分派、验收、汇总,不是独自完成全部工作。

## 团队成员
- <名> — <职责一句话>
  技能: a, b（分派给它时优先按这些技能执行）

## 工作流(按序推进,每阶段完成并验收后再进入下一阶段)
1. <阶段名>: <角色A、角色B>

## 协作规则
- 用 Agent 工具分派成员;每次委派的 prompt 必须自带完整上下文:任务目标、
  工作区路径、相关文件、已知约束、验收标准——子代理看不到我们的对话
- 相互独立的任务并行执行,有依赖的串行;每个成员只改自己负责的部分,避免冲突
- 子代理返回后先对照验收标准验收,不合格明确指出问题打回重做
[/mc-team]
```

约定：角色 ≤ 8（超出截断并在尾部提示"仅列前 8 名"）；职责为空的角色显示"（职责待定，谨慎分派核心任务）"。

### 3. 数据模型（localStorage，向后兼容）

```ts
interface TeamWorkflowStep { id: string; title: string; roleIds: string[] }
```

- `prefs.ts` 增加 `readTeamWorkflow/writeTeamWorkflow`（key `mc.teamWorkflow`），读取时过滤已删角色的 roleId（悬空引用即弃）。
- `TeamRole` 字段不变（name/skill/skills）——旧数据零迁移。

### 4. 内置团队预设（发散功能，参考 MetaGPT/CrewAI 经典班组）

TS 常量内置 4 套：**全栈交付小队**（策划/前端/后端/测试 + 3 阶段工作流）、**严格评审小队**（实现/评审/安全审查）、**文档工程小队**（整理/撰写/校对）、**精简单兵**（通用执行者，无工作流）。预设应用 = 覆盖角色+工作流（confirm 确认）。

### 5. 导入/导出

团队配置（角色+工作流）序列化为 JSON 文本：**导出=写剪贴板**、**导入=粘贴 textarea**（零新 IPC，桌面/浏览器双态可用）；导入时校验结构、过滤非法字段。

### 6. useComposer 修复（bug 1）

入队 `QueueItem.text` 改存 `finalText`（含前缀）：补投、留档恢复、日志渲染天然一致；直发路径不变。`LogList` 渲染统一走 strip（本地+云端用户气泡同源）。

### 7. TeamSection 编辑器增强

- 工作流编辑块：步骤（标题 + 角色多选 chips）增删/上下移。
- 预设库下拉（应用需 confirm）、导入/导出按钮、角色数提示（>8 警示）。

## 明确不做

- 子代理按角色绑定不同模型：引擎统一父模型（协议如此），做不了。
- 运行时团队看板：子代理实时 feed 已有呈现，重复建设。
- 服务端团队共享：本地功能，localStorage 即与既有偏好一致。

## 验证

- `teamPreamble.test.ts`：v2 契约结构、技能交集、空角色/空工作流退化、上限截断、strip 新旧格式 round-trip。
- `prefs` 团队数据 round-trip + 悬空 roleId 过滤。
- 全套 vitest + tsc + build 回归。
