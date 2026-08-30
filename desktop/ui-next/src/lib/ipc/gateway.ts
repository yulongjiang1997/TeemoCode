// 模型网关域 IPC:网关运行态、模型组 CRUD、连通性测试与请求日志。
//
// 命令对表 desktop/src/gateway/mod.rs(invoke_handler 与 capability 见
// build.rs / tauri.conf.json / tauri.debug.conf.json 的 gateway_* 组)。
// 浏览器模式语义与其他域一致:读降级(空态),写抛「浏览器模式」错误。
import { inDesktopShell, invoke } from "./ipc";

/** 组内一个模型条目(对表 Rust GroupModel)。alias 非空 = 引用模型库。 */
export interface GroupModel {
  id: string;
  enabled: boolean;
  /** 权重 1..=100:priority 下大者先行;weighted 下为分流比例 */
  weight: number;
  alias: string;
  provider: string;
  base_url: string;
  api_key: string;
  model: string;
}

/** 模型组(对表 Rust ModelGroup;组级上下文全组共享)。 */
export interface ModelGroup {
  id: string;
  name: string;
  enabled: boolean;
  key: string;
  /** priority | weighted */
  strategy: string;
  context_window: number;
  max_output: number;
  temperature: number | null;
  system_prompt: string;
  timeout_seconds: number;
  models: GroupModel[];
}

export type HealthState = "healthy" | "degraded" | "open" | "probing";

/** 运行态下带健康注记的模型条目(gateway_status 返回)。 */
export interface GroupModelStatus extends GroupModel {
  label: string;
  upstream_model: string;
  /** 引用条目解析失败原因(已删除/缺模型标识) */
  unavailable: string | null;
  health: HealthState;
}

export interface GroupCounters {
  total: number;
  ok: number;
  fail: number;
  failovers: number;
}

export interface GatewayGroupStatus extends ModelGroup {
  models: GroupModelStatus[];
  counters: GroupCounters;
}

export interface GatewayStatus {
  running: boolean;
  enabled: boolean;
  port: number;
  /** 服务级错误(端口被占等);正常为 null */
  error: string | null;
  groups: GatewayGroupStatus[];
}

export interface GatewayLogEntry {
  ts_ms: number;
  group_id: string;
  group_name: string;
  stream: boolean;
  ok: boolean;
  status: number | null;
  latency_ms: number;
  model: string;
  attempts: number;
  prompt_tokens: number | null;
  completion_tokens: number | null;
  error: string | null;
}

export interface GatewayTestResult {
  ok: boolean;
  model?: string;
  latency_ms: number;
  status?: number | null;
  content?: string;
  error?: string;
}

/** 网关对外端点(设置页展示/复制用)。 */
export function gatewayEndpoint(port: number): string {
  return `http://127.0.0.1:${port}/v1`;
}

export async function gatewayStatus(): Promise<GatewayStatus | null> {
  if (!inDesktopShell()) return null;
  return invoke<GatewayStatus>("gateway_status");
}

export async function gatewayLog(limit?: number): Promise<GatewayLogEntry[]> {
  if (!inDesktopShell()) return [];
  return invoke<GatewayLogEntry[]>("gateway_log", { limit: limit ?? null });
}

export async function gatewaySaveGroup(group: ModelGroup): Promise<ModelGroup> {
  return invoke<ModelGroup>("gateway_save_group", { group });
}

export async function gatewayDeleteGroup(id: string): Promise<void> {
  return invoke<void>("gateway_delete_group", { id });
}

export async function gatewayUpdateSettings(enabled: boolean, port: number): Promise<void> {
  return invoke<void>("gateway_update_settings", { enabled, port });
}

export async function gatewayRegenKey(id: string): Promise<string> {
  return invoke<string>("gateway_regen_key", { id });
}

export async function gatewayTestGroup(id: string): Promise<GatewayTestResult> {
  return invoke<GatewayTestResult>("gateway_test_group", { id });
}
