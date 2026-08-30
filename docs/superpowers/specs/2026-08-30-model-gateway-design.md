# 模型网关（统一大模型调度平台）设计

- Date: 2026-08-30
- Status: accepted
- 关联计划: `docs/superpowers/plans/2026-08-30-model-gateway.md`

## 背景与需求

桌面端（TeemoCode）内置一个**大模型统一调度平台**：

1. 用户创建**模型组**，在组内配置多个模型，组对外暴露**一个统一接口**供外部程序调用；
2. 组内某个模型请求**报错时自动切换**到组内其他模型；
3. 按**权重顺序**决定调用次序；
4. 可配置多个模型组；
5. 模型组有**组级共享上下文设置**（全组模型共用一套上下文/输出/提示词配置）。

## 需求发散（本轮一并落地）

在需求 1-5 之上，补齐让它成为"平台"而非"转发器"的能力：

- **OpenAI Chat Completions 兼容协议**：外部调用方用任意 OpenAI SDK/工具（Cherry Studio、Dify、沉浸式翻译、curl…）把 base_url 指到 `http://127.0.0.1:{port}/v1` 即可，无需改造。
- **三协议上游适配**：组内模型可混用桌面端已配置的 openai / anthropic / openai_responses 三种协议的模型，网关做协议转换（请求/响应/流式 SSE 双向翻译）。
- **健康熔断**：连续失败 ≥3 次的模型自动熔断 30s（半开探测恢复），避免每次请求都在坏模型上白白超时。
- **两种调度策略**：`priority`（顺序优先：权重最高者恒先，失败顺延）、`weighted`（加权随机：健康模型间按权重分流，失败退出重抽）。
- **组级 API Key**：每组独立 Bearer Key（组即身份），可随时重置；不做全局 Key（组是调度与鉴权的统一边界）。
- **调用观测**：最近 100 条请求日志（模型/状态/耗时/Token/错误）+ 会话期累计计数，UI 实时可见；每次尝试的上游模型通过响应头 `X-Gateway-Model` / `X-Gateway-Group` 外显。
- **连通性测试**：组级「测试」按钮走真实调度链路（含故障切换），报告最终由哪个模型应答。
- **模型来源引用**：组内条目可引用桌面端模型库（config.models，含会员模型凭据注入），也可独立手填（指向库外模型）；引用型条目凭据单一来源，库中改名/删除自动外显为不可用。

## 明确不做（本轮）

- 不做对话历史托管（网关无状态，上下文由调用方按 OpenAI 协议每次传全量；"全组共享上下文设置"指组级**配置**共享，见下）。
- 不做 embeddings/completions(legacy)/audio 等非 chat 端点（404 外显）。
- 不做跨进程持久化的用量统计（会话期内存计数即可，桌面端引擎用量已有 stats.rs 一条线）。
- 不做局域网监听（恒 127.0.0.1，需要局域网访问时走系统级反代，安全面不进壳）。

## 架构与放置

新增壳模块 `desktop/src/gateway/`（mod/sched/upstream/server 四文件），与 browser/（扩展桥）同级的**壳原生服务**：UI 只经 IPC 命令对话，不建立网络连接；网关与引擎零耦合（引擎不感知网关存在）。

```
外部调用方(OpenAI SDK) ──HTTP──▶ gateway/server.rs(127.0.0.1:{port}, 手写最小 HTTP + SSE)
                                   ├─ 鉴权: Bearer = 组 Key → 定位组
                                   ├─ sched.rs: 调度(权重/策略/熔断) → 候选序列
                                   └─ upstream.rs: 按候选协议调上游(openai 直通/anthropic/responses 翻译) → OpenAI 格式应答
配置权威: config.json DesktopConfig.gateway(经 ConfigStore 事务, merge_shell_prefs 保全)
运行时快照: GatewayHost(壳 managed state) — 配置变更后 reload,请求期只读
```

## 契约

### 配置（config.json 权威）

```rust
GatewaySettings { enabled: bool(默认 false), port: u16(默认 8317), groups: Vec<ModelGroup> }
ModelGroup { id: "mg-<hex>", name, enabled, key: "tgk-<32hex>", strategy: "priority"|"weighted",
             context_window: i64(默认 128_000), max_output: i64(默认 32_768),
             temperature: Option<f64>, system_prompt: String, timeout_seconds: u64(默认 120),
             models: Vec<GroupModel> }
GroupModel { id: "gm-<hex>", enabled, weight: u32(1..=100,大=先/多), alias: String(引用模型库;空=自定义),
             provider, base_url, api_key, model }  // 自定义条目四字段生效;引用条目运行期解析
```

- 组的增删改走**独立 gateway_* 命令**（`update_config_json` 事务），不进设置页 save_config 表单；`merge_shell_prefs` 必须保全 `gateway`（同 pet_* 惯例），防止表单保存把组清空。
- 引用条目解析：`config.models` 按 name(=alias) 匹配；会员条目（source=monkeycode）复用与引擎物化同一套凭据注入（抽出 `config::resolve_monkeycode_connection`，写 ohmyagent 配置处同步改用，行为不变有单测钉着）。

### 对外 HTTP（`http://127.0.0.1:{port}`）

| 端点 | 行为 |
|---|---|
| `GET /v1/models` | 任一有效组 Key 鉴权；返回启用组为 OpenAI model 条目（id=组名，context_length=组 context_window） |
| `POST /v1/chat/completions` | Bearer 组 Key 定位组；OpenAI Chat Completions 请求；`stream:true` 走 SSE（close-delimited），否则 JSON |
| `GET /health` | 免鉴权 `{ok:true}`（探活） |
| 其余 | 405/404，OpenAI error 形态 |

- 失败切换只在**首字节发出前**发生：上游连接失败/超时/任何 HTTP≥400 均视为该候选失败，换下一个；流已开始后中断则补发一条 SSE error 事件并关闭。
- 全部候选失败 → 返回最后一个上游错误（保留其 HTTP 状态），body 附各尝试摘要；请求本身非法 → 400。
- 组级共享上下文的执行点：`max_tokens`/`max_completion_tokens` 超 `max_output` 时钳制；未指定时补默认；`temperature` 未指定时套组默认；`system_prompt` 非空时前置（openai: system 消息 / anthropic: system 参数 / responses: instructions）；`context_window` 在 /v1/models 外显并用于日志侧请求规模估算。

### IPC 命令（七条，四处登记）

`gateway_status`(运行态+组+健康+计数) / `gateway_log` / `gateway_save_group` / `gateway_delete_group` / `gateway_update_settings`(enabled+port,重启服务) / `gateway_regen_key` / `gateway_test_group`(走真实调度链路)。

登记四处：main.rs invoke_handler、build.rs commands、tauri.conf.json main-app capability、tauri.debug.conf.json main-app capability；`scripts/check_command_contract.py` 守护。

### 生命周期

- setup 时按配置 enabled 启动；端口被占 → 服务状态 Failed 外显在设置页（不阻塞应用其余功能）。
- `gateway_update_settings` / 组变更后 reload 运行时快照（enabled/port 变更重启监听线程）；save_config 后同样 reload（兜用户手编 config.json）。
- 监听线程：std::net + 每连接一线程（browser/mcp.rs 同款），在途连接上限 32；上游调用经 `tauri::async_runtime::block_on` + reqwest(stream)。

## 验证

- Rust 单测：调度顺序/加权分布/熔断开-半开-恢复；三协议请求与 SSE 翻译对表；配置 serde 默认与 merge 保全；引用解析含会员注入。
- 集成测试（假上游 HTTP 服务）：非流式直通、SSE 中继、故障切换（首个上游恒 500）、鉴权失败、/v1/models。
- UI：ipc 域测试 + 组件冒烟；`check_command_contract.py`、`check_theme_tokens.py` 过；ui-next `npm run test` + `npm run build` 过。
