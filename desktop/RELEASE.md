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

### 3.3 更新 Gitee 更新源
1. 克隆更新仓库：`git clone https://gitee.com/xiaotimor/teemo-code-update.git`
2. 把 `TeemoCode_<版本>_x64-setup.exe` 和 `.sig` 复制进去。
3. 改 `latest.json`：
   - `version` → 新版本号
   - `notes` → 本次更新说明（中文）
   - `pub_date` → 当前 UTC 时间（`2026-08-21T...Z`）
   - `platforms.windows-x86_64.signature` → `.sig` 文件的**完整内容**（单行 base64）
   - `platforms.windows-x86_64.url` → `https://gitee.com/xiaotimor/teemo-code-update/releases/download/v<版本>/TeemoCode_<版本>_x64-setup.exe`
   - `history` → 保留旧版本（0.1.1~上一个版本）的记录数组
4. commit + push 到 Gitee `master`，并打 tag `v<版本>`：
```bat
git add -A && git commit -m "release: v<版本> ..." && git push origin master
git tag -a v<版本> -m "TeemoCode v<版本>: ..." && git push origin v<版本>
```

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

- Gitee `teemo-code-update` 仓库已超 800MB（每次发版直接 commit 56MB 安装包进历史）。建议后续迁移到 Git LFS，或只保留最近 2~3 个版本，否则会被 Gitee 限流/禁止推送。
- `.sig` 文件是**单行** base64；`latest.json` 的 `signature` 字段填其完整内容（不要手动换行）。
- 打包依赖：`binaries/` 下需有 `ohmyagent-x86_64-pc-windows-msvc.exe`（engine sidecar）、`browser-extension/dist`（前端扩展）、NSIS 已安装。缺资源时 `make windows` 会报错，可参考 `Makefile` 的 `check-bundle-configs` / `engine-windows` / `browser-extension` 目标补齐全。
