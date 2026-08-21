# 多引擎可切换设计（OhMyAgent / Codex）

## 背景

MonkeyCode 桌面端（Tauri + Rust）当前仅驱动一种智能体内核 **OhMyAgent**：
通过 `desktop/src/driver/ohmy.rs` spawn 子进程，用私有 stdio JSON-RPC 通信
（`system/ready` → `session/create|send|destroy|switchModel|switchMode`），
事件归一化为 `desktop/src/driver/frame.rs` 的 Frame 词汇交给 UI 归约层。

`openai/codex` 仓库提供**两种对外形态**，区分极为关键：

1. **`codex exec --experimental-json`**（一次性快捷通道）：父进程 spawn 一个
   `codex` 进程，把 prompt 写进 stdin 后关闭，读 stdout 的 JSONL 直到进程退出。
   TS SDK（`sdk/typescript/src/exec.ts`）走的就是这条路。**这是给脚本/CI 用的，
   不是引擎集成形态。**
2. **`codex app-server --listen stdio://`**（常驻引擎服务）：与 OhMyAgent 的
   `--stdio` 完全同范式——常驻进程、行分隔 JSON-RPC、基于 thread/turn 的会话模型、
   支持实时中断与审批。**这才是接入 MonkeyCode 作为可切换引擎的正确形态。**

经实测（`codex-rs/app-server`），`app-server` 的接口与 OhMyAgent 几乎一一对应：

| OhMyAgent（MonkeyCode） | Codex app-server（实测证据） |
|--------------------------|------------------------------|
| `ohmyagent --stdio` 常驻 | `codex app-server --listen stdio://`（`cli/src/main.rs` 默认 `DEFAULT_LISTEN_URL`） |
| 行分隔 JSON-RPC | 行分隔 JSON-RPC（`app-server-transport/src/transport/stdio.rs`） |
| `system/ready` 握手 | `initialize` / `InitializeResponse`（`app-server-protocol/src/protocol/v1.rs`） |
| `session/create` / `session/send` | `thread/start`（或 `thread/resume`）+ `turn/run`（见 `thread_processor.rs` / `turn_processor.rs`） |
| `session/destroy` | thread 生命周期管理（`thread_lifecycle.rs`） |
| `session/switchModel` | `ThreadResumeParams.model` / `model_provider` 校验切换 |
| 实时审批 `respond` | `ApplyPatchApproval` / `ExecCommandApproval` / `InterruptConversation`（`v1.rs`） |
| 中断 | `turn_processor.turn_interrupt`（`Op::Interrupt`） |

本设计的目标：在 **不改动 UI 归约层** 的前提下，让桌面端支持 **OhMyAgent 与
Codex（app-server）二进制引擎自由切换**，二者均为常驻 JSON-RPC 引擎。

### 真实的目录布局（源码权威，来自本仓库）

所有路径位于 Tauri `app_config_dir()`，Windows 上为
`C:\Users\<用户>\AppData\Roaming\com.teemocode.desktop\`。

| 用途 | 真实路径 | 写入方 | 源码证据 |
|------|----------|--------|----------|
| 引擎私有配置 | `com.teemocode.desktop/ohmyagent/`（settings.json / mcp.json / skills/） | 壳 `write_ohmyagent_config` | `config.rs:442-445`，经 `OHMYAGENT_CONFIG_DIR` 注入引擎 |
| 引擎会话缓存 | `com.teemocode.desktop/ohmyagent/sessions/<sid>/messages.jsonl` | OhMyAgent 引擎 | `session.rs:120`：`<engine_dir>/sessions/<id>/` |
| driver 自记帧日志（回放） | `com.teemocode.desktop/ohmy-sessions/<sid>/replay.jsonl` + `meta.json` | 桌面壳 driver | `ohmy.rs:5-6`、`session.rs:120` |
| 普通对话工作区 | `com.teemocode.desktop/../Local/.../chat-workspaces/chat-xxxx/`（`app_local_data_dir`） | 引擎在 cwd 下写 | `session.rs:280` |
| 审批记忆（壳侧兼容尾巴） | `com.teemocode.desktop/.../perm_persist` | 壳 | `ohmy.rs:9` |
| **Codex 引擎目录（新增）** | `com.teemocode.desktop/codex/`（设 `CODEX_HOME`） | 壳 bootstrap | 取代 `~/.codex` 默认家目录 |
| **Codex 会话缓存（新增）** | `com.teemocode.desktop/codex/sessions/<threadId>/rollout-*.jsonl` | Codex app-server | `config/src`：`CODEX_HOME/sessions` |

**核心不变量**：同一个 `sid` 同时拼进 `ohmy-sessions/<sid>/`（driver 侧）与
`ohmyagent/sessions/<sid>/`（引擎侧），会话恢复/回放均依赖该 sid 对齐
（`session.rs:119-124`）。Codex 侧因 threadId 由 Codex 自身生成，需在 `meta.json`
维护 `shell_sid ↔ codex_thread_id` 映射。

**会话缓存与代码工作目录是分离的**：对话历史不混进项目目录，真实代码改动落在
cwd（项目目录或 `chat-workspaces/chat-xxxx`），两引擎共享同一 cwd。

## 设计目标

- 抽象统一 `EngineDriver` 接口层；OhMyAgent 与 Codex 各自实现，**均为常驻
  JSON-RPC client**，UI 归约层零改动。
- 新增 `CodexEngine`：spawn `codex app-server --listen stdio://`、走 JSON-RPC、
  把事件翻译为 Frame。
- 引擎可切换：全局默认引擎 + 会话创建时指定引擎。
- `codex` 二进制（app-server 形态）随桌面应用分发（类比现有 OhMyAgent 落在
  `ohmyagent/` 的方式）。
- 复用 MonkeyCode 已配置的 OpenAI provider 映射为 Codex 认证参数，避免重复登录。
- 复用现有「sid 双写」不变量与 `valid_session_id` 路径守卫，不另造目录树。

## 设计决策

| 决策项 | 选择 | 理由 |
|--------|------|------|
| 接入形态 | spawn `codex app-server --listen stdio://`（常驻 JSON-RPC，同 OhMyAgent） | 与现有 driver 架构同构；规避「exec 一次性编排器」的虚拟常驻伪装成本；保留实时审批/中断能力 |
| 会话目录结构 | 沿用扁平 sid：`codex/sessions/<threadId>` 与 `ohmyagent/sessions/<sid>` 并列；`meta.json` 维护 sid↔threadId 映射 | 复用现有 `ohmy-sessions/<sid>` 回放/恢复/清理守卫，零破坏 |
| 工作目录 | 两引擎共享同一 cwd（项目目录或 chat-workspaces） | 切换引擎不丢文件进度；代码改动是「地面真相」，与对话历史解耦 |
| 切换语义 | 空闲时 `destroy` 旧引擎会话 + `create` 新引擎会话（`app-server` 有真实 RPC 对应） | 直接复用 `ohmy.rs:7` 已有的「destroy + create{resume}」变通模式 |
| 上下文连续性 | Handoff 摘要层（旧引擎产出结构化摘要喂给新引擎） | 跨引擎对话历史无法互解析，摘要近似续接，文件进度由共享 cwd 保证 |
| 实时审批 | 复用 `app-server` 的 `apply_patch_approval` / `exec_command_approval` / `turn_interrupt` RPC | 保住 MonkeyCode 现有审批/打断 UI，无需降级为预授权 |
| 路由中枢 | 在现有 `ohmy-sessions/<sid>/meta.json` 增加 `engine` 与 `codex_thread_id` 字段 | `meta.json` 是桌面版会话权威索引（`session.rs:3`），改动局部 |
| 合规 | 仅作为独立进程调用 Codex 二进制（Apache-2.0），保留其 NOTICE | 不静态链接 `codex-rs/core`，规避 copyleft 传染 |

## 详细设计

### 1. 引擎抽象接口（`desktop/src/driver/engine.rs`，新增）

常驻 JSON-RPC 引擎模型，与现有 `transport.rs` 的行分隔 JSON-RPC 框架同构：

```rust
/// 引擎驱动统一接口：所有方法以 Frame 为统一输出，UI 归约层不感知后端。
pub trait EngineDriver: Send + Sync {
    /// 能力握手（对应 OhMy system/ready / Codex initialize）
    fn capabilities(&self) -> HashSet<String>;
    /// 创建会话，返回引擎侧 session/thread id（可能需要 sid 映射）
    fn create_session(&self, opts: SessionOpts) -> Result<String, String>;
    /// 发送一轮输入（对应 session/send / turn/run）
    fn send(&self, id: &str, input: SessionInput) -> Result<(), String>;
    /// 销毁会话（对应 session/destroy / thread 生命周期管理）
    fn destroy_session(&self, id: &str) -> Result<(), String>;
    /// 从引擎侧事件流产出 Frame（由 driver 帧管线消费）
    fn event_stream(&self, id: &str) -> Receiver<Frame>;
    /// 切换模型（对应 session/switchModel / ThreadResumeParams.model）
    fn switch_model(&self, id: &str, model: &str) -> Result<(), String>;
    /// 实时审批应答（对应 respond / apply_patch_approval / exec_command_approval）
    fn approve(&self, id: &str, decision: ApprovalDecision) -> Result<(), String>;
    /// 中断当前轮（对应 session 中断 / turn_interrupt）
    fn interrupt(&self, id: &str) -> Result<(), String>;
}
```

`Inner`（`ohmy.rs:135-166`）新增两个根目录字段：
```rust
pub(super) codex_data_dir: PathBuf,        // <app_config>/codex  (CODEX_HOME)
pub(super) codex_engine_dir: PathBuf,      // <app_config>/codex/sessions
```
由 `ShellCtx` 新增 `codex_config_dir()` 提供（类比 `config_dir()`）。

### 2. OhMyAgent 适配器（`desktop/src/driver/ohmy.rs`，重构）

将现有逻辑包为 `struct OhmyEngine`，实现 `EngineDriver`，行为零变化。
仅结构迁移，确保 `ohmy_tests.rs` 全部通过。

### 3. Codex 适配器（`desktop/src/driver/codex.rs`，新增）

- **启动**：spawn `codex app-server --listen stdio:// --session-source <src>
  --config openai_base_url=...`（认证走 env `CODEX_API_KEY` / `--config`，复用
  MonkeyCode 已配置的 OpenAI provider）。设 `CODEX_HOME=<codex_data_dir>` 使会话
  落进 `com.teemocode.desktop/codex/sessions`。
- **协议翻译**（JSON-RPC client，对照 `app-server-transport/stdio.rs` 的行分隔帧）：
  - 握手 → `initialize`
  - 建会话 → `thread/start`（或 `thread/resume` 带 `codex_thread_id`）
  - 发输入 → `turn/run{threadId, turnId, input}`
  - 收事件 → `thread_started` / `turn_completed` / `item_added` 等 → 翻译为 Frame
  - 审批 → `apply_patch_approval` / `exec_command_approval` 应答
  - 中断 → `turn_interrupt`（`Op::Interrupt`）
- **会话 id 映射**：Codex 的 threadId 由 `reserve_thread_id()` 生成（非 8 位 sid），
  driver 层维护 `shell_sid ↔ codex_thread_id`（类比现有 `shell_sid_of` 反查），
  路径拼接走受控函数，遵守 `valid_session_id` 守卫（`session.rs:130`）。

### 4. 切换与配置

- 全局默认引擎：桌面设置项 `engine: "ohmy" | "codex"`，落配置文件。
- 会话级引擎选择：`session/create` 入参增加 `engine`；UI 新建会话可下拉。
- `meta.json` 字段扩展：
  ```json
  {
    "sid": "06b3e207",
    "engine": "codex",
    "codex_thread_id": "thr_xxx-uuid",
    "ohmy_session_id": "06b3e207",
    "cwd": "D:\\works\\MonkeyCodeAi\\..."
  }
  ```

### 5. 二进制分发与降级

- 桌面 bootstrap 增加 `codex` 二进制获取（含 `app-server` 子命令能力）：版本锁定、
  下载、sha 校验，落 `com.teemocode.desktop/codex/bin/`（类比 `ohmyagent/bin/`）。
- 启动自检（`doctor` 类）：`codex` 缺失或版本不符 → UI 禁用该引擎并提示。

### 6. 合规

- 在 `NOTICE` / 关于页追加 Codex（Apache-2.0）许可证与版权声明。
- 不静态链接 `codex-rs`，仅以独立进程调用。

## 会话隔离与上下文连续性（关键澄清）

- **会话缓存隔离**：`ohmyagent/sessions/<sid>` 与 `codex/sessions/<threadId>` 物理
  分离，互不读对方格式，不污染。
- **工作目录共享**：两引擎指向同一 cwd，切换时磁盘代码改动全部保留。
- **对话上下文跨引擎不保证完整**：两者对话历史格式互不可解析。切换时通过
  Handoff 摘要层近似续接（旧引擎产出结构化摘要 → 作为新引擎首条消息）。
- **切回旧引擎可原样恢复**：因旧引擎的 `<sid>` 会话缓存原样保留，
  仅需在 `meta.json` 切回 `engine` 字段并 resume。

## 风险与约束

| 编号 | 风险 | 缓解 |
|------|------|------|
| R-1 | Codex 引擎级接口是 `app-server` JSON-RPC，method/param/事件 schema 需逐字段映射 | Phase 0 Spike 抓真实报文，产出映射表 |
| R-2 | `app-server` 默认 `--session-source vscode`，含 IDE 专属逻辑（code_mode_host / attestation） | Spike 确认 MonkeyCode 下取值（如 `cli` / 自定义），规避 IDE 路径 |
| R-3 | AGPL 项目分发 Apache-2.0 二进制合规 | 保留 NOTICE，仅进程调用，不链接 |
| R-4 | Codex 自带沙箱（bwrap/Windows sandbox）与 MonkeyCode 桌面沙箱策略可能冲突 | Phase 2 对齐权限模式，限定 cwd |
| R-5 | 两套私有 JSON-RPC 协议字段级不兼容（方法名/事件名不同） | `CodexEngine` 做翻译层，工作量确定但不可省略（见 Phase 2） |
| D-1 | 未校验 sid 会被 `remove_dir_all` 向上清空主目录（`session.rs:120`） | Codex 侧 threadId 同样走 `valid_session_id` + sid 映射 |
| D-2 | cwd 在 Windows 上 `~` 展开错位（`config.rs:161-167`，USERPROFILE 优先） | Codex 继承壳已算好的 cwd，不复算 |
