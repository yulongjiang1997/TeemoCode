# 多引擎可切换设计（OhMyAgent / Codex）

## 背景

MonkeyCode 桌面端（Tauri + Rust）当前仅驱动一种智能体内核 **OhMyAgent**：
通过 `desktop/src/driver/ohmy.rs` spawn 子进程，用私有 stdio JSON-RPC 通信
（`system/ready` → `session/create|send|destroy|switchModel|switchMode`），
事件归一化为 `desktop/src/driver/frame.rs` 的 Frame 词汇交给 UI 归约层。

`openai/codex` 是另一款成熟的本地编码智能体，其能力（工具执行、沙箱、提示词
编排）与 OhMyAgent 高度相似，但对外接口不同：它不以 JSON-RPC 服务进程形态
存在，而是以 **`codex` CLI 二进制** 形态提供，父进程经 **stdin/stdout 的 JSONL
事件流** 通信（见 `codex/sdk/typescript/README.md` 第 5 行）。

本设计的目标是：在 **不改动 UI 归约层** 的前提下，让桌面端支持 **OhMyAgent 与
Codex 二进制引擎自由切换** —— 用户可在全局默认维度、或每个会话维度选择引擎。

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

**核心不变量**：同一个 `sid` 同时拼进 `ohmy-sessions/<sid>/`（driver 侧）与
`ohmyagent/sessions/<sid>/`（引擎侧），会话恢复/回放均依赖该 sid 对齐
（`session.rs:119-124`）。

**会话缓存与代码工作目录是分离的**：`messages.jsonl` 只存对话历史，真实代码改动
落在 cwd（项目目录或 `chat-workspaces/chat-xxxx`），cwd 由会话创建时决定，不混
入会话缓存目录（从真实 `messages.jsonl` 中 `Read("D:\works\...")` 可见）。

## 设计目标

- 抽象统一 `Engine` 接口层；OhMyAgent 与 Codex 各自实现，UI 归约层零改动。
- 新增 `CodexEngine`：spawn `codex` 二进制、喂 prompt、解析 JSONL、翻译为 Frame。
- 引擎可切换：全局默认引擎 + 会话创建时指定引擎。
- `codex` 二进制随桌面应用分发（类比现有 OhMyAgent 落在 `ohmyagent/` 的方式）。
- 复用 MonkeyCode 已配置的 OpenAI provider 映射为 Codex 认证参数，避免重复登录。
- 复用现有「sid 双写」不变量与 `valid_session_id` 路径守卫，不另造目录树。

## 设计决策

| 决策项 | 选择 | 理由 |
|--------|------|------|
| 接入形态 | spawn `codex` 二进制子进程（同 OhMyAgent 模式） | 与现有 driver 架构一致；规避 AGPL 静态链接 Apache-2.0 内核的合规风险 |
| 会话目录结构 | 沿用扁平 sid：`codex/sessions/<sid>/` 与 `ohmyagent/sessions/<sid>/` 并列，靠 sid 关联 | 复用现有 `ohmy-sessions/<sid>` 回放/恢复/清理守卫，零破坏 |
| 工作目录 | 两引擎共享同一 cwd（项目目录或 chat-workspaces） | 切换引擎不丢文件进度；代码改动是「地面真相」，与对话历史解耦 |
| 切换语义 | 空闲时 `destroy` 旧引擎会话 + 用交接摘要 `create` 新引擎会话 | 复用 `ohmy.rs:7` 已有的「destroy + create{resume}」变通模式 |
| 上下文连续性 | Handoff 摘要层（旧引擎产出结构化摘要喂给新引擎） | 跨引擎对话历史无法互解析，摘要近似续接，文件进度由共享 cwd 保证 |
| 路由中枢 | 在现有 `ohmy-sessions/<sid>/meta.json` 增加 `engine` 与 `codex_session_id` 字段 | `meta.json` 是桌面版会话权威索引（`session.rs:3`），改动局部 |
| 合规 | 仅作为独立进程调用 Codex 二进制，保留其 Apache-2.0 NOTICE | 不静态链接 `codex-rs/core`，规避 copyleft 传染 |

## 详细设计

### 1. 引擎抽象接口（`desktop/src/driver/engine.rs`，新增）

```rust
/// 引擎驱动统一接口：所有方法以 Frame 为统一输出，UI 归约层不感知后端。
pub trait EngineDriver: Send + Sync {
    /// 能力握手（对应 OhMy system/ready / Codex 就绪事件）
    fn capabilities(&self) -> HashSet<String>;
    /// 创建会话，返回引擎侧 session id（可能与壳 sid 不同，需映射）
    fn create_session(&self, opts: SessionOpts) -> Result<EngineSession, String>;
    /// 发送一轮输入
    fn send(&self, sid: &str, input: SessionInput) -> Result<(), String>;
    /// 销毁会话（切换引擎前调用）
    fn destroy_session(&self, sid: &str) -> Result<(), String>;
    /// 从引擎侧事件流产出 Frame（由 driver 帧管线消费）
    fn event_stream(&self, sid: &str) -> Receiver<Frame>;
    /// 切换模型（若无协议则 destroy+create 变通）
    fn switch_model(&self, sid: &str, model: &str) -> Result<(), String>;
}
```

`Inner`（`ohmy.rs:135-166`）新增两个根目录字段：
```rust
pub(super) codex_data_dir: PathBuf,        // <app_config>/codex
pub(super) codex_engine_dir: PathBuf,      // <app_config>/codex/sessions
```
由 `ShellCtx` 新增 `codex_config_dir()` 提供（类比 `config_dir()`）。

### 2. OhMyAgent 适配器（`desktop/src/driver/ohmy.rs`，重构）

将现有逻辑包为 `struct OhmyEngine`，实现 `EngineDriver`，行为零变化。
仅结构迁移，确保 `ohmy_tests.rs` 全部通过。

### 3. Codex 适配器（`desktop/src/driver/codex.rs`，新增）

- `create_session`：设 `CODEX_HOME=<codex_data_dir>` 与 `openai_api_key`/
  `openai_base_url`（来自 MonkeyCode 已配置的 OpenAI provider），spawn
  `codex` 二进制，cwd 指向与 OhMy 相同的共享工作目录。
- 读取 stdout JSONL → 翻译为 Frame（复用 `frame.rs` 词汇，必要时扩展
  `agent_thought_chunk` / `usage_update` 等变体）。
- 会话 id 映射：Codex 自身的 session id 可能为 uuid，driver 层维护
  `shell_sid ↔ codex_session_id`（类比现有 `shell_sid_of` 反查），
  路径拼接走受控函数，遵守 `valid_session_id` 守卫（`session.rs:130`）。

### 4. 切换与配置

- 全局默认引擎：桌面设置项 `engine: "ohmy" | "codex"`，落配置文件。
- 会话级引擎选择：`session/create` 入参增加 `engine`；UI 新建会话可下拉。
- `meta.json` 字段扩展：
  ```json
  {
    "sid": "06b3e207",
    "engine": "codex",
    "codex_session_id": "<codex-uuid>",
    "ohmy_session_id": "06b3e207",
    "cwd": "D:\\works\\MonkeyCodeAi\\..."
  }
  ```

### 5. 二进制分发与降级

- 桌面 bootstrap 增加 `codex` 二进制获取：版本锁定、下载、sha 校验，
  落 `com.teemocode.desktop/codex/bin/`（类比 `ohmyagent/bin/`）。
- 启动自检（`doctor` 类）：`codex` 缺失或版本不符 → UI 禁用该引擎并提示。

### 6. 合规

- 在 `NOTICE` / 关于页追加 Codex（Apache-2.0）许可证与版权声明。
- 不静态链接 `codex-rs`，仅以独立进程调用。

## 会话隔离与上下文连续性（关键澄清）

- **会话缓存隔离**：`ohmyagent/sessions/<sid>` 与 `codex/sessions/<sid>` 物理分离，
  互不读对方格式，不污染（解决「混进 tasks/ 互相污染」的顾虑）。
- **工作目录共享**：两引擎指向同一 cwd，切换时磁盘代码改动全部保留。
- **对话上下文跨引擎不保证完整**：两者对话历史格式互不可解析。切换时通过
  Handoff 摘要层近似续接（旧引擎产出结构化摘要 → 作为新引擎首条消息）。
- **切回旧引擎可原样恢复**：因旧引擎的 `<sid>` 会话缓存原样保留，
  仅需在 `meta.json` 切回 `engine` 字段并 resume。

## 风险与约束

| 编号 | 风险 | 缓解 |
|------|------|------|
| R-1 | Codex JSONL 确切调用 flag 待验证（SDK 文档只说 spawn+JSONL） | Phase 0 Spike 实测确认 |
| R-2 | Codex 强制 Git 仓库检查 / 自带会话目录 `~/.codex/sessions` | 经 `--config` / `CODEX_HOME` 对齐；Spike 验证 |
| R-3 | AGPL 项目分发 Apache-2.0 二进制合规 | 保留 NOTICE，仅进程调用，不链接 |
| R-4 | Codex 自带沙箱/审批与 MonkeyCode 桌面沙箱策略可能冲突 | Phase 2 对齐权限模式，限定 cwd |
| D-1 | 未校验 sid 会被 `remove_dir_all` 向上清空主目录（`session.rs:120`） | Codex 侧 session id 同样走 `valid_session_id` + sid 映射 |
| D-2 | cwd 在 Windows 上 `~` 展开错位（`config.rs:161-167`，USERPROFILE 优先） | Codex 继承壳已算好的 cwd，不复算 |
