// 外部 CLI 子代理 API(2026-09-09):把本机安装的外部 CLI agent
// (Claude Code / Codex CLI)当子代理跑任务。壳侧 driver/external.rs spawn
// CLI、物化壳侧子会话、流式出帧;渲染零新增(复用引擎子代理的卡片/子会话
// 浮层体系)。引擎停着也能跑(不走引擎,直接 spawn CLI)。
import { inDesktopShell, invoke } from "./ipc";

/** 外部代理名(claude/codex;壳侧 AgentPreset 认死,识别不了会 Err)。 */
export type ExternalAgent = "claude" | "codex";

/** PATH 探测结果:哪个 CLI 装上了(菜单据此置灰)。 */
export interface ExternalAgentProbe {
  claude: boolean;
  codex: boolean;
}

/** 派发一条任务给外部 CLI(立即返回 run_id,worker 线程后台跑;失败抛)。 */
export function externalAgentRun(
  sessionId: string,
  agent: ExternalAgent,
  prompt: string,
  workdir: string,
  timeoutSecs: number,
): Promise<string> {
  return invoke<string>("external_agent_run", {
    sessionId,
    agent,
    prompt,
    workdir,
    timeoutSecs,
  });
}

/** 取消在跑的外部 CLI 子代理(置 cancel,worker 下一轮 kill 进程树)。 */
export function externalAgentCancel(runId: string): Promise<void> {
  return invoke<void>("external_agent_cancel", { runId }).catch(() => {});
}

/** 在跑的外部 CLI 子代理列表。浏览器模式返回空。 */
export function externalAgentList(): Promise<Array<{ run_id: string; child_sid: string; parent_sid: string }>> {
  if (!inDesktopShell()) return Promise.resolve([]);
  return invoke<Array<{ run_id: string; child_sid: string; parent_sid: string }>>("external_agent_list").catch(() => []);
}

/** 安装探测(菜单置灰);失败(非壳环境/旧壳)按全没装处理。 */
export function externalAgentProbe(): Promise<ExternalAgentProbe> {
  if (!inDesktopShell()) return Promise.resolve({ claude: false, codex: false });
  return invoke<ExternalAgentProbe>("external_agent_probe").catch(() => ({ claude: false, codex: false }));
}
