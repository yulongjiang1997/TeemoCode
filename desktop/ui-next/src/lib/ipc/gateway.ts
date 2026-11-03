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
  /** 是否走代理(2026-10-31):null=跟随模型库配置,true=强制走代理,false=强制不走。
   * 组内单独配置,修改不影响模型库的原配置。 */
  proxy: boolean | null;
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
  /** 是否记录探测/调用日志(2026-09-14) */
  log_enabled?: boolean;
  models: GroupModel[];
}

export type HealthState = "healthy" | "degraded" | "open" | "probing" | "abandoned";

/** 运行态下带健康注记的模型条目(gateway_status 返回)。 */
export interface GroupModelStatus extends GroupModel {
  label: string;
  upstream_model: string;
  /** 引用条目解析失败原因(已删除/缺模型标识) */
  unavailable: string | null;
  health: HealthState;
  /** 延迟(毫秒),来自最快模式后台探测或手动测试;null = 未探测/失败 */
  latency_ms: number | null;
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
  /** `{ts_ms}-{seq}`; 点详情用。pending 条目没有。 */
  id?: string;
  ts_ms: number;
  group_id: string;
  group_name: string;
  stream: boolean;
  ok: boolean;
  status: number | null;
  latency_ms: number;
  model: string;
  /** 候选的接口地址(2026-11-02):前端日志按厂商名显示。
   * 内存日志有;持久化历史查询无(缺省空)。 */
  base_url?: string;
  /** 厂商端点 URL(2026-11-02):「网关→厂商」请求的目标地址。
   * 形如 https://api.openai.com/v1/chat/completions。 */
  vendor_url?: string;
  attempts: number;
  prompt_tokens: number | null;
  completion_tokens: number | null;
  error: string | null;
  request_content: string | null;
  response_content: string | null;
  pending: boolean;
  /** 完整请求体。列表接口不带,gateway_log_detail 才有。 */
  raw_request?: string | null;
  /** 完整响应体。列表接口不带,gateway_log_detail 才有。 */
  raw_response?: string | null;
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
  /** 平均吞吐:total_tokens / 总时长(秒),即每秒 token 数(2026-11-02) */
  tokens_per_sec: number;
  /** 平均延迟:duration_ms / calls(毫秒,2026-11-02) */
  avg_latency_ms: number;
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
  /** 范围内总成功次数(2026-11-02) */
  total_ok_calls: number;
  /** 范围内总失败次数(2026-11-02) */
  total_fail_calls: number;
  /** 范围内请求耗时累计(毫秒) */
  total_duration_ms: number;
  /** 范围内平均吞吐:total_tokens / 总时长(秒)(2026-11-02) */
  total_tokens_per_sec: number;
  /** 范围内平均延迟:total_duration_ms / total_calls(毫秒)(2026-11-02) */
  total_avg_latency_ms: number;
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

/** Filter parameters for querying gateway logs. */
export interface GatewayLogFilter {
  group_id?: string | null;
  model?: string | null;
  ok?: boolean | null;
  search?: string | null;
  limit?: number | null;
  offset?: number | null;
  /** 是否包含探测/手动测试日志(默认 false) */
  include_probes?: boolean;
}

export async function gatewayLog(
  filter?: GatewayLogFilter,
): Promise<GatewayLogEntry[]> {
  if (!inDesktopShell()) return [];
  return invoke<GatewayLogEntry[]>("gateway_log", {
    limit: filter?.limit ?? null,
    groupId: filter?.group_id ?? null,
    model: filter?.model ?? null,
    ok: filter?.ok ?? null,
    search: filter?.search ?? null,
    offset: filter?.offset ?? null,
    includeProbes: filter?.include_probes ?? false,
  });
}

/** Total count of persisted log entries matching filters (for pagination). */
export async function gatewayLogCount(
  filter?: Omit<GatewayLogFilter, "limit" | "offset">,
): Promise<number> {
  if (!inDesktopShell()) return 0;
  return invoke<number>("gateway_log_count", {
    groupId: filter?.group_id ?? null,
    model: filter?.model ?? null,
    ok: filter?.ok ?? null,
    search: filter?.search ?? null,
    includeProbes: filter?.include_probes ?? false,
  });
}

/** 单条请求的完整请求/响应体。 */
export async function gatewayLogDetail(id: string): Promise<GatewayLogEntry | null> {
  if (!inDesktopShell()) return null;
  return invoke<GatewayLogEntry>("gateway_log_detail", { id });
}

/** 跨会话调用统计(2026-09-12):按模型 + 范围聚合 tokens/调用/总时长 + 全量热力图。
 * range 可选(today/day7/all),缺省 today。 */
export async function gatewayLogStats(range?: GatewayRangeKind): Promise<GatewayLogStats | null> {
  if (!inDesktopShell()) return null;
  return invoke<GatewayLogStats>("gateway_log_stats", { range: range ?? null });
}

/** 清空网关调用统计(2026-11-02):清除落盘的天×模型聚合,之后从零重新计算。 */
export async function gatewayLogStatsClear(): Promise<void> {
  return invoke<void>("gateway_log_stats_clear");
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

export async function gatewayTestGroup(id: string, timeoutMs?: number): Promise<GatewayTestResult> {
  return invoke<GatewayTestResult>("gateway_test_group", { id, timeoutMs: timeoutMs ?? null });
}

/** 单模型延迟探测结果(2026-09-13)。latency_ms = null 表示探测失败/超时。 */
export interface GatewayProbeModel {
  id: string;
  /** 延迟毫秒;null = 失败/超时 */
  latency_ms: string | null;
}

/** 整组延迟探测结果(gateway_probe_group)。 */
export interface GatewayProbeResult {
  id: string;
  models: GatewayProbeModel[];
}

/** 单模型探测结果(gateway_probe_model)。编辑表单中逐个测试模型用。 */
export interface GatewayProbeModelResult {
  ok: boolean;
  latency_ms: number;
  error?: string;
}

/** 单模型探测(2026-10-30):编辑表单中逐个测试已添加的模型,不依赖
 * 已保存的组——直接用连接参数发起探测。用于保存前验证模型可用性。
 * timeout_ms:探测超时(毫秒),默认 15000。 */
export async function gatewayProbeModel(
  provider: string,
  baseUrl: string,
  apiKey: string,
  model: string,
  timeoutMs?: number,
): Promise<GatewayProbeModelResult> {
  return invoke<GatewayProbeModelResult>("gateway_probe_model", {
    provider,
    baseUrl,
    apiKey,
    model,
    timeoutMs: timeoutMs ?? null,
  });
}

/** 用户自建厂商预设(对表 Rust VendorPreset)。 */
export interface VendorPreset {
  id: string;
  name: string;
  provider: string;
  base_url: string;
  api_key: string;
  /** 上下文窗口(token);0 = 缺省 */
  context_window?: number;
  /** 最大输出(token);0 = 缺省 */
  max_output?: number;
  /** 是否支持图片输入 */
  vision?: boolean;
  /** 思考模式:off | low | medium | high | max;空串 = 缺省 */
  think?: string;
}

/** 保存厂商预设列表(全量替换)。返回含新生成 id 的归一化列表。 */
export async function gatewaySaveVendors(vendors: VendorPreset[]): Promise<VendorPreset[]> {
  return invoke<VendorPreset[]>("gateway_save_vendors", { vendors });
}

/** 人工解除模型弃用状态(2026-09-15):连续失败超阈值被永久弃用后,
 * 用户确认问题已修复可手动解除,模型恢复可用。 */
export async function gatewayResetModelHealth(groupId: string, modelId: string): Promise<void> {
  return invoke<void>("gateway_reset_model_health", { groupId, modelId });
}

/** 逐模型探测延迟:对组内每个候选并行 ping,返回各模型延迟(毫秒)。
 * timeout_ms:探测超时(毫秒),默认 5000;超过则标记为 null(异常)。
 * 前端在测试按钮后展示每个模型后的延迟数值。 */
export async function gatewayProbeGroup(id: string, timeoutMs?: number): Promise<GatewayProbeResult> {
  return invoke<GatewayProbeResult>("gateway_probe_group", { id, timeoutMs: timeoutMs ?? null });
}

/** 幂等自愈:enabled 但没在跑时按当前配置重建。工作区拉模型列表时调用,
 * 避免网关意外停止后用户无从发现(发网关组请求会一直 connection refused)。 */
export async function gatewayEnsureRunning(): Promise<void> {
  return invoke<void>("gateway_ensure_running");
}

// ==================== 内置代理(2026-10-30) ====================
// app 自带 mihomo(Clash Meta)内核,用户填入订阅链接后自动下载内核、
// 拉取订阅、启动本地代理服务。网关上游请求自动走代理。

/** 代理运行状态(proxy_status 返回)。 */
export interface ProxyStatus {
  running: boolean;
  enabled: boolean;
  proxy_port: number;
  api_port: number;
  kernel_ready: boolean;
  has_subscription: boolean;
  /** 已保存的订阅地址(2026-10-31):回显到输入框。 */
  subscription_url: string;
  error: string | null;
}

/** 代理组(mihomo API 返回的 Selector/URLTest/Fallback)。 */
export interface ProxyGroup {
  name: string;
  type: string;
  now: string;
  options: string[];
}

export async function proxyStatus(): Promise<ProxyStatus> {
  if (!inDesktopShell()) return { running: false, enabled: false, proxy_port: 0, api_port: 0, kernel_ready: false, has_subscription: false, subscription_url: "", error: null };
  return invoke<ProxyStatus>("proxy_status");
}

export async function proxyStart(): Promise<ProxyStatus> {
  return invoke<ProxyStatus>("proxy_start");
}

export async function proxyStop(): Promise<ProxyStatus> {
  return invoke<ProxyStatus>("proxy_stop");
}

export async function proxySaveConfig(subscriptionUrl: string, enabled: boolean): Promise<void> {
  return invoke<void>("proxy_save_config", { subscriptionUrl, enabled });
}

export async function proxyProxies(): Promise<ProxyGroup[]> {
  if (!inDesktopShell()) return [];
  return invoke<ProxyGroup[]>("proxy_proxies");
}

export async function proxySelect(group: string, name: string): Promise<void> {
  return invoke<void>("proxy_select", { group, name });
}

/** 节点测速(2026-11-03):返回延迟毫秒数;失败抛错。 */
export async function proxyDelay(name: string): Promise<number> {
  return invoke<number>("proxy_delay", { name });
}
