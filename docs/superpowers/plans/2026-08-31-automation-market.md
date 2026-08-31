# 自动化 + 技能市场 + MCP 一键装 实施计划

- 配套 spec: `docs/superpowers/specs/2026-08-31-automation-market-design.md`

## Task 1: 自动化 Rust 核心

- [ ] automation.rs: Automation 类型 + cron 匹配器(表驱动单测) + due 判定 + 校验
- [ ] config.rs: DesktopConfig.automations(serde default)+ merge 保全 + round-trip 测试
- [ ] 调度线程 + fire(session_create_with_kind→open→send b64) + last_result 记账
- [ ] 4 条 IPC + 四处登记;check_command_contract 绿

## Task 2: 自动化 UI

- [ ] lib/ipc/automation.ts + cronNext.ts(Rust 同语义)+ 测试
- [ ] AutomationSection(设置分区)+ SettingsView 接线 + i18n zh/en

## Task 3: 技能市场

- [ ] ImportSkillsDialog: 源选择(官方/历史/自定义)+ 历史源记录
- [ ] skills_import_git 返回可选 mcp 字段

## Task 4: MCP 一键装

- [ ] mcp_servers_install 命令(save_config 锁序重启引擎)+ 四处登记
- [ ] 市场/扫描结果的 MCP 安装入口 + i18n

## Task 5: 验收

- [ ] cargo test / vitest / tsc / build / 契约脚本;重建 debug exe;提交
