// 仓库级规则文件 AGENTS.md(与 MEMORY.md 用户记忆分开)。
// 引擎通常在会话开始时扫描工作区该文件;壳只负责读/写,不发明第三套格式。

use std::path::Path;

use crate::config::atomic_write_private;

const RULES_REL: &str = "AGENTS.md";
const MAX_RULES_BYTES: usize = 256 * 1024;

fn rules_path(workdir: &str) -> Result<std::path::PathBuf, String> {
    let dir = workdir.trim();
    if dir.is_empty() {
        return Err("工作目录为空".into());
    }
    Ok(Path::new(dir).join(RULES_REL))
}

#[tauri::command]
pub fn project_rules_read(workdir: String) -> Result<String, String> {
    let path = rules_path(&workdir)?;
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(format!("读取项目规则失败: {e}")),
    }
}

#[tauri::command]
pub fn project_rules_write(workdir: String, content: String) -> Result<(), String> {
    let path = rules_path(&workdir)?;
    if content.len() > MAX_RULES_BYTES {
        return Err(format!("规则内容过大(上限 {}KB)", MAX_RULES_BYTES / 1024));
    }
    atomic_write_private(&path, content.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(label: &str) -> String {
        let dir = std::env::temp_dir().join(format!(
            "mc-rules-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.to_string_lossy().into_owned()
    }

    #[test]
    fn missing_rules_read_empty_write_creates() {
        let dir = tmp_dir("basic");
        assert_eq!(project_rules_read(dir.clone()).unwrap(), "");
        project_rules_write(dir.clone(), "# AGENTS\n\n- 用中文回复".into()).unwrap();
        assert!(project_rules_read(dir.clone()).unwrap().contains("用中文回复"));
        assert!(Path::new(&dir).join("AGENTS.md").is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
