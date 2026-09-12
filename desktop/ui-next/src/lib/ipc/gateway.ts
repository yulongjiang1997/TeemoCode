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

/** 统计聚合范围(对表 Rust GatewayRangeKind;serialize_all=lowercase)。 */
export type GatewayRangeKind = "today" | "day7" | "all";

/** 单个模型的聚合统计(gateway_log_stats 返回)。 */
export interface GatewayModelStats {
  model: string;
  calls: number;
  ok_calls: number;
  fail_calls: number;
  input_tokens: number;
  output_tokens: number;
  total_tokens: number;
  /** 请求耗时累计(毫秒);UI 换算秒展示 */
  duration_ms: number;
}

/** 单日热力图数据点。 */
export interface GatewayHeatDay {
  date: string;
  total_tokens: number;
  calls: number;
}

/** 跨会话调用统计面板数据源(gateway_log_stats)。 */
export interface GatewayLogStats {
  range: GatewayRangeKind;
  total_input_tokens: number;
  total_output_tokens: number;
  total_tokens: number;
  total_calls: number;
  /** 范围内请求耗时累计(毫秒) */
  total_duration_ms: number;
  /** 按 total_tokens 降序 */
  models: GatewayModelStats[];
  /** 全量留存天,按日期升序(热力图) */
  heatmap: GatewayHeatDay[];
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

/** 跨会话调用统计(2026-09-12):按模型 + 范围聚合 tokens/调用/总时长 + 全量热力图。
 * range 可选(today/day7/all),缺省 today。 */
export async function gatewayLogStats(range?: GatewayRangeKind): Promise<GatewayLogStats | null> {
  if (!inDesktopShell()) return null;
  return invoke<GatewayLogStats>("gateway_log_stats", { range: range ?? null });
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

/** 幂等自愈:enabled 但没在跑时按当前配置重建。工作区拉模型列表时调用,
 * 避免网关意外停止后用户无从发现(发网关组请求会一直 connection refused)。 */
export async function gatewayEnsureRunning(): Promise<void> {
  return invoke<void>("gateway_ensure_running");
}
