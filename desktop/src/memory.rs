// 工作区记忆面板(对标 ZCode 的持久记忆系统):引擎约定每个工作区用
// `<workdir>/.monkeycode/MEMORY.md` 记忆用户指令与项目知识(每个会话自动
// 加载),桌面端此前没有任何入口看到它。这里提供读/写两条命令,UI 侧
// MemoryDialog 做查看与编辑。
//
// 安全面:文件名固定(MEMORY.md),不存在用户可控的路径拼接;workdir 的
// 信任模型与 uploads/git_push 一致(来自会话元数据,UI 侧工作区)。
// 落盘复用 config::atomic_write_private(0600 临时文件 + 原子替换),
// 内容上限 256KB(记忆文件不该膨胀,超出即拒绝)。

use std::path::Path;

use crate::config::atomic_write_private;

const MEMORY_REL: &str = ".monkeycode/MEMORY.md";
const MAX_MEMORY_BYTES: usize = 256 * 1024;

fn memory_path(workdir: &str) -> Result<std::path::PathBuf, String> {
    let dir = workdir.trim();
    if dir.is_empty() {
        return Err("工作目录为空".into());
    }
    Ok(Path::new(dir).join(MEMORY_REL))
}

/// 读取工作区记忆;文件不存在返回空串(UI 显示空态),不当作错误。
#[tauri::command]
pub fn memory_read(workdir: String) -> Result<String, String> {
    let path = memory_path(&workdir)?;
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(format!("读取记忆失败: {e}")),
    }
}

/// 保存工作区记忆(原子写,自动建 .monkeycode 目录)。
#[tauri::command]
pub fn memory_write(workdir: String, content: String) -> Result<(), String> {
    let path = memory_path(&workdir)?;
    if content.len() > MAX_MEMORY_BYTES {
        return Err(format!("记忆内容过大(上限 {}KB)", MAX_MEMORY_BYTES / 1024));
    }
    atomic_write_private(&path, content.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(label: &str) -> String {
        let dir = std::env::temp_dir().join(format!(
            "mc-memory-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.to_string_lossy().into_owned()
    }

    #[test]
    fn missing_memory_reads_as_empty_and_write_creates_dirs() {
        let dir = tmp_dir("basic");
        // 未创建 .monkeycode/MEMORY.md:读 = 空串,不报错
        assert_eq!(memory_read(dir.clone()).unwrap(), "");

        memory_write(dir.clone(), "# 用户指令记忆\n\n- 别动我的工作区配置".into()).unwrap();
        let text = memory_read(dir.clone()).unwrap();
        assert!(text.contains("用户指令记忆"));

        // 目录与文件确实落在约定位置
        let p = Path::new(&dir).join(".monkeycode/MEMORY.md");
        assert!(p.is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_is_atomic_replace_and_rejects_oversize() {
        let dir = tmp_dir("atomic");
        memory_write(dir.clone(), "第一版".into()).unwrap();
        memory_write(dir.clone(), "第二版".into()).unwrap();
        assert_eq!(memory_read(dir.clone()).unwrap(), "第二版");
        // 旧临时文件不留残骸(同目录只应有 .monkeycode 目录)
        let entries: Vec<String> = std::fs::read_dir(Path::new(&dir))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries, vec![".monkeycode".to_string()]);

        let big = "x".repeat(MAX_MEMORY_BYTES + 1);
        assert!(memory_write(dir.clone(), big).unwrap_err().contains("过大"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_workdir_is_rejected() {
        assert!(memory_read("  ".into()).is_err());
        assert!(memory_write("".into(), "x".into()).is_err());
    }
}
