# 模型网关实施计划

- 配套 spec: `docs/superpowers/specs/2026-08-30-model-gateway-design.md`
- 目标: 桌面端内置 OpenAI 兼容模型网关（模型组调度/故障切换/组级上下文），含 UI 与测试。

## Global Constraints

- 不新增任何 cargo/npm 依赖（reqwest stream、tokio、getrandom 已在树上）。
- 新命令四处登记，过 `scripts/check_command_contract.py`；UI 颜色走 daisyUI token。
- config.json 是唯一权威；网关命令走 `update_config_json` 事务；`merge_shell_prefs` 保全 `gateway`。
- 权威架构文档 desktop/ARCHITECTURE.md 需补网关一节。

## Task 1: 配置层

- [ ] config.rs: `GatewaySettings/ModelGroup/GroupModel` 类型 + `DesktopConfig.gateway`(serde default) + Default/merge_shell_prefs 保全 + 单测（旧配置无字段→默认、表单保存不清组）
- [ ] config.rs: 抽 `resolve_monkeycode_connection`（write_ohmyagent_config 改用，既有单测不动通过）

## Task 2: 调度器 sched.rs

- [ ] 候选序列: priority(权重降序稳定) / weighted(健康内加权随机,失败重抽)
- [ ] 熔断: 连败 3 开 30s,半开单探,成功归零; 单测覆盖状态转移与调度跳过

## Task 3: 上游适配 upstream.rs

- [ ] 请求构建: openai 直通(钳 max_tokens/注 system/温度) / anthropic 转换(system+messages+图片块) / responses 转换(instructions+input)
- [ ] 响应: openai 直通 / anthropic 解析(content[].text,usage,stop_reason) / responses 解析(output_text,usage)
- [ ] SSE: openai 原样中继(旁路抓 usage) / anthropic 事件→chat.completion.chunk / responses 事件→chunk; 单测对表

## Task 4: HTTP 服务 server.rs

- [ ] 手写最小 HTTP(头/体上限、Bearer 常时比较、在途上限 32、nonblocking accept + stopped 标志)
- [ ] /v1/models、/v1/chat/completions(流式 close-delimited/非流式)、/health、错误形态
- [ ] 故障切换循环(首字节前) + 日志/计数 + `X-Gateway-Model`/`X-Gateway-Group`
- [ ] 集成测试: 假上游(200 JSON/200 SSE/恒 500),断言直通/中继/切换/401/models

## Task 5: 命令与装配 mod.rs + main.rs

- [ ] GatewayHost managed state(快照+健康+日志+计数, Mutex<Option<服务>>); init/reload/重启
- [ ] 七条命令; setup 挂载; save_config 后 reload
- [ ] build.rs + tauri.conf.json + tauri.debug.conf.json capability; 契约脚本过

## Task 6: UI

- [ ] lib/ipc/gateway.ts(字面量 invoke + 浏览器降级) + 测试
- [ ] i18n zh/en `settings.gateway.*`
- [ ] GatewaySection.tsx: 总开关/端口/端点复制/运行态; 组列表(展开编辑: 名称/策略/上下文/系统提示词/超时; 模型行: 引用下拉或自定义+权重+启停+健康); 测试按钮; 请求日志表
- [ ] SettingsView 挂导航; check_theme_tokens 过

## Task 7: 验收

- [ ] cargo test（新增模块全绿，既有不回归）
- [ ] ui-next npm run test + npm run build
- [ ] python scripts/check_command_contract.py / check_theme_tokens.py
- [ ] ARCHITECTURE.md 补「模型网关」节
