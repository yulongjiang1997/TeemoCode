# MonkeyCode 项目全景地图（二次开发参考）

> 面向二次开发的架构速查。生成于 2026-08-30，基于 `wip-local` 分支（桌面端版本 0.1.25 / TeemoCode）。
> 权威架构文档：`desktop/ARCHITECTURE.md`（桌面端契约）；本文是全局索引。

## 1. 这是什么

MonkeyCode 是长亭开源的**企业级 AI 开发平台**（AGPL-3.0）：Web 控制台编排云端开发环境（VM）里运行的 AI 编码任务；本地形态是 Tauri 桌面客户端（TeemoCode），在本机以子进程方式驱动 ohmyagent 引擎执行任务。本仓库 `wip-local` 分支在官方代码之上做了大量桌面端二开。

## 2. 模块地图

| 目录 | 技术栈 | 角色 |
|---|---|---|
| `backend/` | Go 1.25 + Echo(GoYoko/web) + samber/do DI + ent(PostgreSQL) + Redis + ClickHouse/Loki + S3 | 云端控制面：API/会话/任务编排/配置下发/流转发。**不跑容器、不跑 agent**，委派给外部 taskflow 服务 |
| `frontend/` | React 19 + Vite 7 + Tailwind 4 + shadcn 风格组件 + react-router 7 | Web 控制台。分 `online`（SaaS）与 `offline`（私有化）两种构建形态；任务流/终端走 WebSocket |
| `desktop/` | Tauri 2 + Rust 壳 + `ui-next/`(React 19 SPA) | **本地二开主战场**。壳管理窗口/托盘/桌宠/更新/引擎生命周期；`driver/` 经 stdio JSON-RPC 驱动 ohmyagent；`browser/` 桥接浏览器扩展；`baizhi/` 对接百智云/云端 |
| `agent/` | Rust（私有仓库 chaitin/OhMyAgent） | git submodule，**未初始化**（私仓拉不到）。本地 AI 引擎 sidecar，`--stdio` JSON-RPC |
| `plugins/` | Markdown 技能库 | submodule（MonkeyCodeOfficialPlugins，pc-client 分支），20 个内置技能随桌面包分发 |
| `browser-extension/` | MV3 扩展 | chrome.debugger 代理 + 标签页授权 UI；协议权威在 `desktop/src/browser/protocol.rs` |
| `mobile/` | Expo RN 0.83 / expo-router | 移动端，对接百智云端；`ota-server/` 自建 Expo Updates OTA 服务 |
| `docs/superpowers/` | — | 本地二开的需求文档工作区：`specs/<日期>-<主题>-design.md` 先行，`plans/<日期>-<主题>.md` 是带 checkbox 的执行计划 |

## 3. 云端任务执行链路（backend 视角）

```
前端(WS /api/v1/users/tasks/stream) ↔ backend ↔ taskflow(gRPC :50443) ↔ host 上 codingmatrix 创建 VM
                                                                        └ VM 内跑 Claude Code/Codex/OpenCode CLI
```

1. `biz/task/usecase/task.go Create()`：校验 → 渲染 `templates/claude|codex|opencode.tmpl` 成 VM 内 agent 配置 → 注入 skills/plugins（presigned URL，`biz/agentresource/resolver.go` 按 user>team>global 解析）→ `taskflow.VirtualMachiner().Create` 建 VM（vmID=`agent_<uuid>`）。
2. 整个 `CreateTaskReq` 序列化进 Redis `task:create_req:{taskID}`；`pkg/lifecycle/` 双状态机（task/vm）等 `/internal/vm-ready` 回调后再下发任务。
3. 执行期：WS 流转发（`pkg/taskflow/task_live.go` 连 taskflow），日志落 Loki/ClickHouse（`pkg/tasklog/gateway.go`），摘要走 Redis 延迟队列（`biz/task/service/tasksummary.go`）。
4. LLM 流量：VM 内 agent 回连 backend 的 `biz/llmproxy/`（`/v1/chat/completions|/responses|/messages`），用量记 ClickHouse。
5. 认证：Cookie + Redis Session（非 JWT），`middleware/auth.go`；Git 集成在 `biz/git/`（7 平台 PAT/OAuth + GitBot 代码审查 + 各平台 webhook）。
6. 扩展点：`domain/domain.go` 的 Hook 接口 + `bridge.go`（整个后端可作为库嵌入 SaaS 主程序，`backend.Register(echo, dir, WithXxxHook...)`）。
7. biz 模块分层：`handler/v1 → usecase(实现 domain 接口) → repo(ent)`；路由在各 handler 构造时自注册（`w.Group("/api/v1/...")`），无集中路由文件。ent schema 在 `backend/ent/schema/`（54 个实体），生成代码进 `backend/db/`（改 schema 后 `make generate`；注意 Makefile check-generate 写的 dbv2/ 是错的，以 `ent/entc.go` 为准）。配置全靠 `MCAI_` 前缀环境变量（viper）。

## 4. 桌面端架构（desktop，重点）

权威文档 `desktop/ARCHITECTURE.md` 定义 6 条契约：Frame 帧词汇、Caps 能力模型、IPC 规约、配置所有权、会话状态机、引擎生命周期。

```
ui-next/(React SPA, 产物 uidist/)  ←Tauri IPC→  src/(Rust 壳)  ←stdio JSON-RPC→  ohmyagent 子进程
                                                ├ driver/: frame/transport/session/normalize/fold/subagent
                                                ├ browser/: WS 桥(7440-7449) + 壳内 MCP server → 9 个 browser_* 工具给引擎
                                                ├ baizhi/: 百智云登录/模型网关同步/云端任务代理(mc_*/cloud_ws_*)
                                                └ config/skills/repo/uploads/wsl/telemetry/stats/git/native_pet
```

- **引擎协议**：`system/ready`（握手+capabilities）→ `session/create|sendMessage|destroy|exists|switchModel|switchMode|setThinking|compact`；下行 `event/stream`（sessionUpdate 家族）、`turn/stopped` 等。能力缺口自动回退（如无 switchMode → destroy+resume）。
- **Frame**（`driver/frame.rs` 唯一权威，ts-rs 生成 `ui/src/gen/`，同步到 `ui-next/src/gen/`，`genSync.test.ts` 钉字节一致）：`task-started/ended/error`、`user-input`、`permission-req/resolved`、`reply-question`、`task-running(kind=acp_event→sessionUpdate)`。改词汇 = 改 frame.rs → 重新生成 gen/ → 同步 types.ts/reduce.ts → 补 reduce.test.ts，同一 PR。
- **会话状态机**：`created → running → idle|finished|interrupted|error`；"和解原则：引擎应答是确认，不是前提"——壳侧看门狗/冷修复（`replay_open`）保证崩溃后不卡"执行中"。
- **配置**：`DesktopConfig`(config.json) 唯一权威，`ConfigStore` 串行原子写+bak；引擎配置是单向物化到 `app_config_dir/ohmyagent/{settings,mcp}.json`（OHMYAGENT_CONFIG_DIR 注入）。模型按别名物化恒写 8 键。技能走**按会话物化**（每次 session/create 前整删整建到 `<engine_dir>/sessions/<engine_id>/skills/`）。
- **数据归属**：会话索引/帧日志在壳 sidecar `ohmy-sessions/<sid>/`（meta.json + replay.jsonl + events.jsonl 大字段截断回读 `session_frame`）；引擎上下文在 `ohmyagent/sessions/<engine_id>/messages.jsonl`；附件在 `<workdir>/.monkeycode/uploads`；用户技能在 `app_config_dir/skills/<name>/SKILL.md`。
- **IPC 规约**：命令 `domain_verb`，新命令**三处登记**（main.rs invoke_handler、build.rs、tauri.conf.json capability）+ UI 侧封装，`scripts/check_command_contract.py` CI 强制；事件 `channel:{id}`；全局事件 6 个；"监听先于命令"。`tauri.debug.conf.json` 的 capability 也要同步维护。
- **WSL**：`kernel_env = wsl:<发行版>` 时引擎 spawn 进 WSL，引擎数据留宿主侧（wslpath 注入），壳摸 guest 文件统一走 `\\wsl$`；浏览器桥要求 networking-mode=mirrored。
- **ui vs ui-next**：两套 SPA，产物都落 `uidist/`，**打包用 ui-next**（tauri.conf.json beforeBuildCommand）；gen/ 两边同步。文档里 `cd ui && npm run build` 是旧说法，实际 `cd desktop/ui-next && npm run build`。

## 5. 本仓库（wip-local）二开现状

- **已完成**（近期提交主题）：技能导入 git 仓库 + LLM 并行分析（`src/git.rs` skills_import_git/skill_analyze + `ui-next/src/features/settings/ImportSkillsDialog.tsx`）；team mode 编排前缀（纯 UI，`ui-next/src/lib/ipc/teamPreamble.ts`）；发送队列（`ui-next/src/features/chat/composer/QueueModal.tsx`）；桌宠 GIF（`src/native_pet.rs` + `pet.html`）；用量统计（`src/stats.rs`）；发布流水线（`scripts/release_v0_1_x.py` + `RELEASE.md`，更新源 Gitee teemo-code-update，Ed25519 签名必须在 cmd.exe 内联）；**模型网关**（2026-08-30：统一大模型调度平台，OpenAI 兼容端点 + 模型组权重调度/故障切换/熔断/组级共享上下文——`desktop/src/gateway/{mod,sched,upstream,server}.rs` + 设置页「模型网关」分区，spec/plan 见 `docs/superpowers/{specs,plans}/2026-08-30-model-gateway*`，契约详见 `desktop/ARCHITECTURE.md` 模型网关节）。
- **设计完成、代码零行**：多引擎（ohmyagent + codex app-server 并存）。三份文档齐备：`docs/superpowers/plans/2026-08-21-multi-engine-ohmy-codex.md`（Phase 0-5）、`...-AGENT-BRIEF.md`（接口对照表）、`specs/2026-08-21-multi-engine-ohmy-codex-design.md`。第一步是 Phase 0 抓包 Spike。
- **存量修复（2026-08-30）**：`update_download`（漏登记 build.rs，UI 点「更新」报 Command not found）、`import_sound`（一次 stash 丢失，音效自定义文件按钮失效）、`import_mc_scan/scan_dir/apply`（侧栏"导入原版任务"三命令漏注册）已全部恢复登记，`check_command_contract.py` 由红转绿。
- **已知存量测试失败（非网关引入，涉及用户在改的聊天区，未动）**：`ui-next` 的 `layoutContract.test.ts`（CommandWarehouse/ImportSkillsDialog 的 overflow-y 组合）与 `CloudTaskView.test.tsx`（提问大纲"显示更早"）在 `wip-local` 上本就红；工作区另有 `QueueModal.tsx` 处于未暂存删除态。
- **本地跑桌面端**：引擎二进制已在 `desktop/binaries/`（ohmyagent.exe 等），不初始化 submodule 也能跑/打包（打包 check-ohmy-src 才强制要源码）。`npx tauri dev --config tauri.dev-next.conf.json`（1421 HMR）；`cargo test` hermetic，E2E `MC_OHMYAGENT_BIN=<绝对路径> cargo test e2e_ -- --ignored --test-threads=1`。
- **杂项垃圾**：根目录 `agent_err.txt`（克隆失败日志）、`run.log`（旧 Electron 壳日志）、`nul`（误建文件）、空的根 `node_modules/` 与 `debug-data/`，可清理。`.ohmyagent/settings.json` 是引擎运行产物（权限规则）。`.monkeycode/MEMORY.md` 记录了 online 预览构建/验证码验收流程。

## 6. 二开约定速查

| 事项 | 约定 |
|---|---|
| 需求流程 | 先写 `docs/superpowers/specs/<日期>-<主题>-design.md`，再写 `plans/<日期>-<主题>.md`（TDD checkbox） |
| 后端加接口 | biz 模块内 handler→usecase→repo 分层 + domain/ DTO；`make swag` 重新生成 swagger → 前端 `pnpm api` 重新生成 `src/api/Api.ts`（勿手改） |
| 前端加页面 | `src/App.tsx` 注册路由；文案 cn/en 双份（i18n 完整性测试会拦）；请求走 `apiRequest()`；online/offline 差异用 `IS_OFFLINE_EDITION` 分支 |
| 桌面端加 IPC 命令 | main.rs + build.rs + tauri.conf.json（+tauri.debug.conf.json）capability + ui-next/src/lib/ipc/ 封装，check_command_contract.py 守护 |
| 桌面端改帧词汇 | 只改 `driver/frame.rs` → ts-rs 重新生成 gen/ → 同步 ui-next types/reduce + 测试 |
| UI 颜色 | 一律主题 token（check_theme_tokens.py 拦截写死颜色） |
| 版本发布 | Cargo.toml + tauri.conf.json 两处版本（release_v0_1_x.py 自动）；打包配置只经 --config 传 bundle.*.conf.json（禁 tauri.<平台>.conf.json 命名） |
| 内置技能 | `plugins/skills/`（submodule），官方默认启用集在 `desktop/src/skills.rs DEFAULT_ENABLED`；SKILL.md frontmatter：name/description(+可选 arguments) |
