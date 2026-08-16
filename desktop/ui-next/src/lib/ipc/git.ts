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
