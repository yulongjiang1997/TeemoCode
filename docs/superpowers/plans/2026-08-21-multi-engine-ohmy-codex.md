# 多引擎可切换执行计划（OhMyAgent / Codex）

> 配套 spec：`specs/2026-08-21-multi-engine-ohmy-codex-design.md`
> 目标：桌面端在不改动 UI 归约层的前提下，支持 OhMyAgent 与 Codex 二进制引擎
> 自由切换，复用现有 sid 双写不变量与路径守卫。

## Phase 0 — 调研 Spike（消除 R-1 / R-2，必须先做）

- [ ] 本地安装 `codex` 二进制，实测其机器可读模式的准确调用方式
      （预期 `codex --jsonl` 或 `codex exec --json` 之类），抓取真实 JSONL 事件样本。
- [ ] 对照 `codex/sdk/typescript/src/events.ts` 类型，列出
      **Codex 事件 → Frame 映射表**（复用 `frame.rs` 词汇）。
- [ ] 确认免登录传入 API Key 的方式（`--config openai_api_key=` / `openai_base_url=`
      或环境变量），以及跳过 Git 仓库检查、自定义 `CODEX_HOME` 的参数。
- [ ] 产出：`docs/specs/codex-engine-protocol.md`（调用示例 + 事件映射 + 参数清单）。

## Phase 1 — 抽象层（核心地基）

- [ ] 新增 `desktop/src/driver/engine.rs`，定义 `trait EngineDriver`
      （`capabilities` / `create_session` / `send` / `destroy_session` /
      `event_stream` / `switch_model`），所有方法以 Frame 为统一输出。
- [ ] 重构 `desktop/src/driver/ohmy.rs` → `struct OhmyEngine` 实现该 trait，
      **行为零变化**，仅结构迁移；确保 `ohmy_tests.rs` 全绿。
- [ ] 拆分 `transport.rs` 进程生命周期为 `transport/jsonrpc.rs`（供 OhMy）与
      预留 `transport/jsonl.rs`（供 Codex）。
- [ ] `Inner`（`ohmy.rs:135`）新增 `codex_data_dir` / `codex_engine_dir` 字段；
      `ShellCtx` 新增 `codex_config_dir()`。

## Phase 2 — Codex 适配器

- [ ] 新增 `desktop/src/driver/codex.rs` → `struct CodexEngine`：
      - `create_session`：设 `CODEX_HOME=<codex_data_dir>`，注入 MonkeyCode 已配
        OpenAI provider 的 key/base_url，spawn `codex`，cwd 指向共享工作目录。
      - `send`：写 prompt 到子进程 stdin（或等价启动参数）。
      - `event_stream`：读 stdout JSONL → 翻译为 Frame。
- [ ] 维护 `shell_sid ↔ codex_session_id` 映射（类比 `shell_sid_of`），
      路径拼接走受控函数，遵守 `valid_session_id` 守卫（`session.rs:130`）。
- [ ] 若需扩展 Frame 词汇（reasoning / outputSchema），在 `frame.rs` 增补变体。

## Phase 3 — 切换与配置

- [ ] 桌面设置增加全局默认引擎 `engine: "ohmy" | "codex"`。
- [ ] `session/create` 入参增加 `engine`；UI 新建会话下拉选择。
- [ ] 扩展 `ohmy-sessions/<sid>/meta.json`：增加 `engine` / `codex_session_id` /
      `ohmy_session_id` / `cwd` 字段（spec §详细设计 4）。
- [ ] 切换实现：复用 `ohmy.rs:7` 的「destroy + create{resume}」变通，挂钩
      Handoff 摘要（旧引擎产出结构化摘要 → 作为新引擎首条消息）。
- [ ] 恢复逻辑：切回旧引擎时按 `meta.json` 的 `engine` 字段 resume 原 `<sid>` 会话。

## Phase 4 — 二进制分发

- [ ] 桌面 bootstrap 增加 `codex` 二进制获取（版本锁定 / 下载 / sha 校验），
      落 `com.teemocode.desktop/codex/bin/`（类比 `ohmyagent/bin/`）。
- [ ] 启动自检（`doctor` 类）：`codex` 缺失或版本不符 → UI 禁用该引擎并提示。

## Phase 5 — 合规与测试

- [ ] `NOTICE` / 关于页追加 Codex（Apache-2.0）许可证与版权声明（NF-3）。
- [ ] 单测：`frame.rs` 翻译逻辑、`EngineDriver` trait mock。
- [ ] 集成测：实跑 `CodexEngine` 完成最小任务（改文件 + 读回 Frame），
      与 `OhmyEngine` 对照。
- [ ] 端到端：桌面内切换两种引擎各跑一遍需求，验证 UI 归约层无改动即可工作。
- [ ] 路径守卫回归：确认 Codex 侧 session id 同样经过 `valid_session_id`，
      不触发 `session.rs:120` 警告的 `remove_dir_all` 越界。

## 交付物清单

- `desktop/src/driver/engine.rs`（新）
- `desktop/src/driver/ohmy.rs`（重构为 `OhmyEngine`）
- `desktop/src/driver/codex.rs`（新）
- `desktop/src/driver/transport/{jsonrpc,jsonl}.rs`（拆分/新）
- `desktop/src/driver/frame.rs`（按需扩展）
- `ShellCtx::codex_config_dir()` + `Inner` 新增字段
- `meta.json` 字段扩展
- `docs/specs/codex-engine-protocol.md`（Spike 产出）
- `NOTICE` 增补

## 验收标准

1. 默认引擎为 OhMyAgent 时，行为与现状完全一致（回归测试通过）。
2. 全局切到 Codex 后，新建会话能正常 spawn `codex`、流式收到 Frame、产出代码文件。
3. 同一任务内切换引擎：文件进度因共享 cwd 不丢失；对话经 Handoff 摘要近似续接；
   切回原引擎可原样 resume。
4. `codex` 二进制缺失时，引擎选项灰态禁用并提示，不影响 OhMyAgent 使用。
5. 不引入 AGPL/Apache 链接合规风险（仅进程调用）。
