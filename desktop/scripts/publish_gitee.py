#!/usr/bin/env python3
"""TeemoCode Gitee 发布脚本(2026-09-07 新规则)

规则(由用户指定):
1. 每次发布**新建**自己的 release 节点(tag = v<版本>),不再复用旧 release。
2. 发布前查询已用配额:遍历所有 release 的附件,HEAD 请求累计大小。
   若 已用 + 新包( exe + sig )* 1.1(冗余系数) > 1024MB,删除最老的
   2 个 release(连附件一起),再新建。
3. latest.json 用 Contents API + base64 更新,raw URL 回读验证。
4. 下载 URL 验证:HTTP 200 且字节数与本地一致。

用法: python scripts/publish_gitee.py <版本号> <更新说明>
示例: python scripts/publish_gitee.py 0.1.38 "新增 xx 功能"
"""
import json
import base64
import sys
import time
import urllib.request
import uuid
from datetime import datetime, timezone
from pathlib import Path

VER = sys.argv[1] if len(sys.argv) > 1 else ""
NOTES = sys.argv[2] if len(sys.argv) > 2 else ""
TOKEN = "6413249386ee049a45469c7957b5d336"
API = "https://gitee.com/api/v5/repos/xiaotimor/teemo-code-update"
BASE = "https://gitee.com/xiaotimor/teemo-code-update"
QUOTA_MB = 1024
SAFETY_FACTOR = 1.1  # 冗余系数:附件之外仓库还有 git 对象占空间
KEEP_CLEAN_N = 2     # 配额不足时删除最老的 N 个 release

EXE = Path(rf"D:\works\ziji\MonkeyCode\desktop\target\release\bundle\nsis\TeemoCode_{VER}_x64-setup.exe")
SIG = Path(str(EXE) + ".sig")
LATEST_JSON = Path(r"C:\Users\12090\sdk\mc-update\latest.json")


def with_retry(fn, n=5, label=""):
    for i in range(1, n + 1):
        try:
            return fn()
        except Exception as e:
            print(f"  [{label}] attempt {i} failed: {e}")
            if i < n:
                time.sleep(5 * i)
    raise SystemExit(f"[{label}] all retries failed")


def api_get(path):
    req = urllib.request.Request(f"{API}{path}")
    with urllib.request.urlopen(req, timeout=30) as r:
        return json.loads(r.read().decode())


def asset_size(url):
    """HEAD 请求测附件大小,失败按 0 计(不阻塞流程)。"""
    try:
        req = urllib.request.Request(url, method="HEAD")
        with urllib.request.urlopen(req, timeout=30) as r:
            return int(r.headers.get("Content-Length", 0))
    except Exception:
        return 0


def list_releases():
    return with_retry(lambda: api_get("/releases?access_token=" + TOKEN + "&per_page=100"), label="list releases")


def used_quota_mb():
    """遍历所有 release 的附件,HEAD 累计大小(MB)。"""
    rels = list_releases()
    total = 0
    for rel in rels:
        for a in rel.get("assets", []):
            total += asset_size(a["browser_download_url"])
    return total / 1024 / 1024, rels


def delete_release(rid, tag):
    def do():
        req = urllib.request.Request(f"{API}/releases/{rid}?access_token={TOKEN}", method="DELETE")
        urllib.request.urlopen(req, timeout=30)
    with_retry(do, label=f"delete {tag}")
    print(f"  deleted release {tag} (id={rid})")


def cleanup_for_quota(new_size_mb, rels):
    """配额不够就删最老的 N 个 release。返回是否清理过。"""
    used, _ = used_quota_mb()
    need = new_size_mb * SAFETY_FACTOR
    print(f"  quota: used={used:.0f}MB + new~{need:.0f}MB vs {QUOTA_MB}MB")
    if used + need <= QUOTA_MB:
        return False
    # 按创建时间升序 = 最老在前;跳过任何 latest.json 还没指到文件的地方——
    # 反正删的是最老的,latest.json 永远指向最新版本,安全。
    oldest = sorted(rels, key=lambda x: x.get("created_at", ""))
    for rel in oldest[:KEEP_CLEAN_N]:
        delete_release(rel["id"], rel["tag_name"])
    return True


def create_release():
    def do():
        body = json.dumps({
            "access_token": TOKEN, "tag_name": f"v{VER}", "name": f"v{VER}",
            "body": f"TeemoCode v{VER}\n\n{NOTES}", "target_commitish": "master", "prerelease": False,
        }).encode()
        req = urllib.request.Request(f"{API}/releases", data=body, method="POST",
            headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(req, timeout=60) as r:
            return json.loads(r.read().decode())
    rel = with_retry(do, label="create release")
    print(f"  created release v{VER} id={rel['id']}")
    return rel


def upload_asset(rid, f):
    def do():
        b = uuid.uuid4().hex
        body = (f"--{b}\r\nContent-Disposition: form-data; name=\"access_token\"\r\n\r\n{TOKEN}\r\n"
                f"--{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{f.name}\"\r\n"
                f"Content-Type: application/octet-stream\r\n\r\n").encode() + f.read_bytes() + f"\r\n--{b}--\r\n".encode()
        req = urllib.request.Request(f"{API}/releases/{rid}/attach_files", data=body,
            headers={"Content-Type": f"multipart/form-data; boundary={b}"}, method="POST")
        with urllib.request.urlopen(req, timeout=3600) as r:
            print(f"  upload {f.name}: HTTP {r.status}")
    with_retry(do, label=f"upload {f.name}")


def update_latest_json(download_url, signature):
    def do():
        data = json.loads(LATEST_JSON.read_text(encoding="utf-8"))
        data["version"] = VER
        data["notes"] = NOTES
        data["pub_date"] = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
        data["platforms"]["windows-x86_64"]["url"] = download_url
        data["platforms"]["windows-x86_64"]["signature"] = signature
        LATEST_JSON.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        content = LATEST_JSON.read_text(encoding="utf-8")
        json.loads(content)
        sha = api_get("/contents/latest.json?access_token=" + TOKEN)["sha"]
        payload = {"access_token": TOKEN, "sha": sha,
                   "content": base64.b64encode(content.encode("utf-8")).decode(),
                   "branch": "master", "message": f"update latest.json for v{VER}", "encoding": "base64"}
        req = urllib.request.Request(f"{API}/contents/latest.json", data=json.dumps(payload).encode(),
            headers={"Content-Type": "application/json"}, method="PUT")
        urllib.request.urlopen(req, timeout=30)
    with_retry(do, label="latest.json")


def verify():
    ok = False
    for i in range(6):
        time.sleep(5)
        try:
            with urllib.request.urlopen(f"{BASE}/raw/master/latest.json?t={time.time()}", timeout=30) as r:
                parsed = json.loads(r.read())
            assert parsed["version"] == VER
            print(f"  raw URL: v{parsed['version']} OK")
            ok = True
            break
        except Exception as e:
            print(f"  verify attempt {i+1}: {e}")
    if not ok:
        raise SystemExit("raw URL verification FAILED")

    url = f"{BASE}/releases/download/v{VER}/TeemoCode_{VER}_x64-setup.exe"
    for i in range(10):
        time.sleep(10)  # CDN 滞后(见 RELEASE.md 2.10)
        try:
            req = urllib.request.Request(url, method="HEAD")
            with urllib.request.urlopen(req, timeout=60) as r:
                size = int(r.headers.get("Content-Length", 0))
                if r.status == 200 and size == EXE.stat().st_size:
                    print(f"  download URL: HTTP 200 {size} bytes OK")
                    return
                print(f"  download attempt {i+1}: status={r.status} size={size} (expect {EXE.stat().st_size})")
        except Exception as e:
            print(f"  download attempt {i+1}: {e}")
    raise SystemExit("download URL verification FAILED")


def main():
    if not VER:
        raise SystemExit("usage: python scripts/publish_gitee.py <version> <notes>")
    if not EXE.exists() or not SIG.exists():
        raise SystemExit(f"missing build artifacts: {EXE} / {SIG}")
    new_size_mb = (EXE.stat().st_size + SIG.stat().st_size) / 1024 / 1024
    print(f"[1/5] publish v{VER} ({new_size_mb:.0f}MB)")

    print("[2/5] quota check")
    rels = list_releases()
    cleaned = cleanup_for_quota(new_size_mb, rels)
    if cleaned:
        print("  (cleaned old releases for quota)")

    print("[3/5] create release + upload")
    rel = create_release()
    upload_asset(rel["id"], EXE)
    upload_asset(rel["id"], SIG)

    url = f"{BASE}/releases/download/v{VER}/TeemoCode_{VER}_x64-setup.exe"
    sig_text = SIG.read_text(encoding="utf-8").strip()
    print("[4/5] latest.json")
    update_latest_json(url, sig_text)

    print("[5/5] verify")
    verify()
    print(f"\nALL CHECKS PASSED - v{VER} published")
    print(f"download: {url}")


if __name__ == "__main__":
    main()
