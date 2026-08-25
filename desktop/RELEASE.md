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
> 表现为"密码错误/私钥解密失败"（2026-08-25 发 v0.1.16 踩过：同样的密码在 bash 里失败、cmd 里成功）。
> 验证签名后 .sig 第一行 base64 解码应含 `signature from tauri secret key`。

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
