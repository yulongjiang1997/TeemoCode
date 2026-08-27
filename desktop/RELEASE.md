# TeemoCode 桌面端发布指南 (Windows)

本文件记录 TeemoCode 桌面端（Tauri 2）的打包、签名与更新发布流程，重点是**签名私钥的位置**，避免后续发布时找不到密钥。

> 更新源：`https://gitee.com/xiaotimor/teemo-code-update`
> 更新元数据：`https://gitee.com/xiaotimor/teemo-code-update/raw/master/latest.json`
> 安装包 / 签名文件随每次发版提交到上述 Gitee 仓库，并通过该仓库的 Release（tag）对外提供下载。

---

## 1. 签名密钥（最重要）

Tauri updater 用 Ed25519 密钥对给安装包签名，公钥已写死在 `tauri.conf.json` 的 `plugins.updater.pubkey`。
**换个私钥就必须同步换 `tauri.conf.json` 的公钥，否则老用户自动更新会校验失败。**

| 项 | 值 |
|---|---|
| 私钥文件 | `C:\Users\12090\sdk\mc-release-keys-new` |
| 私钥密码 | `R3l3ase!K3y#2026x` |
| 公钥（已嵌 `tauri.conf.json`） | `C881F6D9A26598F3` 开头（`untrusted comment: minisign public key: C881F6D9A26598F3\nRWTzmGWi2faByG4nTBMG2hU2rav1trB9h8JIwzs0HBRzQASJAjjuA9Nh`） |

验证密钥配对（发布前必做，零风险）：
```bat
python -c "import base64,json; p=open(r'C:\Users\12090\sdk\mc-release-keys-new.pub','rb').read().strip(); print(base64.b64decode(p).decode()); print(json.load(open('tauri.conf.json',encoding='utf-8'))['plugins']['updater']['pubkey'])"
```
两份输出应完全一致（公钥明文相同）。

> ⚠️ 安全提醒：本文件含明文私钥密码。若此仓库对外公开，请勿把真实密码提交到远程，
> 改为本地用环境变量 `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` 注入，此处仅保留路径与公钥。

---

## 2. 版本号

沿用 `0.1.x` 序列（与 Gitee 已有 `v0.1.1`~`v0.1.14` 连续）。
发布前把 `Cargo.toml` 和 `tauri.conf.json` 的 `version` 改为下一个 `0.1.x`（如 `0.1.15`）。
（注：`scripts/set_release_version.py` 是给 `vYYMMDDNN` tag 用的另一套体系，本项目当前未采用。）

---

## 3. 发布步骤（v0.1.x 体系，推荐）

### 3.0 一键发布脚本（推荐）

以下 3.1~3.4 的全部步骤已固化为脚本（v0.1.17 实战流程）：

```bat
cd desktop
set TAURI_SIGNING_PRIVATE_KEY_PASSWORD=R3l3ase!K3y#2026x
set GITEE_UPDATE_TOKEN=6413249386ee049a45469c7957b5d336
python scripts\release_v0_1_x.py --version 0.1.18 --notes "更新说明"
```

脚本自动完成：版本号升级 → 提交 → UI 构建 → Tauri 打包 → 签名（cmd.exe 内）
→ Gitee master 推送 → Release 创建 + assets 上传 → 端到端验证 → 主仓库打 tag。
常用参数：`--skip-build` 复用已有产物；`--notes` 自定义更新说明。

### 3.1 打包安装包
```bat
cd desktop
package_windows.bat
```
产物：`target\release\bundle\nsis\TeemoCode_<版本>_x64-setup.exe`

### 3.2 签名（用上面的私钥 + 密码）
```bat
cd desktop
npx --yes @tauri-apps/cli@2 signer sign ^
  -f C:\Users\12090\sdk\mc-release-keys-new ^
  -p "R3l3ase!K3y#2026x" ^
  target\release\bundle\nsis\TeemoCode_<版本>_x64-setup.exe
```
生成：`TeemoCode_<版本>_x64-setup.exe.sig`（约 420 字节，单行 base64）。

> ⚠️ **必须在 cmd.exe 里签名（Cmd 工具），不要在 bash/msys 里跑！**
> 私钥密码含 `!`（bash 历史扩展）和 `#`（bash 注释符），在 bash 里即使加了双引号也可能被破坏，
> 表现为"密码错误/私钥解密失败"（2026-08-25 发 v0.1.16 踩过：同样的密码在 bash 里失败、cmd 里成功；
> v0.1.17 再次踩坑确认）。一键脚本已内置此处理。
> **v0.1.21 踩坑补充**：即使用 Cmd 工具，.bat 脚本里的 `!` 也可能被 cmd 延迟展开破坏；
> Python subprocess 调用 cmd.exe 也会遇到同样问题。**唯一可靠的方式是直接在 Cmd 工具里内联执行命令**，
> 不要通过 .bat 文件或 Python subprocess 中转。签名时绝对不要使用 Bash 工具。
> 验证签名后 .sig 第一行 base64 解码应含 `signature from tauri secret key`。

> 💡 **Gitee API 注意**（v0.1.17 实战）：创建 release 时 `target_commitish` 现在是
> **必填项且需完整 commit hash**（缺了报 `body is missing`，短 hash 报「创建标签失败」）；
> tag 必须先推送到 Gitee 再建 release；curl 在部分 Windows 环境下不稳定（exit 3/6），
> 脚本统一用 Python requests。

### 3.3 更新 Gitee 更新源

**核心机制（务必理解，踩过坑）：**
- Gitee 上 `latest.json` 是**仓库 master 分支的文件**，Tauri updater 读它拿版本号、下载 URL、签名。
- 下载 URL 用 `https://gitee.com/xiaotimor/teemo-code-update/releases/download/v<版本>/<文件名>`，这个 URL **只解析 Release 的附件（assets）**，不解析仓库文件——所以 **exe 必须上传成 Release asset**，光 commit 进仓库 master 是下载不了的（会 404/403）。
- Gitee 仓库单文件超 ~50MB 时，raw / release 下载都会被 WAF 拦，仓库直链 403；**唯一的正道是走 Release asset**。

**步骤：**

1. 准备更新仓库工作区（仓库历史里有多个 56MB 安装包，普通 `git clone` 可能卡住；用 `--depth 1`，或本地已有 `mc-update` 工作区时先 `git fetch` 再 reset）：
```bat
rem 方式 A：全新浅克隆
git clone --depth 1 https://gitee.com/xiaotimor/teemo-code-update.git C:\Users\12090\sdk\mc-update
rem 方式 B：复用已有工作区（推荐，快）
cd C:\Users\12090\sdk\mc-update
git fetch https://gitee.com/xiaotimor/teemo-code-update.git master:refs/remotes/gitee/master
git reset --hard gitee/master
```

2. 复制安装包和签名进工作区：
```bat
copy <exe> <sig> C:\Users\12090\sdk\mc-update\
```

3. 更新 `latest.json`（用 Python 脚本生成最稳，避免手改 JSON 转义出错）：
   - `version` → 新版本号
   - `notes` → 本次更新说明（中文，UTF-8）
   - `pub_date` → 当前 UTC 时间（`2026-08-25T04:59:49Z` 格式）
   - `platforms.windows-x86_64.signature` → `.sig` 文件的**完整内容**（单行 base64，长度 ~420）
   - `platforms.windows-x86_64.url` → `https://gitee.com/xiaotimor/teemo-code-update/releases/download/v<版本>/TeemoCode_<版本>_x64-setup.exe`
   - `history` → 保留旧版本记录数组（最新在上）
   - 生成后**务必回读 raw 确认**：`curl https://gitee.com/xiaotimor/teemo-code-update/raw/master/latest.json`

4. 提交并推送 master（Gitee 推送无需额外认证，git 会提示但凭据在 wincred）：
```bat
cd C:\Users\12090\sdk\mc-update
git add latest.json TeemoCode_<版本>_x64-setup.exe TeemoCode_<版本>_x64-setup.exe.sig
git commit -m "release: v<版本> ..."
git push https://gitee.com/xiaotimor/teemo-code-update.git master
```

5. 创建 Release 并上传附件（**API 端点必须用 `attach_files`**）：
   - `releases/{id}/assets` 端点会被 WAF 拦（404 HTML 页）；`attach_files` 端点正常（201）。
   - 先创建 release（需带 `target_commitish` 指向含 exe 的提交 hash；tag 可提前 push 或由 release 创建）：
```bat
rem 1) 创建 release（tag 指向 master 最新提交）
curl -s -X POST "https://gitee.com/api/v5/repos/xiaotimor/teemo-code-update/releases" ^
  -d "access_token=<TOKEN>" -d "tag_name=v<版本>" -d "name=v<版本>" ^
  -d "target_commitish=<含exe的提交hash>" -d "prerelease=false"
rem 2) 上传附件（exe 大文件也能传，返回 201；.sig 一样）
curl -s -X POST "https://gitee.com/api/v5/repos/xiaotimor/teemo-code-update/releases/<releaseId>/attach_files" ^
  -d "access_token=<TOKEN>" -F "file=@TeemoCode_<版本>_x64-setup.exe"
curl -s -X POST "https://gitee.com/api/v5/repos/xiaotimor/teemo-code-update/releases/<releaseId>/attach_files" ^
  -d "access_token=<TOKEN>" -F "file=@TeemoCode_<版本>_x64-setup.exe.sig"
```

6. **端到端验证（发布后必做）**：
```bat
rem 1) latest.json 可读、version 正确
curl https://gitee.com/xiaotimor/teemo-code-update/raw/master/latest.json
rem 2) exe 下载 200 且字节数与本地一致
curl -sL -o /tmp/dl.exe -w "%%{http_code} %%{size_download}\n" ^
  https://gitee.com/xiaotimor/teemo-code-update/releases/download/v<版本>/TeemoCode_<版本>_x64-setup.exe
rem 3) latest.json 里的 signature 与本地 .sig 完全一致（updater 校验签名必须通过）
```

> 🔑 Gitee access token（xiaotimor）：`6413249386ee049a45469c7957b5d336`（2026-08-25 验证有效；更早的 `6abf4a...` 已失效）。
> token 只作更新源操作，勿提交到仓库；建议后续改用环境变量/凭据管理。

### 3.4 主仓库打 tag
```bat
cd <MonkeyCode 主仓库>
git tag -a v<版本> -m "TeemoCode v<版本>: ..."
git push fork v<版本>
```

---

## 4. 备选：用 Makefile（vYYMMDDNN 体系）

`make windows-release` 会：检查私钥环境变量 → 按当前 commit 的 `vYYMMDDNN` tag 改写版本为 `YYMMDDNN.0.0` → 打包 → 自动签名（`tauri.release.conf.json` 的 `createUpdaterArtifacts: true`）。

前置：
- 私钥设为环境变量：`set TAURI_SIGNING_PRIVATE_KEY=<私钥内容>`
- 当前 commit 带 `vYYMMDDNN` tag（脚本 `scripts/set_release_version.py` 仅识别此格式）
- 注意改出来的版本号是 `26082101.0.0` 这类，与本项目现行 `0.1.x` 序列不连续，仅当有意切换体系时使用。

---

## 5. 注意事项

- Gitee `teemo-code-update` 仓库已超 819MB 配额（2026-08-25 发 v0.1.16 时 889MB，Gitee 已告警"超 80%"，push 仍成功但随时可能被限流/禁止）。建议：只保留最近 2~3 个版本的 exe，或迁移更新源。**每次发版直接 commit 56MB 进历史只会加速爆仓。**
- **Gitee WAF 拦 `releases/{id}/assets` 上传，但 `releases/{id}/attach_files` 正常**（v0.1.16 踩坑确认）。不要用前者，直接上 `attach_files`。
- **`releases/download/v<版本>/<文件>` 只解析 Release 附件**，commit 进仓库 master 的 exe 不会出现在该 URL（404）。必须把 exe 上传成 Release asset。
- Gitee Release 创建接口需要 `target_commitish` 参数（指向含 exe 的提交 hash），否则报 `target_commitish is missing`。
- Gitee 上传大文件后 CDN 生效可能滞后：刚传完下载 404 是正常的，等几十秒~几分钟再验证。
- `.sig` 文件是**单行** base64；`latest.json` 的 `signature` 字段填其完整内容（不要手动换行）。
- 发布后**必须**端到端验证（见 3.3 第 6 步）：latest.json 可读 + exe 下载 200 + signature 与本地一致，三者缺一不可，否则用户自动更新会失败。
- 打包依赖：`binaries/` 下需有 `ohmyagent-x86_64-pc-windows-msvc.exe`（engine sidecar）、`browser-extension/dist`（前端扩展）、NSIS 已安装。缺资源时 `make windows` 会报错，可参考 `Makefile` 的 `check-bundle-configs` / `engine-windows` / `browser-extension` 目标补齐全。

## 6. 更新源仓库清理(仓库爆仓时)

Gitee `teemo-code-update` 每次发版都把约 56MB 安装包 commit 进历史，几个版本就逼近 819MB 配额。**只保留最近 N 个版本**的清理流程（2026-08-25 发 v0.1.16 时实战，保留最后 3 个）。

关键是分辨"删文件"和"删历史"：**普通 commit 删旧 exe 不缩仓库**（对象还在历史里），必须重写主分支历史。

1. **备份**：`tar -czf mc-update-backup-$(date +%Y%m%d).tar.gz mc-update`（约 1.7GB，删错可复原）
2. **新建干净仓库**：单独目录 `git init`，只拷入**要保留版本**的 exe+sig+latest.json，单提交。这个新提交的 hash 记为 `NEW`（下文示例记为 `4e24a5c`）。
3. **删除要清理的旧 release（逐个）**：用 Gitee API `DELETE /repos/xiaotimor/teemo-code-update/releases/{id}?access_token=...`。删除会连附件一起删，释放 Gitee 空间。先记下保留版本 release 的 id 和附件，万一误删可重建。
4. **删除旧 tag**：`git push :refs/tags/v0.1.X`。**Gitee 删 tag 不会连带删 release**（实测确认，附件保留），先删 tag 最安全。
5. **force push 新 master 覆盖**：`git push --force https://gitee.com/xiaotimor/teemo-code-update.git master`，覆盖旧的多提交历史，旧提交变悬空（dangling）。
6. **重定向保留版本的 tag 到 `NEW`**：本地 `git tag -a v0.1.N -m "TeemoCode v0.1.N" $NEW`，然后 `git push -f ... v0.1.N` 覆盖旧 tag；这样 tag 不再指向旧历史，Gitee 才能 GC 旧对象。
7. **验证**（必做）：
   - 浅克隆 `git clone --depth 1` 检查只有 ~325MB（workdir 含几个 exe + git 对象）
   - `https://gitee.com/xiaotimor/teemo-code-update/releases/download/v保留/<保留文件名>` 仍要返回 200

⚠️ 2026-08-25 踩坑记录（行为确认）：
- **删 tag 不影响 release 附件**；force push tag 覆盖时附件也不受影响（附件是独立存储的）
- Gitee 的 `releases/download/` URL 只认 Release 附件，不认仓库文件——所以 exe 必须上传成 asset
- `releases/{id}/assets` 端点被 WAF 拦（404 HTML 页），要用 `releases/{id}/attach_files`

**参考命令**：

```bash
# === 1/2: 备份 + 新建干净仓库
cd C:/Users/12090/sdk
tar -czf mc-update-backup-$(date +%Y%m%d).tar.gz mc-update
rm -rf mc-update-clean && mkdir mc-update-clean && cd mc-update-clean && git init
cp ../mc-update/TeemoCode_0.1.1{4,5,6}_x64-setup.exe ../mc-update/TeemoCode_0.1.1{5,6}_x64-setup.exe.sig ../mc-update/latest.json .
# 0.1.14 无 sig(0.1.15 起才有),按实际保留版本调整
git add -A && git -c user.name=xiaotimor -c user.email=xiaotimor@users.noreply.gitee.com commit -m "release: 精简更新源,仅保留 v0.1.14/15/16"
NEW=$(git rev-parse HEAD)

# === 3/4: 删旧 release + 旧 tag（保留最新 3 个）
TOKEN=6413249386ee049a45469c7957b5d336
curl -s -X DELETE "https://gitee.com/api/v5/repos/xiaotimor/teemo-code-update/releases/<id>?access_token=$TOKEN"
git push https://gitee.com/xiaotimor/teemo-code-update.git :refs/tags/v0.1.X

# === 5: force push master
git push --force https://gitee.com/xiaotimor/teemo-code-update.git master

# === 6: 重建保留版本 tag 指向 NEW
git tag -a v0.1.14 -m "TeemoCode v0.1.14" $NEW
git tag -a v0.1.15 -m "TeemoCode v0.1.15" $NEW
git tag -a v0.1.16 -m "TeemoCode v0.1.16" $NEW
git push -f https://gitee.com/xiaotimor/teemo-code-update.git v0.1.14 v0.1.15 v0.1.16

# === 7: 验证
git clone --depth 1 https://gitee.com/xiaotimor/teemo-code-update.git size-check
du -sh size-check
curl -sI "https://gitee.com/xiaotimor/teemo-code-update/releases/download/v0.1.16/TeemoCode_0.1.16_x64-setup.exe"
```

---

## 4. 踩坑记录（每次发布必读）

### 4.1 签名：密码传输链上的转义地狱

**现象**：同一密码 `R3l3ase!K3y#2026x`，同样的 `npx @tauri-apps/cli@2 signer sign` 命令，
在不同调用方式下结果截然不同——昨天成功今天失败，或者换一种调用方式就失败。

**根因**：密码里的 `!`（bash 历史扩展）和 `#`（bash 注释符）在整条执行链的每一层都可能被转义：
- **Bash 工具**（Git Bash/MSYS）：`!` 触发 history expansion → 密码被截断 → 失败
- **.bat 脚本**（即使通过 Cmd 工具运行）：cmd.exe 的延迟展开机制（`setlocal EnableDelayedExpansion`）会破坏 `!`
- **Python subprocess 调用 cmd.exe**：传递参数时 Windows 的引号规则可能额外转义
- **Cmd 工具内联执行**（唯一可靠路径）：密码原样传给 cmd.exe → 传给 npx → 传给 tauri-cli → minisign 解密 → 成功

**铁律**：
```
签名命令必须在 Cmd 工具中直接敲，不通过 .bat / Python / Bash 工具中转。
命令格式（v0.1.21 验证通过）：
  cd /d D:\works\ziji\MonkeyCode\desktop && npx --yes @tauri-apps/cli@2 signer sign -f C:\Users\12090\sdk\mc-release-keys-new -p "R3l3ase!K3y#2026x" target\release\bundle\nsis\TeemoCode_<版本>_x64-setup.exe
```

### 4.2 Tauri 新命令：三处都要注册

**现象**：新增 IPC 命令后前端调用报 `Command not found` 或 `not allowed`。

**根因**：Tauri 的 IPC 权限是三层白名单，新增命令必须同时注册：
1. `main.rs` `generate_handler!()` → 运行时路由
2. `build.rs` 权限清单 → 编译时生成权限定义
3. `tauri.conf.json` + `tauri.debug.conf.json` → 前端调用授权

只改一处或两处都会导致「函数存在但前端调不通」的诡异错误。示例：`mc_models_list` 在 v0.1.20 加了 ①② 但漏了 ③，前端报 `not allowed`。

### 4.3 usage 事件：头尾双发导致统计翻倍

**现象**：用量统计面板的 token 数值远大于实际（今天 11 亿 → 修正后 5.8 亿）。

**根因**：provider 在同一次模型调用的**头**（message_start，input 已定）和**尾**（message_delta，output 定稿）各发一次 usage 事件，壳侧逐事件累加就记了两份。fold.rs 注释明确写了「后到者覆盖前值」但 stats.rs 的 record() 是累加语义。

**修复**：按会话记录上次入账的 `(input,output)` 完全相同视为重复跳过。
> stats.json 的历史数据无法自动修复——若出现过双记，需要写脚本把对应天的 input/output/calls 减半。

### 4.4 releaseHistory：旧版记录别放云端清单

**现象**：云端 latest.json 的 `history` 数组越滚越长，加载慢且历史记录会在发版时被挪走。

**方案**：历史版本记录内置到 `src/lib/ipc/releaseHistory.ts`（编译期嵌入），云端清单只保留最新一版的 `notes`。发版时在数组头部追加一条 `{version, notes}`，与 `Cargo.toml` 版本同步改。

### 4.5 指令队列补投 effect：turnStartedRef 守卫过严

**现象**：队列最后一条指令完成后卡在 `executing` 状态，永远不会出队。

**根因**：flush effect 的完成判断要求 `turnStartedRef=true`（只有在 effect 捕获到 `running=true` 时才置位）。当 `task-started` 与 `task-ended` 被 React 批量合并为一次渲染时，`running` 从未在 effect 里被设 true → `turnStartedRef` 永远 false → 完成块进不去。

**修复**：条件改为 `turnStartedRef.current || turnEnded`——当 `turnEnded=true` 时（task-ended 已到达）不论 `turnStartedRef` 状态如何都处理完成。

### 4.6 .bat 文件编码：中文注释导致 CP936 解析失败

**现象**：.bat 脚本里的中文注释在 Windows 下被 cmd 按 GBK/CP936 解析，乱码会导致语法错误或意外行为。

**方案**：.bat 脚本只用 ASCII/英文；Python 脚本用 `sys.stdout.reconfigure(encoding="utf-8", errors="replace")` 处理输出。

### 4.7 cargo build 被锁文件阻断

**现象**：`cargo build` 报 `failed to remove file: os error 5`（拒绝访问）。

**根因**：debug 版 TeemoCode 正在运行，Windows 锁住了 `.exe` 文件。`cargo build` 先删旧二进制再写新的，删不掉就报错。

**方案**：构建前先关闭正在运行的 debug 版 TeemoCode；或 `cargo build` 本身只是编译——被锁时只保留旧文件但不影响内部构建流程（`cargo build 2>&1 | grep Finished` 仍显示成功）。

