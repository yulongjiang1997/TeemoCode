import { invoke } from "@/lib/ipc/ipc";

export interface GitPushResult {
  ok: boolean;
  pushed: boolean;
  remote?: string;
  branch?: string;
  commit?: string;
}

export interface GitImportResult {
  ok: boolean;
  branch?: string;
  remote?: string;
}

/** 上传:工作目录有 git → 提交推送;没有 → init + 提交 + 推送(远程地址可选)。 */
export async function gitPush(workdir: string, remoteUrl?: string): Promise<GitPushResult> {
  return invoke<GitPushResult>("git_push", { workdir, remoteUrl });
}

/** 导入:按 git 地址加载到工作目录并拉取代码。 */
export async function gitImport(workdir: string, url: string): Promise<GitImportResult> {
  return invoke<GitImportResult>("git_import", { workdir, url });
}

export interface ImportTaskDataResult {
  migrated: number;
  sessions: string[];
}

/** 导入项目后检测 `.teemocode/` 任务数据并迁移到应用数据目录(本地任务工作区)。 */
export async function importTaskData(workdir: string): Promise<ImportTaskDataResult> {
  return invoke<ImportTaskDataResult>("import_task_data", { workdir });
}

/** 重启应用(任务数据迁移后重新加载)。 */
export async function relaunchApp(): Promise<void> {
  return invoke("relaunch_app");
}
