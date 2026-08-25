# TeemoCode v0.1.x 发布脚本(Windows)
#
# 一键执行完整发布流程,对应 desktop/RELEASE.md 的手动步骤:
#   1. 版本号升级(Cargo.toml + tauri.conf.json + Cargo.lock)
#   2. 提交版本号提交
#   3. UI 构建(ui-next: tsc + vite build)
#   4. Tauri Release 打包(NSIS 安装包)
#   5. 签名(必须在 cmd.exe 里跑——密码含 !/#,bash 会转义坏)
#   6. Gitee 更新源推送(master: latest.json + exe + sig)
#   7. Gitee Release 创建(tag 先推,target_commitish 必须完整 hash)
#   8. Release assets 上传(exe/sig,attach_files 端点)
#   9. 端到端验证(latest.json 可读/签名一致/exe 下载 200 且字节数一致)
#  10. 主仓库打 tag 并推送
#
# 用法(cmd.exe):
#   python scripts\release_v0_1_x.py --version 0.1.18
#   python scripts\release_v0_1_x.py --version 0.1.18 --skip-build   # 复用已有产物
#   python scripts\release_v0_1_x.py --version 0.1.18 --notes "更新说明"
#
# 前置:
#   - 已在 wip-local 分支且工作区干净
#   - 私钥 C:\Users\12090\sdk\mc-release-keys-new 存在
#   - 更新源工作区 C:\Users\12090\sdk\mc-update 存在(或可 clone)

import argparse
import json
import os
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from datetime import datetime, timezone
from pathlib import Path

sys.stdout.reconfigure(encoding="utf-8", errors="replace")
sys.stderr.reconfigure(encoding="utf-8", errors="replace")

# ---------- 固定配置(变更频率低;密码不入库,见 RELEASE.md 安全提醒) ----------
REPO = Path(__file__).resolve().parent.parent.parent  # scripts/ → desktop/ → 仓库根
DESKTOP = REPO / "desktop"
BUNDLE_DIR = DESKTOP / "target" / "release" / "bundle" / "nsis"
UI_DIR = DESKTOP / "ui-next"

PRIVATE_KEY = Path(r"C:\Users\12090\sdk\mc-release-keys-new")
SIGN_PASSWORD = os.environ.get("TAURI_SIGNING_PRIVATE_KEY_PASSWORD", "")
UPDATE_REPO_WORKDIR = Path(r"C:\Users\12090\sdk\mc-update")
UPDATE_REPO_URL = "https://gitee.com/xiaotimor/teemo-code-update.git"
GITEE_API = "https://gitee.com/api/v5/repos/xiaotimor/teemo-code-update"
GITEE_TOKEN = os.environ.get("GITEE_UPDATE_TOKEN", "")
MAIN_REPO_REMOTE = "fork"
MAIN_REPO_BRANCH = "wip-local"


def run(cmd, cwd=None, capture=False, check=True):
    """执行命令;capture=True 时返回 stdout(文本)。"""
    print(f"  $ {cmd if isinstance(cmd, str) else subprocess.list2cmdline(cmd)}")
    r = subprocess.run(
        cmd,
        cwd=cwd or str(REPO),
        shell=isinstance(cmd, str),
        capture_output=capture,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if check and r.returncode != 0:
        if capture:
            print(r.stdout[-2000:])
            print(r.stderr[-2000:])
        raise SystemExit(f"命令失败(exit {r.returncode}): {cmd}")
    return r.stdout.strip() if capture else ""


def step(n, total, msg):
    print(f"\n=== [{n}/{total}] {msg} ===")


def main():
    ap = argparse.ArgumentParser(description="TeemoCode v0.1.x 一键发布")
    ap.add_argument("--version", required=True, help="新版本号,如 0.1.18")
    ap.add_argument("--notes", default="", help="更新说明(缺省用通用文案)")
    ap.add_argument("--skip-build", action="store_true", help="跳过构建/签名(复用已有产物)")
    ap.add_argument("--skip-push-main", action="store_true", help="跳过主仓库代码推送")
    args = ap.parse_args()

    ver = args.version
    exe_name = f"TeemoCode_{ver}_x64-setup.exe"
    exe_path = BUNDLE_DIR / exe_name
    sig_path = Path(str(exe_path) + ".sig")

    if not SIGN_PASSWORD:
        raise SystemExit("错误: 缺环境变量 TAURI_SIGNING_PRIVATE_KEY_PASSWORD(签名私钥密码)")
    if not GITEE_TOKEN:
        raise SystemExit("错误: 缺环境变量 GITEE_UPDATE_TOKEN(Gitee 更新源 access_token)")

    # ---------- 0. 前置检查 ----------
    step(0, 10, "前置检查")
    if not PRIVATE_KEY.exists():
        raise SystemExit(f"错误: 私钥不存在: {PRIVATE_KEY}")
    branch = run("git rev-parse --abbrev-ref HEAD", capture=True)
    if branch != MAIN_REPO_BRANCH:
        raise SystemExit(f"错误: 当前分支 {branch},需在 {MAIN_REPO_BRANCH}")
    dirty = run("git status --porcelain", capture=True)
    if dirty and not args.skip_push_main:
        raise SystemExit(f"错误: 工作区不干净:\n{dirty}")
    print("OK")

    total = 10

    # ---------- 1. 版本号升级 ----------
    step(1, total, f"版本号升级 → {ver}")
    import re

    cargo = DESKTOP / "Cargo.toml"
    tauri_conf = DESKTOP / "tauri.conf.json"
    cargo.write_text(
        re.sub(r'^version = "[^"]+"', f'version = "{ver}"', cargo.read_text(encoding="utf-8"), count=1, flags=re.M),
        encoding="utf-8",
    )
    conf = json.loads(tauri_conf.read_text(encoding="utf-8"))
    conf["version"] = ver
    tauri_conf.write_text(json.dumps(conf, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    run(["cargo", "update", "--workspace"], cwd=str(DESKTOP), check=False)  # 刷 Cargo.lock
    print(f"OK (Cargo.toml / tauri.conf.json / Cargo.lock → {ver})")

    # ---------- 2. 提交版本号 ----------
    step(2, total, "提交版本号提交")
    run(["git", "add", "desktop/Cargo.toml", "desktop/tauri.conf.json", "desktop/Cargo.lock"])
    commit_msg = f"chore: bump version to {ver}"
    run(["git", "commit", "-m", commit_msg], check=False)
    print("OK")

    # ---------- 3. UI 构建 ----------
    if not args.skip_build:
        step(3, total, "UI 构建(ui-next)")
        run("npm ci || true", cwd=str(UI_DIR), check=False)
        run("npm run build", cwd=str(UI_DIR))
        print("OK")
    else:
        print("\n=== [3/10] UI 构建 — 跳过(--skip-build) ===")

    # ---------- 4. Tauri 打包 ----------
    if not args.skip_build:
        step(4, total, "Tauri Release 打包(NSIS)")
        bat = DESKTOP / "package_windows.bat"
        run(str(bat), cwd=str(DESKTOP))
    else:
        print("\n=== [4/10] Tauri 打包 — 跳过(--skip-build) ===")
    if not exe_path.exists():
        raise SystemExit(f"错误: 安装包不存在: {exe_path}")
    print(f"安装包就绪: {exe_path}")

    # ---------- 5. 签名(必须 cmd.exe)----------
    step(5, total, "签名(cmd.exe 内执行,bash 会转义坏密码)")
    sign_cmd = (
        f'npx --yes @tauri-apps/cli@2 signer sign '
        f'-f "{PRIVATE_KEY}" '
        f'-p "{SIGN_PASSWORD}" '
        f'"{exe_path}"'
    )
    r = subprocess.run(["cmd.exe", "/c", sign_cmd], cwd=str(DESKTOP), capture_output=True, text=True, encoding="utf-8", errors="replace")
    if r.returncode != 0 or not sig_path.exists():
        print(r.stdout[-1500:], r.stderr[-1500:])
        raise SystemExit("签名失败")
    print(f"签名 OK: {sig_path.name} ({sig_path.stat().st_size} bytes)")

    # ---------- 6. Gitee master 推送 ----------
    step(6, total, "Gitee 更新源 master 推送")
    if not UPDATE_REPO_WORKDIR.exists():
        run(["git", "clone", "--depth", "1", UPDATE_REPO_URL, str(UPDATE_REPO_WORKDIR)])
    else:
        run(["git", "fetch", UPDATE_REPO_URL, "master:refs/remotes/gitee/master"], cwd=str(UPDATE_REPO_WORKDIR))
        run(["git", "reset", "--hard", "gitee/master"], cwd=str(UPDATE_REPO_WORKDIR))
    for src in (exe_path, sig_path):
        run(["cmd.exe", "/c", f'copy /Y "{src}" "{UPDATE_REPO_WORKDIR}\\"'])
    gen_latest(ver, args.notes, UPDATE_REPO_WORKDIR)
    run(["git", "add", "-A"], cwd=str(UPDATE_REPO_WORKDIR))
    run(["git", "commit", "-m", f"release_{ver}"], cwd=str(UPDATE_REPO_WORKDIR), check=False)
    update_head = run(["git", "rev-parse", "HEAD"], cwd=str(UPDATE_REPO_WORKDIR), capture=True)
    run(["git", "push", UPDATE_REPO_URL, "master"], cwd=str(UPDATE_REPO_WORKDIR))
    print(f"OK (head={update_head[:8]})")

    # ---------- 7+8. Gitee Release + assets ----------
    step(7, total, "Gitee Release 创建 + assets 上传")
    publish_release(update_head, ver, UPDATE_REPO_WORKDIR, exe_path, sig_path)

    # ---------- 9. 端到端验证 ----------
    step(9, total, "端到端验证")
    verify(ver, sig_path.stat().st_size, exe_path)
    print("OK")

    # ---------- 10. 主仓库 tag ----------
    step(10, total, "主仓库 tag 推送")
    run(["git", "tag", "-a", f"v{ver}", "-m", f"TeemoCode_v{ver}"], check=False)
    run(["git", "push", MAIN_REPO_REMOTE, f"v{ver}"])
    if not args.skip_push_main:
        run(["git", "push", MAIN_REPO_REMOTE, MAIN_REPO_BRANCH])
    print("OK")

    print("\n" + "=" * 50)
    print(f"v{ver} 发布完成!下载地址:")
    print(f"  https://gitee.com/xiaotimor/teemo-code-update/releases/download/v{ver}/{exe_name}")
    print("=" * 50)


# ---------- latest.json 生成(表单/文件操作,纯本地) ----------
def gen_latest(ver, notes, workdir: Path):
    manifest_path = workdir / "latest.json"
    data = json.loads(manifest_path.read_text(encoding="utf-8"))
    sig = (workdir / f"TeemoCode_{ver}_x64-setup.exe.sig").read_text(encoding="utf-8").strip()
    old = {"version": data["version"], "notes": data["notes"]}
    new = {
        "version": ver,
        "notes": notes or f"TeemoCode v{ver}",
        "pub_date": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "platforms": {
            "windows-x86_64": {
                "signature": sig,
                "url": f"{URL_BASE_RELEASE(ver)}",
            }
        },
        "history": [old] + data.get("history", []),
    }
    manifest_path.write_text(json.dumps(new, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    chk = json.loads(manifest_path.read_text(encoding="utf-8"))
    assert chk["version"] == ver and chk["platforms"]["windows-x86_64"]["signature"] == sig
    print(f"latest.json OK(version={ver}, history={len(new['history'])})")


def URL_BASE_RELEASE(ver):
    return (
        f"https://gitee.com/xiaotimor/teemo-code-update/releases/download/"
        f"v{ver}/TeemoCode_{ver}_x64-setup.exe"
    )


# ---------- Gitee API(requests 优先,curl 在该环境下不稳定) ----------
def gitee_post_form(path, payload):
    import requests

    payload = dict(payload, access_token=GITEE_TOKEN)
    r = requests.post(f"{GITEE_API}{path}", params={"access_token": GITEE_TOKEN}, data=payload, timeout=60)
    return r


def gitee_list_releases():
    import requests

    r = requests.get(f"{GITEE_API}/releases", params={"access_token": GITEE_TOKEN}, timeout=60)
    r.raise_for_status()
    return r.json()


def publish_release(update_head, ver, workdir: Path, exe_path, sig_path):
    import requests

    token_p = {"access_token": GITEE_TOKEN}
    rels = gitee_list_releases()
    rel = next((x for x in rels if x["tag_name"] == f"v{ver}"), None)

    if rel is None:
        # tag 必须先推,否则 release 创建报「创建标签失败」
        run(["git", "tag", "-a", f"v{ver}", "-m", f"release_{ver}"], cwd=str(workdir), check=False)
        run(["git", "push", UPDATE_REPO_URL, f"v{ver}"], cwd=str(workdir), check=False)
        # target_commitish 必填且需完整 hash(短 hash 报 body is missing 家族错误)
        full_hash = run(["git", "rev-parse", "HEAD"], cwd=str(workdir), capture=True)
        r = gitee_post_form(
            "/releases",
            {
                "tag_name": f"v{ver}",
                "name": f"v{ver}",
                "body": f"TeemoCode v{ver}",
                "target_commitish": full_hash,
                "prerelease": "false",
            },
        )
        if r.status_code >= 300:
            raise SystemExit(f"创建 release 失败: {r.status_code} {r.text[:300]}")
        rel = r.json()
        print(f"release 创建成功 id={rel['id']}")
    else:
        print(f"release 已存在 id={rel['id']},复用")

    url = f"{GITEE_API}/releases/{rel['id']}/attach_files"
    for f in (exe_path, sig_path):
        with open(f, "rb") as fh:
            r = requests.post(url, params=token_p, files={"file": (f.name, fh)}, timeout=1800)
        if r.status_code >= 300:
            raise SystemExit(f"上传附件失败 {f.name}: {r.status_code} {r.text[:300]}")
        print(f"上传 OK: {f.name} ({r.json().get('size')} bytes)")


def verify(ver, local_sig_len_unused, exe_path):
    local_sig = (exe_path.parent / (exe_path.name + ".sig")).read_text(encoding="utf-8").strip()
    with urllib.request.urlopen("https://gitee.com/xiaotimor/teemo-code-update/raw/master/latest.json") as r:
        latest = json.loads(r.read().decode())
    assert latest["version"] == ver, f"latest version={latest['version']}"
    remote = latest["platforms"]["windows-x86_64"]
    assert remote["signature"] == local_sig, "signature mismatch"
    req = urllib.request.Request(remote["url"], method="HEAD")
    with urllib.request.urlopen(req, timeout=120) as r:
        size = int(r.headers.get("Content-Length", 0))
        assert r.status == 200, f"HTTP {r.status}"
    assert size == exe_path.stat().st_size, f"size mismatch remote={size}"
    print(f"验证通过: version={ver}, signature match, download 200 ({size} bytes)")


if __name__ == "__main__":
    main()
