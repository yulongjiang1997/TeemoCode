# TeemoCode 桌面端发布指南 (Windows)

> 最后实战验证:v0.1.26(2026-08-30)。本文档是**实战验证过的完整流程**,直接照做即可。
> 旧文档中已失效的内容(手动 curl 步骤、仓库 git push 更新源等)已全部移除。

---

## 0. 当前发布体系速览

| 项 | 值 |
|---|---|
| 更新源 latest.json | `https://gitee.com/xiaotimor/teemo-code-update/raw/master/latest.json` |
| 下载 URL 模式 | `https://gitee.com/xiaotimor/teemo-code-update/releases/download/v0.1.22/TeemoCode_<版本>_x64-setup.exe` |
| **exe 附件挂载点** | **v0.1.22 的 Release**(仓库超配额,无法建新 tag/Release,exe 统一挂老 Release 下) |
| Gitee token | `6413249386ee049a45469c7957b5d336` |
| 签名私钥 | `C:\Users\12090\sdk\mc-release-keys-new` |
| 签名密码 | `R3l3ase!K3y#2026x` |
| 主仓库 | `D:\works\ziji\MonkeyCode`(branch: `wip-local`,remote: `fork` → GitHub) |
| 更新仓库工作区 | `C:\Users\12090\sdk\mc-update`(基本不再需要 git 操作,latest.json 走 API) |

**核心认知(为什么流程长这样):**
1. **Gitee 更新仓库已超配额(~1205MB > 1024MB)**:任何 git push(master/tag)都会被 pre-receive hook 拒绝。所以 exe 不能再 commit 进仓库,latest.json 也不能 git push 更新,只能走 **Gitee API**。
2. **`releases/download/<tag>/<file>` 只解析 Release 附件**:exe 必须上传成 Release asset。由于不能建新 Release,统一把新版本 exe 挂到 **v0.1.22 的 Release** 下(文件名带版本号,不会冲突)。
3. **latest.json 通过 Contents API 更新**,且**必须用 base64 编码方式上传**——直接传字符串内容会被 Gitee API 破坏(历史 3 次检查更新失败全是这个原因)。

---

## 1. 完整发布流程(实战验证版)

以下步骤按顺序执行。整个过程约 10-15 分钟(主要是 release 编译 5-8 分钟)。

### 步骤 1: 版本号升级

用 Python 脚本一次改三处(Cargo.toml / tauri.conf.json / releaseHistory.ts):

```python
# bump_version.py
import json, re
from pathlib import Path

VER = "0.1.27"  # 改成目标版本
DESKTOP = Path(r"D:\works\ziji\MonkeyCode\desktop")

cargo = DESKTOP / "Cargo.toml"
cargo.write_text(
    re.sub(r'^version = "[^"]+"', f'version = "{VER}"', cargo.read_text(encoding="utf-8"), count=1, flags=re.M),
    encoding="utf-8", newline=""
)

conf_path = DESKTOP / "tauri.conf.json"
conf = json.loads(conf_path.read_text(encoding="utf-8"))
conf["version"] = VER
conf_path.write_text(json.dumps(conf, ensure_ascii=False, indent=2) + "\n", encoding="utf-8", newline="")

hist = DESKTOP / "ui-next" / "src" / "lib" / "ipc" / "releaseHistory.ts"
src = hist.read_text(encoding="utf-8")
entry = '  {\n    version: "' + VER + '",\n    notes: "更新说明",\n  },\n'
anchor = "export const RELEASE_HISTORY: ReleaseNote[] = [\n"
src = src.replace(anchor, anchor + entry, 1)
hist.write_text(src, encoding="utf-8", newline="")
print(f"version -> {VER}")
```

### 步骤 2: 提交推送 + UI 构建 + Tauri 打包

一个 .bat 串起来(**.bat 里只用 ASCII**,中文注释会导致 CP936 解析错误):

```bat
@echo off
cd /d D:\works\ziji\MonkeyCode
git add desktop/Cargo.toml desktop/tauri.conf.json desktop/ui-next/src/lib/ipc/releaseHistory.ts
git commit -m "chore: bump version to 0.1.27"
git push fork wip-local
cd desktop\ui-next
call npm run build
cd ..
cmd /c package_windows.bat
```

产物: `desktop\target\release\bundle\nsis\TeemoCode_<版本>_x64-setup.exe`(约 51-66MB)

> 若 `git push` 报 `Unknown SSL protocol error`,重跑一次即可(GitHub 偶发,重试 1-3 次能过)。

### 步骤 3: 签名(⚠️ 全流程唯一正确的执行方式)

**必须在 Cmd 工具里内联执行,不要用 Bash 工具、不要通过 .bat 文件、不要用 Python subprocess 中转:**

```
工具: Cmd
命令: cd /d D:\works\ziji\MonkeyCode\desktop && npx --yes @tauri-apps/cli@2 signer sign -f C:\Users\12090\sdk\mc-release-keys-new -p "R3l3ase!K3y#2026x" target\release\bundle\nsis\TeemoCode_<版本>_x64-setup.exe
```

**为什么这么苛刻(多次踩坑实录):**
- 密码含 `!` 和 `#`:Bash 里 `!` 触发历史扩展、`#` 触发注释,bash 里必然失败;
- .bat 文件里 `!` 可能被 cmd 延迟展开破坏;
- Python `subprocess.run(..., shell=True)` 同样会破坏;
- **唯一可靠:Cmd 工具直接内联**。输出里 `Your file was signed successfully` + `Public signature:` 即成功,同时生成 `.sig` 文件。

### 步骤 4: 发布到 Gitee(一个 Python 脚本完成)

**这是最容易出错的环节,脚本必须包含以下全部要点**(实战验证模板,v0.1.26 验证通过):

```python
# publish.py — 发版后执行
import json, urllib.request, uuid, base64, subprocess, time
from datetime import datetime, timezone
from pathlib import Path

VER = "0.1.27"
TOKEN = "6413249386ee049a45469c7957b5d336"
API = "https://gitee.com/api/v5/repos/xiaotimor/teemo-code-update"
EXE = Path(rf"D:\works\ziji\MonkeyCode\desktop\target\release\bundle\nsis\TeemoCode_{VER}_x64-setup.exe")
SIG = Path(str(EXE) + ".sig")

# ---- 1) 找 v0.1.22 release(仓库超配额,所有版本统一挂这里) ----
rels = json.loads(urllib.request.urlopen(f"{API}/releases?access_token={TOKEN}", timeout=60).read().decode())
rel = next((x for x in rels if x["tag_name"] == "v0.1.22"), None)
print(f"Using v0.1.22 release id={rel['id']}")

# ---- 2) 上传 exe + sig(multipart,必须用 attach_files 端点) ----
for f in (EXE, SIG):
    b = uuid.uuid4().hex
    body = (f"--{b}\r\nContent-Disposition: form-data; name=\"access_token\"\r\n\r\n{TOKEN}\r\n"
            f"--{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{f.name}\"\r\n"
            f"Content-Type: application/octet-stream\r\n\r\n").encode() + f.read_bytes() + f"\r\n--{b}--\r\n".encode()
    req = urllib.request.Request(f"{API}/releases/{rel['id']}/attach_files", data=body,
        headers={"Content-Type": f"multipart/form-data; boundary={b}"}, method="POST")
    with urllib.request.urlopen(req, timeout=1800) as r:
        print(f"upload {f.name}: HTTP {r.status}")   # 期望 201

# ---- 3) 更新 latest.json:先改本地文件 → base64 编码上传(必须!) ----
lp = Path(r"C:\Users\12090\sdk\mc-update\latest.json")
data = json.loads(lp.read_text(encoding="utf-8"))
data["version"] = VER
data["notes"] = "更新说明"
data["pub_date"] = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
data["platforms"]["windows-x86_64"]["url"] = f"https://gitee.com/xiaotimor/teemo-code-update/releases/download/v0.1.22/TeemoCode_{VER}_x64-setup.exe"
data["platforms"]["windows-x86_64"]["signature"] = SIG.read_text(encoding="utf-8").strip()
data["history"] = [{"version": "0.1.26", "notes": "..."}, ...]  # 旧版本记录,最新在上
lp.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
local_content = lp.read_text(encoding="utf-8")
json.loads(local_content)  # 上传前先验证本地内容是合法 JSON

req = urllib.request.Request(f"{API}/contents/latest.json?access_token={TOKEN}")
with urllib.request.urlopen(req, timeout=30) as r:
    sha = json.loads(r.read().decode())['sha']   # PUT 更新必须带 sha
payload = {'access_token': TOKEN, 'sha': sha,
           'content': base64.b64encode(local_content.encode('utf-8')).decode(),
           'branch': 'master', 'message': f'update latest.json for v{VER}', 'encoding': 'base64'}
req = urllib.request.Request(f"{API}/contents/latest.json", data=json.dumps(payload).encode(),
    headers={'Content-Type': 'application/json'}, method='PUT')
urllib.request.urlopen(req, timeout=30)
print("latest.json uploaded (base64)")

# ---- 4) 强制验证 raw URL(不做这步 = 重演"检查更新失败"事故) ----
ok = False
for i in range(6):
    time.sleep(5)  # 等 CDN 缓存刷新
    try:
        with urllib.request.urlopen(f"https://gitee.com/xiaotimor/teemo-code-update/raw/master/latest.json?t={time.time()}", timeout=30) as r:
            parsed = json.loads(r.read())
        assert parsed["version"] == VER, f"version mismatch: {parsed['version']}"
        ok = True
        break
    except Exception as e:
        print(f"verify attempt {i+1} failed: {e}")
if not ok:
    raise SystemExit("raw URL verification FAILED - latest.json may be corrupted")

# ---- 5) 验证下载链接(200 且字节数一致) ----
req = urllib.request.Request(data["platforms"]["windows-x86_64"]["url"], method='HEAD')
with urllib.request.urlopen(req, timeout=30) as r:
    assert r.status == 200 and int(r.headers['Content-Length']) == EXE.stat().st_size

# ---- 6) 主仓库打 tag + 推送(GitHub SSL 偶发失败,重试 3 次) ----
subprocess.run(f'git tag -a v{VER} -m "TeemoCode_v{VER}"', cwd=r"D:\works\ziji\MonkeyCode", shell=True)
for i in range(3):
    r = subprocess.run("git push fork v" + VER, cwd=r"D:\works\ziji\MonkeyCode", shell=True, capture_output=True, text=True)
    if r.returncode == 0: break
    time.sleep(3)
r = subprocess.run("git push fork wip-local", cwd=r"D:\works\ziji\MonkeyCode", shell=True, capture_output=True, text=True)
print("wip-local push:", "ok" if r.returncode == 0 else r.stderr[-200:])

print(f"ALL CHECKS PASSED - v{VER} published")
```

脚本跑完打印 `ALL CHECKS PASSED` 才算发布成功。

### 步骤 5: 发布后独立复核(可选但推荐)

```python
# verify.py
import json, urllib.request, time
with urllib.request.urlopen(f"https://gitee.com/xiaotimor/teemo-code-update/raw/master/latest.json?t={time.time()}", timeout=30) as r:
    data = json.loads(r.read())
print(f"raw URL version: {data['version']}")
url = data['platforms']['windows-x86_64']['url']
req = urllib.request.Request(url, method='HEAD')
with urllib.request.urlopen(req, timeout=30) as r:
    print(f"download: HTTP {r.status}, {int(r.headers['Content-Length'])//1024//1024}MB")
```

---

## 2. 血泪坑实录(每条都真实发生过)

### 2.1 latest.json 内容损坏 → "检查更新失败"(踩了 3 次!)

**现象**:发版后客户端"检查更新失败",raw URL 返回乱码二进制。

**根因**:Gitee Contents API 直接传 `content` 字符串时,内容会被破坏(编码问题)。API 返回的内容和 raw URL 都变成乱码。

**唯一正确做法**:`PUT /contents/latest.json` 时**必须 base64 编码**(`content` 传 base64,`encoding` 传 `"base64"`)。并且**上传后必须回读 raw URL 验证 JSON 可解析、版本号正确**——不验证等于没发布。

### 2.2 签名密码被破坏 → "密码错误/私钥解密失败"

见步骤 3。结论:**Cmd 工具内联执行,其他任何方式都不可靠**。

### 2.3 Gitee 仓库超配额 → git push 被拒

**现象**:`! [remote rejected] master -> master (pre-receive hook declined)`,`Repo size: 1205MB, exceeds quota 1024MB`。

**现状与对策**:
- 任何 git push(master/tag)都会被拒;
- exe 统一挂现有稳定 Release 下(当前使用 **v0.1.32**,文件名带版本不冲突);
- latest.json 走 **Contents API** 更新;
- ~~filter-branch 重写历史瘦身~~ 也没用——push 本身就被拒,历史瘦身后的对象传不上去。除非升 Gitee 付费版或换更新源,否则现状就是终态。

### 2.4 `releases/{id}/assets` 被 WAF 拦

上传附件**必须用 `attach_files` 端点**(返回 201);`assets` 端点返回 404 HTML。

### 2.5 curl 不可靠

部分 Windows 环境 curl 对 Gitee 行为异常(exit 3/6)。**发布脚本统一用 Python urllib**,不要用 curl。

### 2.6 .bat 中文注释 → CP936 解析错误

.bat 里的中文注释被 cmd 按 GBK 解析会乱码导致语法错误。**.bat 只写 ASCII**;要写复杂逻辑用 Python。

### 2.7 cargo build 报 os error 5

debug 版 TeemoCode 正在运行锁住了 exe。**先关掉运行中的 TeemoCode 再构建**。

### 2.8 GitHub push 偶发 SSL 错误

`Unknown SSL protocol error in connection to github.com:443`——偶发,重试 1-3 次即过。

### 2.9 Tauri 新命令三处注册

新增 IPC 命令必须同时改:①`main.rs` generate_handler ②`build.rs` 命令清单 ③`tauri.conf.json` + `tauri.debug.conf.json` 的 ACL。漏任何一处,前端报 `Command not found` 或 `not allowed`。可用 `python scripts/check_command_contract.py` 自检。

### 2.10 Gitee 大文件 CDN 滞后

刚传完的 asset 下载 404 是正常的,等几十秒再验证(脚本里的重试循环已覆盖)。

### 2.11 0.1.34 签名报“密码错误”(2026-09-03)

**现象**: `tauri signer sign` 报 `incorrect updater private key password`，但密码本身没有更换。

**根因**:

1. `signer sign` 不能假设会用 `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` 环境变量代替 `-p`;只传环境变量时会进入错误的密码路径。
2. 如果同时存在空的 `TAURI_SIGNING_PRIVATE_KEY`，它会和 `-f/--private-key-path` 冲突；删除变量，不要把它设为空字符串。
3. Python 把整条命令交给 `cmd.exe /c` 时，路径再套引号会被 Tauri CLI 解析成错误的私钥/文件上下文，最终被误报成“密码错误”。密码中的 `!` 还可能被 cmd 的 delayed expansion 改写。

**固定做法**:

- 使用 `cmd.exe /d /v:off /c`，密码显式通过 `-p` 传入。
- 私钥使用 `-f`，同时确保 `TAURI_SIGNING_PRIVATE_KEY` 和 `TAURI_SIGNING_PRIVATE_KEY_PATH` 未设置。
- 路径统一使用 `/`;当前发布路径不含空格时，`/c` 命令里的路径不要再套引号。发布脚本遇到含空格路径会直接提示，不要自行加引号绕过。
- 签名后检查 `.sig` 存在，并核对签名使用的公钥与 `tauri.conf.json` 一致；再做远端下载、字节数、SHA-256 和签名回读校验。

已将上述规则固化到 `scripts/release_v0_1_x.py`，后续发布直接使用脚本，不要手工重新拼接签名命令。

### 2.12 Gitee 配额下的 0.1.34 发布

更新仓库已超过配额，不能再依赖 `git push` 发布安装包或 `latest.json`。本次使用现有 `v0.1.32` Release 的 `attach_files` 接口上传 0.1.34 的 exe/sig，再用 Contents API 以 **base64** 更新 `latest.json`；最后必须从 raw URL 和 Release 下载地址做端到端校验。新版本不应为了附件创建新的 tag 或直接向更新仓库推送大文件。

---

## 3. 版本记录速查

| 版本 | 主要内容 |
|---|---|
| 0.1.34 | 工作区切换任务卡顿修复 + 发布签名命令修复 |
| 0.1.26 | Git 技能库导入 + 大模型解析(并行) + 摘要标签持久化 + 版本化 base_url 修复 |
| 0.1.25 | 指令仓库 + confirm 崩溃修复 + 队列持久化 + 团队注入 |
| 0.1.24 | 团队模式编排注入 + 队列弹窗 + GIF 桌宠动画 |
| 0.1.23 | GIF 动画支持 + 桌宠尺寸修复 + 配置持久化 |
| 0.1.22 | 模型下拉 + 队列修复(**exe 统一挂这个 Release**) |
