# 多引擎可并存执行计划（OhMyAgent / Codex）

> 配套 spec：`specs/2026-08-21-multi-engine-ohmy-codex-design.md`
> 目标：桌面端在不改动 UI 归约层的前提下，支持 OhMyAgent 与 Codex（**app-server
> 常驻 JSON-RPC 引擎**）并存可选；**每个任务绑定单一引擎、无中途切换**，因此
> 工作任务数据（文件 + 对话 + 元数据）100% 不丢。复用现有 sid 双写不变量与路径守卫。
>
> 关键修正：① Codex 接入形态为 `codex app-server --listen stdio://`（常驻 JSON-RPC，
> 与 OhMyAgent `--stdio` 同构），**不是** `exec --experimental-json` 一次性编排；
> ② 采用**任务绑定引擎**语义，删去中途切换及其 Handoff 摘要层。见 spec。

## Phase 0 — 调研 Spike（消除 R-1 / R-2，必须先做）

- [ ] 本地安装 `codex` 二进制，启动 `codex app-server --listen stdio://`，
      实测其 JSON-RPC 报文（非 exec 模式）：
      - 握手：`initialize` → `InitializeResponse`
      - 建会话：`thread/start`（或 `thread/resume`）
      - 发输入：`turn/run{threadId, turnId, input}`
      - 收事件：`thread_started` / `turn_completed` / `item_added` 等
      - 审批：`apply_patch_approval` / `exec_command_approval`
      - 中断：`turn_interrupt`（`Op::Interrupt`）
- [ ] 对照 `app-server-transport/src/transport/stdio.rs` 确认帧格式（行分隔 JSON、
      是否为 LSP 风格还是裸 JSON-RPC），评估能否复用 MonkeyCode 现有
      `driver/transport.rs` 的行分隔 JSON-RPC 框架（仅换协议解析层）。
- [ ] 确认 `--session-source` 在 MonkeyCode 下的取值（默认 `vscode` 含 IDE 专属
      逻辑），选非 IDE 模式或确认无副作用。
- [ ] 确认认证注入方式：`CODEX_API_KEY` env 与 `--config openai_base_url=`，以及
      `CODEX_HOME` 指向 `com.teemocode.desktop/codex` 使会话落本地目录。
- [ ] 产出：`docs/specs/codex-app-server-protocol.md`（真实报文 + Codex 事件 →
      Frame 映射表 + 参数清单 + session-source 结论）。

## Phase 1 — 抽象层（核心地基）

- [ ] 新增 `desktop/src/driver/engine.rs`，定义 `trait EngineDriver`
      （`capabilities` / `create_session` / `send` / `destroy_session` /
      `event_stream` / `switch_model` / `approve` / `interrupt`），所有方法以
      Frame 为统一输出，常驻 JSON-RPC 语义。
- [ ] 重构 `desktop/src/driver/ohmy.rs` → `struct OhmyEngine` 实现该 trait，
      **行为零变化**，仅结构迁移；确保 `ohmy_tests.rs` 全绿。
- [ ] 拆分 `transport.rs` 进程生命周期为 `transport/jsonrpc.rs`（通用行分隔
      JSON-RPC 框架，供两个引擎复用）与协议适配层（OhMy 协议 / Codex 协议）。
- [ ] `Inner`（`ohmy.rs:135`）新增 `codex_data_dir` / `codex_engine_dir` 字段；
      `ShellCtx` 新增 `codex_config_dir()`。

## Phase 2 — Codex 适配器（JSON-RPC client 翻译层）

- [ ] 新增 `desktop/src/driver/codex.rs` → `struct CodexEngine`：
      - 启动：`codex app-server --listen stdio:// --session-source <src>`
        `--config openai_base_url=...`，env 注入 `CODEX_API_KEY`、`CODEX_HOME`。
      - `create_session` → `initialize` + `thread/start`（resume 时带 `codex_thread_id`）。
      - `send` → `turn/run{threadId, turnId, input}`。
      - `event_stream` → 解析 stdout JSON-RPC 通知，翻译为 Frame（复用 `frame.rs`）。
      - `approve` → `apply_patch_approval` / `exec_command_approval` 应答。
      - `interrupt` → `turn_interrupt`。
- [ ] 维护 `shell_sid ↔ codex_thread_id` 映射（类比 `shell_sid_of`），路径拼接走
      受控函数，遵守 `valid_session_id` 守卫（`session.rs:130`）。
- [ ] 若需扩展 Frame 词汇（reasoning / outputSchema），在 `frame.rs` 增补变体。

## Phase 3 — 引擎选择与路由（任务绑定语义）

- [ ] 桌面设置增加全局默认引擎 `engine: "ohmy" | "codex"`。
- [ ] `session/create` 入参增加 `engine`（默认取全局值，新建会话可下拉覆盖）；
      **选定后即绑定，任务生命周期内不可中途更换**。
- [ ] 扩展 `ohmy-sessions/<sid>/meta.json`：增加 `engine` / `codex_thread_id` /
      `ohmy_session_id` / `cwd` 字段（spec §详细设计 4）。
- [ ] 路由实现：任务运行时按 `meta.json.engine` 路由到对应引擎；文件产物写入该任务
      cwd；对话历史写入该引擎私有会话（OhMy → `ohmyagent/sessions/<sid>`，
      Codex → `codex/sessions/<threadId>`），**不迁移**。
- [ ] 并存约束：同一时刻仅一个引擎 active 操作其绑定任务的 cwd，避免并发写冲突。

## Phase 4 — 二进制分发

- [ ] 桌面 bootstrap 增加 `codex` 二进制获取（需含 `app-server` 子命令能力）：
      版本锁定 / 下载 / sha 校验，落 `com.teemocode.desktop/codex/bin/`
      （类比 `ohmyagent/bin/`）。
- [ ] 启动自检（`doctor` 类）：`codex` 缺失或版本不符 → UI 禁用该引擎并提示。

## Phase 5 — 合规与测试

- [ ] `NOTICE` / 关于页追加 Codex（Apache-2.0）许可证与版权声明（NF-3）。
- [ ] 单测：`frame.rs` 翻译逻辑、`EngineDriver` trait mock。
- [ ] 集成测：实跑 `CodexEngine` 完成最小任务（initialize → thread/start →
      turn/run → 收到 Frame → 产出代码文件），与 `OhmyEngine` 对照。
- [ ] 端到端：分别用 OhMyAgent 与 Codex 各建一个任务跑需求，验证 UI 归约层无改动即可工作、两引擎并存互不污染。
- [ ] 路径守卫回归：确认 Codex 侧 threadId 同样经过 `valid_session_id`，
      不触发 `session.rs:120` 警告的 `remove_dir_all` 越界。

## 交付物清单

- `desktop/src/driver/engine.rs`（新，常驻 JSON-RPC 语义 trait）
- `desktop/src/driver/ohmy.rs`（重构为 `OhmyEngine`）
- `desktop/src/driver/codex.rs`（新，`app-server` JSON-RPC client）
- `desktop/src/driver/transport/jsonrpc.rs`（通用框架，两引擎复用）
- `desktop/src/driver/frame.rs`（按需扩展）
- `ShellCtx::codex_config_dir()` + `Inner` 新增字段
- `meta.json` 字段扩展
- `docs/specs/codex-app-server-protocol.md`（Spike 产出）
- `NOTICE` 增补

## 验收标准

1. 默认引擎为 OhMyAgent 时，行为与现状完全一致（回归测试通过）。
2. 新建任务选 Codex 后，能正常 spawn `codex app-server`、走 JSON-RPC、流式收到
   Frame、产出代码文件；实时审批/中断 UI 工作（非预授权降级）。
3. **任务数据不丢验证**：任务绑定 Codex 后，其文件产物落在任务 cwd、对话历史落在
   `codex/sessions/<threadId>`，全程单一归属、不迁移；关闭重开后按 `meta.json`
   恢复完整上下文。OhMyAgent 任务同理，两种引擎并存互不污染。
4. `codex` 二进制缺失时，引擎选项灰态禁用并提示，不影响 OhMyAgent 使用。
5. 不引入 AGPL/Apache 链接合规风险（仅进程调用）。
