# 自动化定时任务 + 技能市场 + MCP 一键装 设计

- Date: 2026-08-31
- Status: accepted
- 关联计划: `docs/superpowers/plans/2026-08-31-automation-market.md`
- 参考: ZCode 的 Scheduled Automations 与 Plugin Marketplace

## Feature 1: 自动化(定时任务)

对标 ZCode 自动化:持久化的"到点把提示词发给 agent"任务,重启后仍在。

### 数据(config.json 权威,DesktopConfig.automations,ConfigStore 事务)

```rust
Automation { id:"auto-<hex>", name, enabled,
             kind: "once"|"cron", cron(5字段,本地时间), fire_at_ms(u64,once),
             prompt, kind_session:"local"|"chat", workdir(local 必填,chat 忽略),
             model(空=默认), last_fire_ms, last_result }
```

### 调度器(Rust `src/automation.rs`)

- setup 时起**调度线程**(telemetry 同款):每 20s 扫一次,到点即触发;
  进程退出随进程结束(不跨进程补跑——错过的一次性任务标"已过期跳过")。
- **cron 判定是分钟粒度的纯函数** `cron_matches(now_fields, expr)`:支持
  `* / , -`,域=分 时 日 月 周(0-7,0 与 7=周日);同分钟去重靠 last_fire_ms。
  本地时间用 `time` crate(已有依赖,local-offset)。
- **触发链路复用驱动层**:`DriverHost::get()` → `session_create_with_kind`
  → `session_open` → `session_send(user-input, content=b64(prompt))`;
  引擎不在位 → 记 `last_result="引擎未运行"`。once 无论成败即消费;
  cron 同分钟内只试一次(去重),引擎恢复后下一分钟自然重试。
- 所有触发经 `tauri::async_runtime::block_on`(gateway server 同款);
  UI 不在线也能跑(关窗只隐藏,帧落盘,打开即回放)。

### IPC(4 条,四处登记)

`automation_list` / `automation_save`(归一+校验:cron 语法、once 时间必填、
local 必须有 workdir、prompt 非空)/ `automation_delete` / `automation_run_now`。
下次触发时间的预览在 TS 侧算(`cronNext.ts`,与 Rust 匹配器同语义,单测对表)。

### UI

设置新增「自动化」分区:列表(名称/目标/计划/上次结果/启停) + 编辑表单
(名称、目标[本地项目=工作目录 | 新对话]、计划[一次性时间 | cron]、提示词、
模型可选) + 立即运行。cron 输入旁实时预览下次触发。

## Feature 2: 技能市场 v1(纯 UI,复用现有命令)

- ImportSkillsDialog 加**源选择**:官方市场(`https://github.com/chaitin/MonkeyCodeOfficialPlugins.git`,
  常量)+ 历史 git 源(localStorage `mc.market.sources`,自动记录)+ 自定义输入(现状保留)。
- 切源即走现有 `skills_import_git`(浅克隆+扫描),无新命令。
- 已安装判断(同名徽标)已有;更新 = 重新导入覆盖,卸载走技能库删除,均现成。

## Feature 3: MCP 一键装

- `skills_import_git` 扫描扩展:仓库根若有 `mcp.json`(`{"mcpServers":{…}}`
  引擎同构形态)则一并返回 `mcp` 字段(不改返回形状,新增可选键)。
- 新命令 `mcp_servers_install(app, entries)`:`update_config_json` 把
  entries 合并进 `cfg.mcp_servers`(同名覆盖),随后按 save_config 的锁序
  (EngineApply + DriverHost.begin_apply + apply_cloud_config + 物化 +
  restart_engine_locked)重启引擎生效;UI 侧同名冲突时确认。

## 明确不做

- 不跨进程补跑错过的任务;不支持秒级 cron/时区/月份名;不支持自动化邮件等
  通知渠道(触发结果记在 last_result,UI 列表可见)。
- 市场不做远端索引服务(git 浅克隆即目录);不做插件级 hooks/命令扩展
  (引擎无此机制)。

## 验证

- Rust:cron 匹配器表驱动单测、due 判定、config round-trip、命令校验;
  `cargo test` 全量。
- TS:cronNext 与 Rust 同语义单测;automation/market ipc 测试;vitest 全套
  + tsc + build;`check_command_contract.py`。
