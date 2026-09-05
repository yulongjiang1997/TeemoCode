# 用户指令记忆

本文件记录了用户的指令、偏好和教导，用于在未来的交互中提供参考。

## 格式

### 用户指令条目
用户指令条目应遵循以下格式：

[用户指令摘要]
- Date: [YYYY-MM-DD]
- Context: [提及的场景或时间]
- Instructions:
  - [用户教导或指示的内容，逐行描述]

### 项目知识条目
Agent 在任务执行过程中发现的条目应遵循以下格式：

[项目知识摘要]
- Date: [YYYY-MM-DD]
- Context: Agent 在执行 [具体任务描述] 时发现
- Category: [运维部署|构建方法|测试方法|排错调试|工作流协作|环境配置]
- Instructions:
  - [具体的知识点，逐行描述]

## 去重策略
- 添加新条目前，检查是否存在相似或相同的指令
- 若发现重复，跳过新条目或与已有条目合并
- 合并时，更新上下文或日期信息
- 这有助于避免冗余条目，保持记忆文件整洁

## 条目

[任务修改记录(用户要求:每次修改任务都记录标题与大纲)]
- Date: 2026-09-05
- Context: 用户要求从现在起在记忆文件中记录每次修改任务的标题和大纲
- Instructions:
  - 每完成一个修改任务,在「任务修改记录」下追加一条:标题 + 大纲(做了什么/改了哪些文件/结果)。
  - 该记录是版本回溯与"代码丢失事故"排查的依据,必须持续维护。

---

### 任务:release 版切换任务工作区卡 6-10 秒 + cmd 黑框闪烁(定位与修复)
- Date: 2026-09-04 ~ 2026-09-05
- Context: 用户报告 0.1.27 切任务工作区渲染慢(6-10s)且卡住 UI;0.1.25/26 及官方 0.1.26 安装包不卡;debug 版不卡
- Category: 排错调试|构建方法
- 大纲:
  1. **排查过程**:用 git worktree 在独立目录依次构建 v0.1.25(不卡)、v0.1.26(不卡)、v0.1.27(卡)锁定范围;对比官方 0.1.26 安装包(8/30)与今日重打包发现 .text 段差 7.2KB,官方包含从未提交的代码(allow-todo-parse/allow-usage-stats 权限证明),该代码随 0.1.27 重打包永久丢失。
  2. **根因确认**:0.1.27 新增的 4 个 git 命令(git_branch/git_branch_list/git_is_clean/git_checkout)是同步 #[tauri::command] pub fn——Tauri 同步命令在主线程执行,切任务时 ChatView useEffect 必触发 git 子进程 spawn,大仓库+杀软下冻结 UI 数秒。
  3. **修复三处**(已提交 wip-local,commit dd6193a):
     - desktop/src/git.rs:4 个命令改 async + spawn_blocking(修卡顿)
     - desktop/src/git.rs::git():加 CREATE_NO_WINDOW(复用 wsl::no_console,修 cmd 黑框闪烁)
     - desktop/ui-next/src/features/chat/GitBranchBadge.tsx:切任务后分支查询延迟 600ms 防抖
     - 顺带修 useSessionFeed.ts:147 perf 日志 id 判空(wip-local 自带 typecheck 错误)
  4. **结果**:用户实测最新代码(wip-local v0.1.33 + 修复)不卡、无黑框,功能验证完整(0.1.26→0.1.33 全部功能在位:git 分支/系统通知/automation/plan mode/memory 面板/技能市场/MCP 一键装/team v2/model gateway)。
  5. **教训**:①官方安装包 ≠ git 代码,8/30 打包时的未提交改动已永久丢失;②新增命令必须 async 化(有 git/fs 子进程的),同步命令会冻结 UI;③新命令要动三处:main.rs invoke_handler、build.rs 命令表、tauri.conf.json capability。

### 任务:打包发布提速分析
- Date: 2026-09-04
- Context: 用户问打包为什么慢、有没有更快方法
- Category: 构建方法
- 大纲:
  - 慢的主因是 Cargo.toml release profile 的 lto = true(full LTO),链接期占大头。
  - 建议:①正式发版可改 lto = "thin"(快 2-5 倍);②测试版用 tauri build --no-bundle(不出 NSIS 安装包,实测每次 6-10 分钟);③新 worktree 首次构建需全量编译依赖,后续增量只有 6-10 分钟。
  - 构建命令参考(分离式后台进程):powershell Start-Process cmd /c "set PATH=C:\Users\12090\.cargo\bin;%PATH%&& npx @tauri-apps/cli@2 build --no-bundle > build.log 2>&1";资源布局:target/release/ 下需 browser-extension/(dist 内容平铺)、skills/、ohmyagent.exe、WebView2Loader.dll。

---

### 任务:发布 0.1.36 到 Gitee(含仓库大小检查)
- Date: 2026-09-05
- Context: 用户要求打包 0.1.36 并发布到 Gitee,发布前检查仓库大小、超限删最早发布文件
- Category: 运维部署
- 大纲:
  1. 版本号 bump: Cargo.toml + tauri.conf.json → 0.1.36, releaseHistory.ts 头部追加 0.1.36 条目(commit b3ffec3, tag v0.1.36)。
  2. 打包: 前端 vite build(20s) + tauri build --config bundle.windows.conf.json(NSIS),产物 57.8MB;签名用 Cmd 工具内联执行(私钥 C:\Users\12090\sdk\mc-release-keys-new),sig 第一行 base64 解码含 "signature from tauri secret key" 验证通过。
  3. Gitee 容量检查: HEAD 实测旧附件 ~564MB(0.1.22-0.1.31 共11版 + 0.1.34/35),加 0.1.36 后 ~622MB,未超 1GB,用户确认不删旧版本直接上传。注意 RELEASE.md 记载 Gitee 后台统计可能含历史对象(~1205MB 超 1024MB 配额),git push 到更新仓库会被拒,所以 exe 全部走 API 上传。
  4. 发布流程(按 RELEASE.md 已验证流程): exe+sig 用 attach_files 端点挂到 v0.1.35 release(id=1121225, HTTP 201);latest.json 改本地后 base64 走 Contents API PUT(旧版本 0.1.35 滚入 history 头部);端到端验证 raw URL 版本号正确、下载 URL HEAD 200 且字节数一致(57802425)。
  5. 主仓库 fork: tag v0.1.36 + wip-local 分支均已推送成功。

[Online 预览构建与验证码验收]
- Date: 2026-07-26
- Context: Agent 在排查 online 构建后登录验证码失败时发现
- Category: 构建方法|测试方法|排错调试
- Instructions:
  - 在 `frontend` 运行 `pnpm run build:online` 验证 online 生产构建。
  - 启动 online 开发预览时显式设置 API 目标，例如 `TARGET=https://monkeycode-ai.com pnpm run dev:online -- --host 0.0.0.0 --port <PORT>`。
  - 获得预览地址后运行 `PREVIEW_URL=<URL> pnpm run check:online-preview`，验证 CAP JavaScript、WASM 和 challenge API。
  - 自动健康检查通过后，在浏览器完成一次真实验证码求解和登录，再开始登录后页面的 UI 验收。
  - UI 验收需等待 `document.fonts.ready`，确认 JetBrains Mono Variable 与 Noto Sans SC Variable 已加载，并检查浏览器控制台和 Network 中没有字体资源失败。
  - 在 320px、375px、390px、430px 和 1280px 对照基准页面核对字体族、字号、字重和行高，字体变化应作为构建后高频回归项记录和处理。
  - Vite 日志出现 `Must set target or forward` 表示 `/api` proxy 缺少 `TARGET`，应使用显式目标重启预览。
