# 借鉴 ZCode 的功能补强（计划模式 + 工作区记忆面板）设计

- Date: 2026-08-30
- Status: accepted
- 关联计划: `docs/superpowers/plans/2026-08-30-zcode-parity.md`

## 背景与取舍

对照 ZCode（交互式编码代理）的能力清单与 TeemoCode 现状:

| ZCode 能力 | TeemoCode 现状 | 结论 |
|---|---|---|
| 技能/斜杠命令(SKILL.md) | 已有(技能库 + 云端斜杠面板) | 不重复建设 |
| 待办清单(TodoWrite) | 已有(侧栏待办 + 引擎 plan 帧) | 已覆盖 |
| 子代理 | 已有(Agent 工具 + 子代理 feed + 团队编排 v2) | 已覆盖 |
| 交互式提问(选项卡) | 已有(AskCard/reply-question) | 已覆盖 |
| 上下文压缩 | 已有(auto_compact_ratio + /compact) | 已覆盖 |
| **计划模式(Plan Mode)** | **缺失** | ✅ 本轮补(纯 UI) |
| **持久记忆系统(索引+管理)** | 引擎已按 `<workdir>/.monkeycode/MEMORY.md` 约定读写记忆,**UI 完全不可见可编辑** | ✅ 本轮补管理面板(小 Rust + UI) |
| 定时任务(Cron) | 缺失(需壳调度器,体量大) | 列为后续方向,本轮不做 |
| 会话交接/自动化 | 会话持久化+恢复已有 | 已覆盖 |

## Feature 1: 计划模式（纯 UI,与团队模式同构）

- composer 新增「计划」开关(团队开关旁,localStorage `mc.planMode.<sid>`,与 team mode 同款持久化)。
- 开启后发送注入 `[mc-plan]…[/mc-plan]` 前缀:本轮只读调研、产出实施计划(目标/分步/文件/风险/验证)、输出后停下等确认,禁止写文件与副作用命令。对应 ZCode 的 Plan Mode 语义,也呼应引擎已有的 plan 帧呈现(计划会以任务清单钉在 composer 上方)。
- **前缀剥离统一**:新增组合剥离 `stripInjectedPreambles`(循环剥离 `[mc-plan]` 与 `[mc-team]` 块直至稳定,任意顺序/叠加),`LogList` 用户气泡统一走它;`stripTeamPreamble` 保留为团队块的单一出处,旧 `[团队协调]` 兼容不变。
- 拼装顺序:`[mc-plan]` 在外层,其后 `[mc-team]`,最后用户原文;入队存最终文本(沿用团队 v2 的修复模式)。

## Feature 2: 工作区记忆面板

引擎约定每个工作区用 `.monkeycode/MEMORY.md` 记忆用户指令与项目知识(格式:用户指令条目/项目知识条目),但桌面端没有任何入口看到它。补一个「工作区记忆」管理面板:

- **Rust `src/memory.rs`**(壳原生服务,2 条命令):
  - `memory_read(workdir)` → 文件内容;不存在返回空串(前端显示空态)。
  - `memory_write(workdir, content)` → 落 `<workdir>/.monkeycode/MEMORY.md`,复用 `config::atomic_write_private`(0600 临时文件+原子替换),内容上限 256KB,目录自动创建。
  - workdir 信任模型与 uploads/git_push 一致(来自会话元数据,UI 侧工作区);文件名固定,无路径拼接注入面。
- **UI**:会话右上「…」菜单加「工作区记忆」项 → 弹窗(工作区路径 + 等宽编辑器 + 保存/关闭 + 引擎约定提示)。复用 ImportSkillsDialog 的弹窗骨架。
- **IPC**: `src/lib/ipc/memory.ts`(字面量 invoke + 浏览器降级);四处登记照 gateway 惯例,`check_command_contract.py` 守护。

## 验证

- Rust: memory round-trip/空态/自动建目录单测;`cargo test` 全量回归。
- UI: planMode.test.ts(生成/组合剥离任意顺序);memory ipc 测试;vitest 全套 + tsc + build。
