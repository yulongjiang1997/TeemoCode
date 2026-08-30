// 工作区记忆域 IPC:读写引擎约定的 <workdir>/.monkeycode/MEMORY.md
// (每个会话自动加载的用户指令/项目知识记忆,对标 ZCode 的持久记忆)。
// 命令对表 desktop/src/memory.rs;浏览器模式读降级 null、写抛错。
import { inDesktopShell, invoke } from "./ipc";

export async function memoryRead(workdir: string): Promise<string | null> {
  if (!inDesktopShell()) return null;
  return invoke<string>("memory_read", { workdir });
}

export async function memoryWrite(workdir: string, content: string): Promise<void> {
  if (!inDesktopShell()) throw new Error("浏览器模式下记忆只读,请在桌面应用中修改");
  return invoke<void>("memory_write", { workdir, content });
}
