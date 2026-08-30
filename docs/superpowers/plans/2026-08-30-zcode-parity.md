# ZCode 对标补强实施计划

- 配套 spec: `docs/superpowers/specs/2026-08-30-zcode-parity-design.md`

## Task 1: 计划模式(纯 UI)

- [ ] prefs: readPlanMode/writePlanMode(`mc.planMode.<sid>`)
- [ ] `src/lib/ipc/planMode.ts`: buildPlanPreamble + stripInjectedPreambles(组合剥离)
- [ ] useComposer: planOn 状态与拼装(plan 在外层,入队存最终文本)
- [ ] Composer: 「计划」badge 开关(团队开关旁)
- [ ] LogList: 换用组合剥离
- [ ] i18n zh/en + planMode.test.ts

## Task 2: 工作区记忆面板

- [ ] Rust `src/memory.rs`: memory_read/memory_write(atomic_write_private,256KB 上限)+ 单测
- [ ] 四处登记: main.rs handler、build.rs、tauri.conf.json、tauri.debug.conf.json
- [ ] ipc memory.ts + 测试
- [ ] MemoryDialog.tsx(弹窗)+ ChatView「…」菜单入口
- [ ] i18n zh/en

## Task 3: 验收

- [ ] cargo test 全量;vitest 全套;tsc;build;check_command_contract
- [ ] 重建 debug exe;提交
