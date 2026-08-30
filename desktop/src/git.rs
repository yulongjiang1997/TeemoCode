use serde_json::json;
use std::path::Path;
use std::process::Command;
use tauri::Manager;

use crate::config::config_dir;

/// Git 上传/导入(本地任务工作目录 ↔ 远程仓库)。
/// 依赖系统 git 命令;身份统一用 TeemoCode 本地配置(不改用户全局 git 配置)。

fn git(dir: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| format!("无法执行 git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if err.is_empty() {
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        } else {
            err
        })
    }
}

fn has_git(dir: &str) -> bool {
    std::path::Path::new(dir).join(".git").exists()
}

/// 当前仓库的远程地址(第一个);无则 None。
fn current_remote(dir: &str) -> Option<String> {
    git(dir, &["remote", "get-url", "origin"]).ok().filter(|s| !s.is_empty())
}

fn commit_identity(dir: &str) -> Result<(), String> {
    git(dir, &["-c", "user.name=TeemoCode", "-c", "user.email=teemocode@local", "commit", "-m", "TeemoCode: 本地任务更新"]).map(|_| ())
}

/// 上传:工作目录有 .git → 直接提交推送;没有 → init + 提交 + (给远程地址则)推送。
/// remote_url 可选:目录已有 origin 就用它,否则用传入地址;都没有则只提交不推送。
#[tauri::command]
pub async fn git_push(app: tauri::AppHandle, workdir: String, remote_url: Option<String>) -> Result<serde_json::Value, String> {
    let dir = workdir.trim();
    if dir.is_empty() {
        return Err("工作目录为空".into());
    }
    if !std::path::Path::new(dir).exists() {
        return Err(format!("工作目录不存在: {dir}"));
    }
    // 把绑定当前工作目录的任务会话数据导出到项目 .teemocode/,随代码一起提交推送
    let _ = export_task_data(&app, dir);
    if !has_git(dir) {
        git(dir, &["init"]).map_err(|e| format!("git init 失败: {e}"))?;
    }
    git(dir, &["add", "-A"]).map_err(|e| format!("git add 失败: {e}"))?;
    // 有改动才提交(避免"nothing to commit"报错)
    let changed = git(dir, &["status", "--porcelain"])
        .map(|s| !s.is_empty())
        .unwrap_or(true);
    if changed {
        commit_identity(dir).map_err(|e| format!("git commit 失败: {e}"))?;
    }
    // 远程:已有 origin 优先;否则用传入地址
    let remote = current_remote(dir).or(remote_url.filter(|r| !r.trim().is_empty()));
    let mut pushed = false;
    let branch = git(dir, &["branch", "--show-current"]).unwrap_or_default();
    if let Some(url) = remote.as_deref() {
        if git(dir, &["remote", "get-url", "origin"]).is_err() {
            git(dir, &["remote", "add", "origin", url]).map_err(|e| format!("添加远程失败: {e}"))?;
        }
        let push_branch = if branch.is_empty() { "master" } else { &branch };
        git(dir, &["push", "-u", "origin", push_branch]).map_err(|e| format!("git push 失败: {e}"))?;
        pushed = true;
    }
    let hash = git(dir, &["rev-parse", "--short", "HEAD"]).ok();
    Ok(json!({
        "ok": true,
        "pushed": pushed,
        "remote": remote,
        "branch": if branch.is_empty() { "master".to_string() } else { branch },
        "commit": hash,
    }))
}

/// 把绑定该工作目录的任务会话数据导出到项目 `.teemocode/`,供 git 一起提交。
/// 覆盖三处数据目录(与导入原版 monkeycode 一致):
///   ohmy-sessions/<sid>/       壳侧会话(meta.json + events.jsonl)
///   ohmyagent/sessions/<sid>/  引擎对话(messages.jsonl)
///   ohmyagent/tasks/<sid>/     任务状态(tasks.json + notifications.json)
fn export_task_data(app: &tauri::AppHandle, workdir: &str) -> Result<usize, String> {
    let root = config_dir(app)?;
    let sid_list = bound_sids(app, workdir);
    let mut exported = 0usize;
    for sid in &sid_list {
        for rel in ["ohmy-sessions", "ohmyagent/sessions", "ohmyagent/tasks"] {
            let src = root.join(rel).join(sid);
            if !src.is_dir() {
                continue;
            }
            let dest = Path::new(workdir).join(".teemocode").join(rel).join(sid);
            if !dest.is_dir() {
                std::fs::create_dir_all(&dest).map_err(|e| format!("创建导出目录失败: {e}"))?;
            }
            if let Ok(files) = std::fs::read_dir(&src) {
                for f in files.flatten() {
                    let name = f.file_name().to_string_lossy().to_string();
                    let _ = std::fs::copy(f.path(), dest.join(&name));
                }
            }
        }
        exported += 1;
    }
    Ok(exported)
}

/// 绑定某工作目录的会话 sid 集合(壳侧 ohmy-sessions 的 meta.workdir 匹配)。
fn bound_sids(app: &tauri::AppHandle, workdir: &str) -> Vec<String> {
    let Ok(root) = config_dir(app) else { return vec![] };
    let shell = root.join("ohmy-sessions");
    let mut out = vec![];
    let Ok(entries) = std::fs::read_dir(&shell) else { return out };
    for e in entries.flatten() {
        let sid = e.file_name().to_string_lossy().to_string();
        let meta_p = e.path().join("meta.json");
        if !meta_p.is_file() {
            continue;
        }
        if let Ok(meta) = read_json(&meta_p) {
            let wd = meta.get("workdir").and_then(|v| v.as_str()).unwrap_or("");
            if wd == workdir {
                out.push(sid);
            }
        }
    }
    out
}

/// 导入项目后检测任务数据:项目内 `.teemocode/` 三目录视为导出的本地任务
/// 会话数据,迁移到配置目录(本地任务工作区)并把 workdir 改成当前目录。
/// 导入:按 git 地址加载到工作目录并拉取代码。
#[tauri::command]
pub async fn git_import(workdir: String, url: String) -> Result<serde_json::Value, String> {
    let dir = workdir.trim();
    let url = url.trim();
    if dir.is_empty() {
        return Err("工作目录为空".into());
    }
    if url.is_empty() {
        return Err("git 地址为空".into());
    }
    if !std::path::Path::new(dir).exists() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建工作目录失败: {e}"))?;
    }
    let empty = std::fs::read_dir(dir).map(|mut it| it.next().is_none()).unwrap_or(true);
    if has_git(dir) {
        git(dir, &["pull"]).map_err(|e| format!("git pull 失败: {e}"))?;
    } else if empty {
        git(dir, &["clone", url, "."]).map_err(|e| format!("git clone 失败: {e}"))?;
    } else {
        git(dir, &["init"]).map_err(|e| format!("git init 失败: {e}"))?;
        git(dir, &["remote", "add", "origin", url]).map_err(|e| format!("添加远程失败: {e}"))?;
        // 先试默认分支拉取
        let pulled = git(dir, &["pull", "origin", "main"])
            .or_else(|_| git(dir, &["pull", "origin", "master"]));
        if let Err(e) = pulled {
            return Err(format!("git pull 失败: {e}"));
        }
    }
    let branch = git(dir, &["branch", "--show-current"]).unwrap_or_default();
    Ok(json!({
        "ok": true,
        "branch": if branch.is_empty() { "main".to_string() } else { branch },
        "remote": url,
    }))
}

/// 导入项目后检测任务数据:项目内 `.teemocode/<sid>/meta.json` 视为导出的
/// 本地任务会话数据,迁移到应用数据目录(本地任务工作区)并把 workdir 改成
/// 当前选择的目录。返回迁移数量,前端提示重启重新加载任务数据。
#[tauri::command]
pub async fn import_task_data(app: tauri::AppHandle, workdir: String) -> Result<serde_json::Value, String> {
    let src = Path::new(&workdir).join(".teemocode");
    if !src.is_dir() {
        return Ok(json!({ "migrated": 0, "sessions": [] }));
    }
    let data = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("取应用数据目录失败: {e}"))?;
    let mut migrated = 0usize;
    let mut sessions: Vec<String> = vec![];
    let entries = std::fs::read_dir(&src).map_err(|e| format!("读取 .teemocode 失败: {e}"))?;
    for entry in entries.flatten() {
        let sid = entry.file_name().to_string_lossy().to_string();
        let meta_src = entry.path().join("meta.json");
        if !meta_src.is_file() {
            continue;
        }
        let dest = data.join(&sid);
        if !dest.is_dir() {
            std::fs::create_dir_all(&dest).map_err(|e| format!("创建会话目录失败: {e}"))?;
        }
        if let Ok(mut meta) = read_json(&meta_src) {
            meta["workdir"] = json!(workdir);
            write_json(&dest.join("meta.json"), &meta)?;
            if let Ok(files) = std::fs::read_dir(entry.path()) {
                for f in files.flatten() {
                    let name = f.file_name().to_string_lossy().to_string();
                    if name == "meta.json" {
                        continue;
                    }
                    let _ = std::fs::copy(f.path(), dest.join(&name));
                }
            }
            migrated += 1;
            sessions.push(sid);
        }
    }
    Ok(json!({ "migrated": migrated, "sessions": sessions }))
}

fn read_json(p: &Path) -> Result<serde_json::Value, String> {
    let s = std::fs::read_to_string(p).map_err(|e| format!("读取 {} 失败: {e}", p.display()))?;
    serde_json::from_str(&s).map_err(|e| format!("解析 {} 失败: {e}", p.display()))
}

fn write_json(p: &Path, v: &serde_json::Value) -> Result<(), String> {
    let s = serde_json::to_string_pretty(v).map_err(|e| e.to_string())?;
    std::fs::write(p, s).map_err(|e| format!("写入 {} 失败: {e}", p.display()))
}

/// 重启应用(任务数据迁移后让用户重启重新加载)。
#[tauri::command]
pub fn relaunch_app(app: tauri::AppHandle) {
    app.restart();
}

/// 克隆 git 仓库到临时目录并扫描 SKILL.md 技能文件。
/// 返回技能原始信息列表(name/description/content/相对路径),供大模型解析。
#[tauri::command]
pub async fn skills_import_git(url: String) -> Result<serde_json::Value, String> {
    let url = url.trim();
    if url.is_empty() {
        return Err("git 地址为空".into());
    }
    let rand: u128 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!("skills-import-{rand}"));
    if tmp.exists() {
        let _ = std::fs::remove_dir_all(&tmp);
    }
    std::fs::create_dir_all(&tmp).map_err(|e| format!("创建临时目录失败: {e}"))?;
    let tmp_s = tmp.to_string_lossy().to_string();
    if let Err(e) = git(&tmp_s, &["clone", "--depth", "1", url, "."]) {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(format!("git clone 失败: {e}"));
    }
    let mut skills: Vec<serde_json::Value> = Vec::new();
    scan_skill_dirs(&tmp, &tmp, &mut skills);
    if skills.is_empty() {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err("仓库中未找到 SKILL.md 技能文件(需形如 <skill-name>/SKILL.md)".into());
    }
    Ok(json!({ "tmp_dir": tmp_s, "skills": skills }))
}

/// 递归扫描目录下所有 SKILL.md(最多 5 层深),读取 name/description/content。
fn scan_skill_dirs(root: &Path, dir: &Path, out: &mut Vec<serde_json::Value>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().map(|n| n == ".git").unwrap_or(false) {
                continue;
            }
            let depth = p.strip_prefix(root).map(|r| r.components().count()).unwrap_or(0);
            if depth <= 5 {
                scan_skill_dirs(root, &p, out);
            }
        } else if p.file_name().map(|n| n == "SKILL.md").unwrap_or(false) {
            let Ok(content) = std::fs::read_to_string(&p) else { continue };
            let rel = p
                .strip_prefix(root)
                .map(|r| r.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            let dir_name = p
                .parent()
                .and_then(|d| d.file_name())
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "skill".into());
            let mut name = dir_name.clone();
            let mut desc = String::new();
            let mut in_fm = false;
            for line in content.lines() {
                let t = line.trim();
                if t == "---" {
                    if in_fm {
                        break;
                    }
                    in_fm = true;
                    continue;
                }
                if in_fm {
                    if let Some(v) = t.strip_prefix("name:") {
                        let v = v.trim().trim_matches('"');
                        if !v.is_empty() {
                            name = v.to_string();
                        }
                    } else if let Some(v) = t.strip_prefix("description:") {
                        desc = v.trim().trim_matches('"').to_string();
                    }
                }
            }
            if desc.is_empty() {
                desc = content
                    .lines()
                    .skip_while(|l| l.trim().is_empty() || l.trim() == "---")
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("")
                    .trim()
                    .chars()
                    .take(120)
                    .collect();
            }
            out.push(json!({
                "dir_name": dir_name,
                "name": name,
                "description": desc,
                "rel_path": rel,
                "content": content,
            }));
        }
    }
}

/// 用大模型解析单个 SKILL.md 内容,提取结构化摘要(名称/作用/关键信息/适用场景)。
/// 复用 model_test 的 provider/base/key 口径(openai/anthropic 两种协议)。
#[tauri::command]
pub async fn skill_analyze(
    provider: String,
    base_url: String,
    api_key: String,
    model: String,
    content: String,
) -> Result<String, String> {
    let base = base_url.trim().trim_end_matches('/').trim_end_matches("/v1").to_string();
    if base.is_empty() {
        return Err("请先填写接口地址".into());
    }
    let key = api_key.trim().to_string();
    if key.is_empty() {
        return Err("请先填写 API Key".into());
    }
    let model = model.trim().to_string();
    if model.is_empty() {
        return Err("请先填写模型标识".into());
    }
    let prompt = format!(
        "你是技能库分析师。以下是一个 Agent 技能(SKILL.md)的完整内容。请用中文输出严格的 JSON(不要 markdown 代码块包裹),格式:\n{{\"summary\": \"一句话作用概括(50字内)\", \"details\": \"详细说明(功能、用法、前置条件,200字内)\", \"keywords\": [\"关键词1\", ...], \"scenarios\": [\"适用场景1\", ...]}}\n\n技能名与 description 已解析,重点分析正文。\n\n--- SKILL.md 内容 ---\n{content}"
    );
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {e}"))?;
    // Responses API(openai_responses)的请求/响应结构都不同:
    // 请求 POST {base}/responses {model, input};回复 output_text 串联。
    let (url, req) = match provider.as_str() {
        "anthropic" => (
            format!("{base}/v1/messages"),
            client
                .post(format!("{base}/v1/messages"))
                .header("x-api-key", &key)
                .header("anthropic-version", "2023-06-01")
                .json(&serde_json::json!({
                    "model": model,
                    "max_tokens": 1024,
                    "messages": [{ "role": "user", "content": prompt }]
                })),
        ),
        "openai_responses" => (
            format!("{base}/responses"),
            client
                .post(format!("{base}/responses"))
                .bearer_auth(&key)
                .json(&serde_json::json!({
                    "model": model,
                    "input": prompt,
                })),
        ),
        _ => (
            format!("{base}/v1/chat/completions"),
            client
                .post(format!("{base}/v1/chat/completions"))
                .bearer_auth(&key)
                .json(&serde_json::json!({
                    "model": model,
                    "messages": [{ "role": "user", "content": prompt }]
                })),
        ),
    };
    let resp = req.send().await.map_err(|e| {
        if e.is_timeout() { "请求超时(120s)".to_string() } else { format!("无法连接: {e}") }
    })?;
    let status = resp.status();
    let body: serde_json::Value = resp.json().await.map_err(|_| format!("响应不是有效 JSON (HTTP {status})"))?;
    if !status.is_success() {
        let msg = body.get("error").and_then(|e| e.get("message")).and_then(|m| m.as_str()).unwrap_or("");
        return Err(format!("HTTP {status} @ {url}{}", if msg.is_empty() { String::new() } else { format!(": {msg}") }));
    }
    // 提取回复文本(openai: choices[0].message.content;anthropic: content[0].text;
    // responses: output_text 或 output[].content[].text)
    let text = body
        .pointer("/choices/0/message/content")
        .and_then(|v| v.as_str())
        .or_else(|| body.get("output_text").and_then(|v| v.as_str()))
        .or_else(|| {
            body.get("content")
                .and_then(|c| c.get(0))
                .and_then(|c| c.get("text"))
                .and_then(|v| v.as_str())
        })
        .or_else(|| {
            // responses API: output[] 里 type=message 的 content[] 里 type=output_text 的 text
            body.get("output")
                .and_then(|o| o.as_array())
                .and_then(|arr| {
                    arr.iter()
                        .filter_map(|step| step.get("content").and_then(|c| c.as_array()))
                        .flatten()
                        .filter_map(|part| part.get("text").and_then(|v| v.as_str()))
                        .next()
                })
        })
        .unwrap_or("")
        .to_string();
    if text.is_empty() {
        return Err("模型返回为空".into());
    }
    Ok(text)
}
