# AGENT 开发指令：MonkeyCode 多引擎并存集成（OhMyAgent + Codex）

> **本文件是给编码 agent 的自包含任务书。** 读完本文件 + 打开本仓库
> （`MonkeyCode`）与参考仓库（`openai/codex`，下称 codex 仓库）即可开工，
> **无需任何额外对话上下文**。
>
> 配套参考（本仓库内，非必需，供查证）：`docs/superpowers/specs/2026-08-21-multi-engine-ohmy-codex-design.md`、`docs/superpowers/plans/2026-08-21-multi-engine-ohmy-codex.md`。

---

## 1. 你要交付什么

在 MonkeyCode 桌面端（`desktop/`，Rust + Tauri）新增一个 **Codex 引擎适配器**，
使桌面端同时支持 **OhMyAgent**（现状唯一引擎）与 **Codex** 两种引擎**并存可选**。
两个引擎都是常驻 JSON-RPC 子进程；每个任务绑定单一引擎、**无中途切换**，
因此任务数据（代码 + 对话 + 元数据）**100% 不丢**。

工作拆分（按依赖顺序）：
1. 抽象出统一引擎接口 `EngineDriver`，OhMyAgent 包成 `OhmyEngine`（行为不变）。
2. 新增 `CodexEngine`，驱动 `codex app-server` 二进制。
3. 引擎选择与路由：新建任务选引擎，`meta.json` 记录绑定。
4. Codex 二进制分发与降级。
5. 测试与合规。

**最重要的约束：`desktop/src/driver/frame.rs`（Frame 词汇）与 UI 归约层不得改动**
（除非确需新增 Frame 变体，见 §9）。所有引擎差异都在 driver 层消化。

---

## 2. 你需要的东西

| 东西 | 位置/状态 |
|---|---|
| MonkeyCode 仓库 | 本工作目录 `MonkeyCode`（`desktop/` 是改动区） |
| codex 参考仓库 | `D:\works\ziji\codex\codex`（只读参考其接口，**不修改**） |
| **codex 二进制（运行依赖）** | **见 §11：默认不随本任务构建，需自行准备/放置** |

---

## 3. 背景：Codex 的正确接入形态（重要，勿走错）

Codex 仓库提供**两种对外形态**，本任务用的是第二种：

1. ❌ `codex exec --experimental-json` —— **一次性**编排：spawn 一个进程、喂一个
   prompt、读 JSONL 到进程退出。**这不是引擎集成形态，不要用。**
2. ✅ **`codex app-server --listen stdio://`** —— **常驻** JSON-RPC 引擎服务，
   与 OhMyAgent 的 `--stdio` **完全同构**。这是正确接入形态。

证据：`codex/codex-rs/app-server/`（入口 `app-server/src/main.rs`，默认
`DEFAULT_LISTEN_URL` 为 stdio）、`app-server-transport/src/transport/stdio.rs`
（行分隔 JSON-RPC）、`app-server/src/request_processors/{thread,turn}_processor.rs`。

### OhMyAgent ↔ Codex app-server 接口对照（这是翻译层的依据）

| OhMyAgent（MonkeyCode 现状） | Codex app-server |
|---|---|
| `ohmyagent --stdio` 常驻 | `codex app-server --listen stdio://` |
| 行分隔 JSON-RPC | 行分隔 JSON-RPC |
| `system/ready` 握手 | `initialize` → `InitializeResponse` |
| `session/create` | `thread/start`（新建）/ `thread/resume`（恢复，带 thread id） |
| `session/send` | `turn/run{threadId, turnId, input}` |
| `session/destroy` | thread 生命周期管理 |
| `session/switchModel` | `ThreadResumeParams.model` / `model_provider` |
| 实时审批 `respond` | `apply_patch_approval` / `exec_command_approval` |
| 中断 | `turn_interrupt`（`Op::Interrupt`） |
| 事件 | `thread_started` / `turn_completed` / `item_added` 等 |

**注意**：上面是依据 codex 源码推断的契约，**尚未实测抓包**。§7 Phase 0 要求你先
实跑抓 JSON-RPC 报文、据此校正映射表，再写适配器。

---

## 4. 目标架构

```
desktop/src/driver/
├── engine.rs              (新) trait EngineDriver —— 统一接口
├── ohmy.rs                (重构) struct OhmyEngine : EngineDriver（行为零变化）
├── codex.rs               (新) struct CodexEngine : EngineDriver
├── transport/
│   ├── jsonrpc.rs         (新/拆分) 通用行分隔 JSON-RPC 生命周期
│   ├── ohmy_protocol.rs   (原 transport 的 OhMy 协议部分)
│   └── codex_protocol.rs  (新) Codex RPC 编解码 + 事件→Frame 翻译
└── frame.rs               (原则上不改；确需时按 §9 增补变体)
```

`EngineDriver` trait 草案（在 `engine.rs`）：

```rust
pub trait EngineDriver: Send + Sync {
    fn capabilities(&self) -> HashSet<String>;
    fn create_session(&self, opts: SessionOpts) -> Result<String, String>; // 返回引擎侧 id
    fn send(&self, id: &str, input: SessionInput) -> Result<(), String>;
    fn destroy_session(&self, id: &str) -> Result<(), String>;
    fn event_stream(&self, id: &str) -> Receiver<Frame>;
    fn switch_model(&self, id: &str, model: &str) -> Result<(), String>;
    fn approve(&self, id: &str, decision: ApprovalDecision) -> Result<(), String>;
    fn interrupt(&self, id: &str) -> Result<(), String>;
}
```
（签名可按现有 `ohmy.rs` / `session.rs` 实际调用方微调，**但必须满足：UI 归约层
只见 `Frame`，不见引擎差异。**）

---

## 5. 关键目录与数据布局（沿用现状，不另造树）

所有路径在 Tauri `app_config_dir()`（Windows 为
`C:\Users\<用户>\AppData\Roaming\com.teemocode.desktop\`）。

| 用途 | 路径 | 写入方 |
|---|---|---|
| OhMy 引擎配置 | `.../ohmyagent/` | 壳 `write_ohmyagent_config`，`config.rs:442` |
| OhMy 会话缓存 | `.../ohmyagent/sessions/<sid>/messages.jsonl` | OhMy 引擎 |
| driver 回放 + 权威索引 | `.../ohmy-sessions/<sid>/{replay.jsonl,meta.json}` | 壳 driver |
| 对话工作区 | `app_local_data_dir/chat-workspaces/chat-xxx/` | 引擎按 cwd 写 |
| **Codex 引擎目录（新增）** | `.../codex/`（设 `CODEX_HOME`） | 壳 bootstrap |
| **Codex 会话缓存（新增）** | `.../codex/sessions/<threadId>/rollout-*.jsonl` | Codex app-server |

**核心不变量**：同一 `sid` 同时拼进 `ohmy-sessions/<sid>/`（driver 侧）与引擎侧会话
目录；Codex 的 threadId 由它自身生成，需在 `meta.json` 维护 `shell_sid ↔
codex_thread_id` 映射。**路径拼接必须走受控函数并遵守 `valid_session_id` 守卫**
（见 `session.rs:130`，未校验 id 会被 `remove_dir_all` 向上清空主目录——高危）。

**会话缓存与代码工作目录分离**：对话历史不进项目目录；代码改动落在任务 cwd。

### `meta.json` 扩展（引擎路由权威索引）

```json
{
  "sid": "06b3e207",
  "engine": "codex",
  "codex_thread_id": "thr_xxx-uuid",
  "ohmy_session_id": "06b3e207",
  "cwd": "D:\\works\\SomeProject"
}
```

---

## 6. 实现目标（任务绑定语义，务必遵守）

- **每个任务绑定单一引擎**，`meta.json.engine` 决定路由，任务生命周期内不可改。
- **无中途切换** → 不需要 Handoff 摘要、不需要虚拟常驻伪装。
- **并存 ≠ 并行**：同一时刻仅一个引擎 active 操作其绑定任务的 cwd，避免并发写冲突。
- 数据不丢由"单一归属"保证：对话历史自始至终写进绑定引擎的私有会话，从不迁移。

---

## 7. 分阶段实施（按序执行）

### Phase 0 — Spike：抓 Codex app-server 真实 RPC（必须先做）
1. 获取 codex 二进制（见 §11）。
2. 启动 `codex app-server --listen stdio://`，用脚本/手动逐条发送并抓包：
   `initialize` → `thread/start` → `turn/run` → 收事件；再测 `apply_patch_approval`、
   `exec_command_approval`、`turn_interrupt`。
3. 确认帧格式（裸 JSON-RPC 还是 LSP 风格）、认证注入（`CODEX_API_KEY` env 或
   `--config openai_base_url=`）、`CODEX_HOME` 行为。
4. **确认 `--session-source` 取值**：默认 `vscode` 含 IDE 专属逻辑
   （`code_mode_host` / `attestation`），选非 IDE 模式或确认无副作用。
5. 产出 `docs/specs/codex-app-server-protocol.md`：真实报文 + **Codex 事件 → Frame
   映射表** + 参数清单 + session-source 结论。**以此校正 §3 推断的契约。**

### Phase 1 — 抽象层
1. 新增 `desktop/src/driver/engine.rs`，定义 `EngineDriver`（§4）。
2. 重构 `desktop/src/driver/ohmy.rs` → `OhmyEngine`，**行为零变化**，仅结构迁移；
   `ohmy_tests.rs` 必须全绿。
3. 拆分 `transport.rs` → `transport/jsonrpc.rs`（通用生命周期）+ 协议适配层。
4. `Inner`（`ohmy.rs` 内）新增 `codex_data_dir` / `codex_engine_dir` 字段；
   `ShellCtx` 新增 `codex_config_dir()`。

### Phase 2 — Codex 适配器
1. 新增 `desktop/src/driver/codex.rs` → `CodexEngine`：启动 `codex app-server
   --listen stdio:// --session-source <结论> --config openai_base_url=...`，env 注入
   `CODEX_API_KEY`、`CODEX_HOME`。
2. 各方法按 Phase 0 抓到的 RPC 实现；`event_stream` 把 Codex 事件翻译为 Frame。
3. 维护 `shell_sid ↔ codex_thread_id` 映射，路径拼接受控 + `valid_session_id` 守卫。

### Phase 3 — 引擎选择与路由
1. 桌面设置加全局默认 `engine: "ohmy" | "codex"`。
2. `session/create` 入参加 `engine`（默认取全局，新建可覆盖，选定即绑定）。
3. 扩展 `meta.json` 字段（§5）；按 `meta.json.engine` 路由创建/恢复会话。

### Phase 4 — 二进制分发与降级
1. bootstrap 增加 codex 二进制获取：版本锁定 / 下载 / sha 校验，落
   `com.teemocode.desktop/codex/bin/`（类比 `ohmyagent/bin/`）。
2. 启动自检：codex 缺失/版本不符 → UI 禁用该引擎并提示，不影响 OhMyAgent。

### Phase 5 — 合规与测试
1. `NOTICE` / 关于页追加 Codex（Apache-2.0）许可证与版权声明；仅进程调用，不链接。
2. 单测：Frame 翻译逻辑、`EngineDriver` mock。
3. 集成测：`CodexEngine` 完成最小任务（initialize → thread/start → turn/run →
   收 Frame → 产出文件），与 `OhmyEngine` 对照。
4. 端到端：OhMyAgent 与 Codex 各建一个任务跑需求，UI 归约层无改动即可工作。
5. 路径守卫回归：Codex threadId 同样过 `valid_session_id`。

---

## 8. 需要参考的源码位置（读它们，别猜）

- MonkeyCode：`desktop/src/driver/ohmy.rs`、`transport.rs`、`session.rs`、`frame.rs`、
  `config.rs`、`mod.rs`（注意 `session.rs:119-124` 的 sid 双写、`session.rs:130` 守卫、
  `config.rs:442` 引擎目录注入）。
- codex 仓库（只读）：`codex-rs/app-server/src/main.rs`、
  `app-server-transport/src/transport/stdio.rs`、
  `app-server/src/request_processors/{thread,turn}_processor.rs`、
  `app-server-protocol/src/protocol/v1.rs`、`sdk/typescript/src/exec.ts`
  （看它怎么注入认证/CODEX_HOME/session-source）。

---

## 9. 允许改动的边界

- **改**：`desktop/src/driver/`（新增 engine.rs、codex.rs、codex_protocol.rs、
  transport/jsonrpc.rs；重构 ohmy.rs、transport.rs）、`config.rs`（新增 codex 目录）、
  `meta.json` 读写逻辑、bootstrap/doctor、`NOTICE`。
- **尽量不改**：`desktop/src/driver/frame.rs`。若 Codex 事件确无对应 Frame 变体
  （如 reasoning token、outputSchema 结果），可**新增变体**，但不得改既有变体语义。
- **绝不改**：UI 归约层（消费 Frame 的地方）、后端 Go 层（首版后端无改动）。

---

## 10. 验收标准（全部通过才算完成）

1. 默认引擎 OhMyAgent 时，行为与现状完全一致（回归测试通过）。
2. 新建任务选 Codex 后，能 spawn `codex app-server`、走 JSON-RPC、流式收 Frame、
   产出代码文件；实时审批/中断 UI 工作（非预授权降级）。
3. **数据不丢**：Codex 任务的文件产物在任务 cwd、对话在 `codex/sessions/<threadId>`，
   单一归属、不迁移；关闭重开后按 `meta.json` 恢复完整上下文。OhMy 任务同理。
4. codex 二进制缺失时引擎选项灰态禁用并提示，不影响 OhMyAgent。
5. 不引入 AGPL/Apache 链接合规风险（仅进程调用）。
6. 无 `remove_dir_all` 越界风险（所有 id 过 `valid_session_id`）。

---

## 11. Codex 二进制的获取（运行依赖，已就绪）

**本机已自建并部署** Codex 二进制（从 openai/codex 源码构建，GNU triple）：

- 位置：`C:\Users\12090\AppData\Roaming\com.teemocode.desktop\codex\bin\codex.exe`
- 版本：`codex-cli 0.0.0`（debug profile，1305 MB，仅供开发/集成测试）
- SHA-256：`8972dd34b2985da8534aedacfcc61a44170084c7d44402edd2a3af7aeba1bb06`
- 构建全过程：见 `plans/2026-08-21-codex-binary-build-record.md`

**开发 agent 可直接使用该二进制完成 Phase 0 与集成测试**，无需再构建。已验证：
`codex --version`、`codex app-server --listen stdio://` 可启动、`codex exec --json`
存在。注意 app-server 前须创建 `CODEX_HOME` 指向的目录。

如需复现/重构建（换 release、换平台），按构建记录操作。

---

## 12. 已知风险（开发时留意，见 spec 完整版）

| 编号 | 风险 | 缓解 |
|---|---|---|
| R-1 | Codex RPC 契约未实测，schema 可能偏差 | Phase 0 抓包校正 |
| R-2 | `--session-source` 默认 vscode 含 IDE 逻辑 | Spike 确认取值 |
| R-3 | AGPL 分发 Apache-2.0 二进制 | 保留 NOTICE，仅进程调用 |
| R-4 | Codex 自带沙箱与桌面沙箱冲突 | 对齐权限模式，限定 cwd |
| R-5 | 两套私有 RPC 字段级不兼容 | CodexEngine 做翻译层（确定工作量） |
| D-1 | 未校验 id 致 `remove_dir_all` 越界 | 全部走 `valid_session_id` |
| D-2 | Windows `~` 展开错位 | 继承壳已算好的 cwd，不复算 |
