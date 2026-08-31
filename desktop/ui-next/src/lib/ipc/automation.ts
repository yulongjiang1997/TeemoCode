// 自动化域 IPC:定时任务的 CRUD 与立即运行。
// 命令对表 desktop/src/automation.rs;浏览器模式读 null/写抛错。
import { inDesktopShell, invoke } from "./ipc";

export interface Automation {
  id: string;
  name: string;
  enabled: boolean;
  kind: string;
  cron: string;
  fire_at_ms: number;
  prompt: string;
  kind_session: string;
  workdir: string;
  model: string;
  last_fire_ms: number;
  last_result: string;
  /** UI 标注:once 已过期(应用离线错过触发) */
  expired?: boolean;
}

export async function automationList(): Promise<Automation[]> {
  if (!inDesktopShell()) return [];
  return invoke<Automation[]>("automation_list");
}

export async function automationSave(automation: Automation): Promise<Automation> {
  return invoke<Automation>("automation_save", { automation });
}

export async function automationDelete(id: string): Promise<void> {
  return invoke<void>("automation_delete", { id });
}

export async function automationRunNow(id: string): Promise<{ ok: boolean; detail: string; latency_ms: number }> {
  return invoke<{ ok: boolean; detail: string; latency_ms: number }>("automation_run_now", { id });
}
