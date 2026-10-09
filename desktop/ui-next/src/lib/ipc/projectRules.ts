// 仓库级 AGENTS.md 规则 IPC。命令对表 desktop/src/project_rules.rs。
import { inDesktopShell, invoke } from "./ipc";

export async function projectRulesRead(workdir: string): Promise<string | null> {
  if (!inDesktopShell()) return null;
  return invoke<string>("project_rules_read", { workdir });
}

export async function projectRulesWrite(workdir: string, content: string): Promise<void> {
  if (!inDesktopShell()) throw new Error("浏览器模式下项目规则只读,请在桌面应用中修改");
  return invoke<void>("project_rules_write", { workdir, content });
}
