// 模型网关:统一大模型调度平台(模型组调度/故障切换/组级共享上下文)。
// 与 SkillsSection 同类:网关有自己的命令面(gateway_*),不进设置页
// save_config 保存条——本分区"改了即生效",没有脏状态管理。
// 行形态照 SkillsSection(list-row + 行内展开编辑);删除用两段确认
// (第一次点变红为"确认删除",失焦/超时还原),不引入弹窗。
import { IconArrowsExchange, IconChevronDown, IconCopy, IconDownload, IconPlus, IconRefresh, IconTrash, IconSearch, IconX } from "@tabler/icons-react";
import { useCallback, useEffect, useRef, useState } from "react";

import { useI18n } from "@/lib/i18n";
import { fetchModelIds } from "@/lib/ipc/config";
import { getConfig, type HostModel } from "@/lib/ipc/config";
import {
  gatewayDeleteGroup,
  gatewayEndpoint,
  gatewayLog,
  gatewayLogCount,
  gatewayLogDetail,
  gatewayProbeGroup,
  gatewayRegenKey,
  gatewayResetModelHealth,
  gatewaySaveGroup,
  gatewayStatus,
  gatewayUpdateSettings,
  type GatewayLogEntry,
  type GatewayLogFilter,
  type GatewayStatus,
  type GroupModel,
  type ModelGroup,
  type VendorPreset,
} from "@/lib/ipc/gateway";
import { inDesktopShell } from "@/lib/ipc/ipc";
import { copyText } from "@/lib/util/clipboard";
import { modelDisplay } from "@/lib/models/modelMenu";
import { VendorImportDialog } from "./VendorImportDialog";

function errText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

const HEALTH_BADGE: Record<string, string> = {
  healthy: "badge-success",
  degraded: "badge-warning",
  open: "badge-error",
  probing: "badge-info",
  abandoned: "badge-error",
};



function emptyGroup(): ModelGroup {
  return {
    id: "",
    name: "",
    enabled: true,
    key: "",
    strategy: "priority",
    context_window: 128_000,
    max_output: 32_768,
    temperature: null,
    system_prompt: "",
    timeout_seconds: 120,
    log_enabled: true,
    models: [],
  };
}

function emptyModel(): GroupModel {
  return {
    id: "",
    enabled: true,
    weight: 1,
    alias: "",
    provider: "openai",
    base_url: "",
    api_key: "",
    model: "",
  };
}

/** 毫秒时间戳 → YYYY-MM-DD HH:mm:ss。 */
function formatLogTime(tsMs: number): string {
  const d = new Date(tsMs);
  if (Number.isNaN(d.getTime())) return "—";
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

function prettyJsonValue(text: string): string | null {
  try {
    return JSON.stringify(JSON.parse(text), null, 2);
  } catch {
    return null;
  }
}

/** JSON 或 SSE(`data: {...}`) 格式化;截断/非 JSON 原文展示。 */
function formatLogBody(text: string): string {
  const trimmed = text.trim();
  if (!trimmed) return text;
  const looksSse = /^data:/m.test(trimmed) || trimmed.includes("\ndata:");
  if (looksSse) {
    return trimmed.split("\n").map((line) => {
      const s = line.replace(/\r$/, "");
      if (!s.startsWith("data:")) return s;
      const payload = s.slice(5).trim();
      if (!payload || payload === "[DONE]") return s;
      const pretty = prettyJsonValue(payload);
      if (!pretty) return s;
      return `data: ${pretty.replace(/\n/g, "\n      ")}`;
    }).join("\n");
  }
  return prettyJsonValue(trimmed) ?? text;
}

export function GatewaySection() {
  const { t } = useI18n();
  const [status, setStatus] = useState<GatewayStatus | null>(null);
  /** 是否有弃用模型(2026-09-15):控制「解除所有弃用」按钮显示 */
  const hasAbandoned = (status?.groups ?? []).some((g) => g.models.some((m) => m.health === "abandoned"));
  const [loadError, setLoadError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [library, setLibrary] = useState<HostModel[]>([]);
  /** 用户保存的厂商预设(2026-09-14):添加模型时选预设自动填 provider+base_url+api_key */
  const [vendorPresets, setVendorPresets] = useState<VendorPreset[]>([]);
  // 展开的组(查看态);编辑中的组 id(edit 非 null 时显示表单)
  const [expanded, setExpanded] = useState<string | null>(null);
  const [edit, setEdit] = useState<ModelGroup | null>(null);
  const [testResult, setTestResult] = useState<Record<string, { ok: boolean; text: string }>>({});
  /** 逐模型延迟探测结果(2026-09-13):key = group_id, value = { model_id -> latency_ms } */
  const [probeResult, setProbeResult] = useState<Record<string, Record<string, string | null>>>({});
  const [probing, setProbing] = useState<Set<string>>(new Set());
  /** 测试超时弹窗(2026-09-14):点测试按钮先弹输入超时,默认 5000ms。 */
  const [testTimeoutOpen, setTestTimeoutOpen] = useState<string | null>(null);
  const [testTimeoutValue, setTestTimeoutValue] = useState("5000");
  /** 从厂商批量导入弹窗(2026-09-14):"create"=新建组模式,edit.id=编辑组模式 */
  const [importDialog, setImportDialog] = useState<string | null>(null);
  /** 远端模型列表拉取(2026-09-14):key = 行下标(idx),value = { ids, error } */
  const [fetched, setFetched] = useState<Record<number, { ids: string[]; error?: string }>>({});
  const [fetching, setFetching] = useState<Set<number>>(new Set());
  const [log, setLog] = useState<GatewayLogEntry[]>([]);
  const [logFilter, setLogFilter] = useState<{
    group_id: string;
    model: string;
    ok: "" | "true" | "false";
    search: string;
  }>({ group_id: "", model: "", ok: "", search: "" });
  const [debouncedModel, setDebouncedModel] = useState("");
  const [debouncedSearch, setDebouncedSearch] = useState("");
  const [logTotal, setLogTotal] = useState(0);
  const [logPage, setLogPage] = useState(0);
  const [logDetail, setLogDetail] = useState<GatewayLogEntry | null>(null);
  // 端口草稿:与保存值不同时出现「应用」按钮
  const [portDraft, setPortDraft] = useState<string | null>(null);
  // 两段确认删除:记下待确认的组 id
  const [confirmingDelete, setConfirmingDelete] = useState<string | null>(null);
  const confirmTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const logFilterActive = !!logFilter.group_id || !!debouncedModel || !!debouncedSearch || logFilter.ok !== "";
  const LOG_PAGE_SIZE = 50;

  useEffect(() => {
    const timer = setTimeout(() => {
      setDebouncedModel(logFilter.model.trim());
      setDebouncedSearch(logFilter.search.trim());
    }, 300);
    return () => clearTimeout(timer);
  }, [logFilter.model, logFilter.search]);

  const buildLogFilter = useCallback((): GatewayLogFilter => ({
    group_id: logFilter.group_id || null,
    model: debouncedModel || null,
    ok: logFilter.ok === "" ? null : logFilter.ok === "true",
    search: debouncedSearch || null,
    limit: LOG_PAGE_SIZE,
    offset: logPage * LOG_PAGE_SIZE,
  }), [logFilter.group_id, logFilter.ok, debouncedModel, debouncedSearch, logPage]);

  const refresh = useCallback(() => {
    gatewayStatus()
      .then((s) => {
        setStatus(s);
        setLoadError(null);
      })
      .catch((e) => setLoadError(errText(e)));
    const filter = buildLogFilter();
    gatewayLog(filter)
      .then(setLog)
      .catch(() => {});
    gatewayLogCount({
      group_id: filter.group_id,
      model: filter.model,
      ok: filter.ok,
      search: filter.search,
    })
      .then(setLogTotal)
      .catch(() => {});
  }, [buildLogFilter]);
  useEffect(refresh, [refresh]);

  const openLogDetail = useCallback(async (e: GatewayLogEntry) => {
    if (e.pending) return;
    setLogDetail(e);
    if (!e.id) return;
    try {
      const full = await gatewayLogDetail(e.id);
      if (full) setLogDetail(full);
    } catch {
      // 列表摘要仍可看
    }
  }, []);

  useEffect(() => {
    setLogPage(0);
  }, [logFilter.group_id, logFilter.ok, debouncedModel, debouncedSearch]);

  // 模型库清单(引用条目的下拉来源;浏览器模式为空)
  useEffect(() => {
    if (!inDesktopShell()) return;
    getConfig()
      .then((cfg) => {
        setLibrary(cfg?.models ?? []);
        setVendorPresets(cfg?.gateway?.vendor_presets ?? []);
      })
      .catch(() => {});
  }, []);

  // 运行态/日志 5s 轮询(分区可见期间),卸载即停
  // 日志刷新间隔(2026-09-14):有 pending 请求时 1.5s 快轮询让"请求中"态
  // 快速翻为完成态;无 pending 时 5s 足够。
  useEffect(() => {
    const hasPending = log.some((e) => e.pending);
    const interval = hasPending ? 1500 : 5000;
    const timer = setInterval(refresh, interval);
    return () => clearInterval(timer);
  }, [refresh, log]);

  useEffect(
    () => () => {
      if (confirmTimer.current) clearTimeout(confirmTimer.current);
    },
    [],
  );

  const run = (action: () => Promise<unknown>, onDone?: () => void) => {
    setBusy(true);
    setError(null);
    action()
      .then(() => {
        onDone?.();
        refresh();
      })
      .catch((e) => setError(t("settings.gateway.opFailed", { reason: errText(e) })))
      .finally(() => setBusy(false));
  };

  const toggleEnabled = (enabled: boolean) => {
    const port = status?.port ?? 8317;
    run(() => gatewayUpdateSettings(enabled, port));
  };

  const applyPort = () => {
    const port = Number(portDraft);
    if (!status || !Number.isInteger(port) || port < 1024 || port > 65535) {
      setError(t("settings.gateway.opFailed", { reason: t("settings.gateway.port") }));
      return;
    }
    run(() => gatewayUpdateSettings(status.enabled, port), () => setPortDraft(null));
  };

  const save = () => {
    if (!edit) return;
    setBusy(true);
    setError(null);
    gatewaySaveGroup(edit)
      .then(() => {
        setEdit(null);
        setExpanded(null);
        refresh();
      })
      .catch((e) => {
        setError(t("settings.gateway.saveFailed", { reason: errText(e) }));
        // 失败也刷新:部分失败形态(如保存后回查异常)组其实已落盘,
        // 不刷新的话列表与磁盘脱节,用户会在旧状态上反复重试
        refresh();
      })
      .finally(() => setBusy(false));
  };

  const remove = (g: ModelGroup) => {
    if (confirmingDelete !== g.id) {
      setConfirmingDelete(g.id);
      if (confirmTimer.current) clearTimeout(confirmTimer.current);
      confirmTimer.current = setTimeout(() => setConfirmingDelete(null), 4000);
      return;
    }
    setConfirmingDelete(null);
    setBusy(true);
    setError(null);
    gatewayDeleteGroup(g.id)
      .then(() => {
        setExpanded(null);
        refresh();
      })
      .catch((e) => setError(t("settings.gateway.saveFailed", { reason: errText(e) })))
      .finally(() => setBusy(false));
  };

  /** 逐模型延迟探测(2026-09-13):并行 ping 组内每个候选,延迟结果
   *  写入 probeResult,在展开的模型列表中每个模型后展示毫秒数。
   *  timeoutMs:探测超时(毫秒),超过则标记为 null(异常)。 */
  const runTest = (id: string) => {
    const timeoutMs = parseInt(testTimeoutValue, 10) || 5000;
    setTestTimeoutOpen(null);
    setTestResult((prev) => ({ ...prev, [id]: { ok: true, text: t("settings.gateway.group.testing") } }));
    setProbing((prev) => new Set(prev).add(id));
    gatewayProbeGroup(id, timeoutMs)
      .then((r) => {
        const map: Record<string, string | null> = {};
        for (const m of r.models) map[m.id] = m.latency_ms;
        setProbeResult((prev) => ({ ...prev, [id]: map }));
        // 推导整组结果:任一模型有延迟=成功
        const anyOk = r.models.some((m) => m.latency_ms !== null);
        setTestResult((prev) => ({
          ...prev,
          [id]: anyOk
            ? { ok: true, text: t("settings.gateway.group.testOk", { model: r.models.find((m) => m.latency_ms !== null)?.id ?? "", latency: Number(r.models.find((m) => m.latency_ms !== null)?.latency_ms) || 0 }) }
            : { ok: false, text: t("settings.gateway.group.testFailed", { error: "所有模型超时或失败" }) },
        }));
        refresh();
      })
      .catch((e) => {
        setTestResult((prev) => ({ ...prev, [id]: { ok: false, text: t("settings.gateway.group.testFailed", { error: errText(e) }) } }));
      })
      .finally(() => {
        setProbing((prev) => { const n = new Set(prev); n.delete(id); return n; });
      });
  };

  /** 拉取远端模型列表(2026-09-14):用当前行的 provider/base_url/api_key
   *  调 models_fetch,结果存入 fetched 供 datalist 下拉选择。 */
  const fetchList = async (idx: number) => {
    if (!edit || fetching.has(idx)) return;
    const m = edit.models[idx];
    if (!m) return;
    const next = new Set(fetching); next.add(idx); setFetching(next);
    try {
      const ids = await fetchModelIds(m.provider, m.base_url, m.api_key);
      setFetched((prev) => ({ ...prev, [idx]: ids.length ? { ids } : { ids: [], error: t("settings.models.fetch.empty") } }));
    } catch (e) {
      setFetched((prev) => ({ ...prev, [idx]: { ids: [], error: e instanceof Error ? e.message : String(e) } }));
    } finally {
      const rest = new Set(fetching); rest.delete(idx); setFetching(rest);
    }
  };

  // 重置 Key 两段确认(Tauri 下 window.confirm 是 dialog 插件命令,未放行
  // 会被 ACL 拒 → unhandledrejection;与删除按钮同款布防模式)
  const [confirmingRegen, setConfirmingRegen] = useState<string | null>(null);
  const regenKey = (g: ModelGroup) => {
    if (confirmingRegen !== g.id) {
      setConfirmingRegen(g.id);
      if (confirmTimer.current) clearTimeout(confirmTimer.current);
      confirmTimer.current = setTimeout(() => setConfirmingRegen(null), 4000);
      return;
    }
    setConfirmingRegen(null);
    run(() => gatewayRegenKey(g.id));
  };

  const copy = (text: string) => {
    copyText(text);
    setError(null);
  };

  if (!inDesktopShell()) {
    return (
      <div role="alert" className="alert alert-warning alert-soft max-w-md text-xs">
        {t("settings.browserReadonly")}
      </div>
    );
  }

  const enabled = status?.enabled ?? false;
  const running = status?.running ?? false;
  const port = status?.port ?? 8317;
  const portDirty = portDraft !== null && Number(portDraft) !== port;

  /** 组编辑表单(新建与编辑共用)。 */
  const editForm = edit && (
    <div className="flex flex-col gap-3 rounded-box border border-base-300 bg-base-100 p-4">
      <div className="grid grid-cols-2 gap-3">
        <fieldset className="fieldset gap-1.5">
          <legend className="fieldset-legend">{t("settings.gateway.form.name")}</legend>
          <input
            className="input input-sm w-full"
            aria-label={t("settings.gateway.form.name")}
            value={edit.name}
            onChange={(e) => setEdit({ ...edit, name: e.target.value })}
          />
          <p className="text-2xs text-base-content/50">{t("settings.gateway.form.nameHint")}</p>
        </fieldset>
        <fieldset className="fieldset gap-1.5">
          <legend className="fieldset-legend">{t("settings.gateway.form.strategy")}</legend>
          <select
            className="select select-sm w-full"
            aria-label={t("settings.gateway.form.strategy")}
            value={edit.strategy}
            onChange={(e) => setEdit({ ...edit, strategy: e.target.value })}
          >
            <option value="priority">{t("settings.gateway.group.strategy.priority")}</option>
            <option value="weighted">{t("settings.gateway.group.strategy.weighted")}</option>
            <option value="fastest">{t("settings.gateway.group.strategy.fastest")}</option>
            <option value="balanced">{t("settings.gateway.group.strategy.balanced")}</option>
          </select>
          <p className="text-2xs text-base-content/50">{t("settings.gateway.form.strategyHint")}</p>
        </fieldset>
      </div>

      {/* 组级上下文(全组共享) */}
      <div className="rounded-box bg-base-200/50 p-3">
        <p className="text-xs font-semibold">{t("settings.gateway.form.ctxTitle")}</p>
        <p className="mb-2 text-2xs text-base-content/50">{t("settings.gateway.form.ctxHint")}</p>
        <div className="grid grid-cols-2 gap-2 sm:grid-cols-4">
          <label className="flex flex-col gap-1 text-2xs">
            {t("settings.gateway.form.contextWindow")}
            <input
              type="number"
              className="input input-xs w-full font-mono"
              value={edit.context_window}
              min={1}
              onChange={(e) => setEdit({ ...edit, context_window: Number(e.target.value) || 0 })}
            />
          </label>
          <label className="flex flex-col gap-1 text-2xs">
            {t("settings.gateway.form.maxOutput")}
            <input
              type="number"
              className="input input-xs w-full font-mono"
              value={edit.max_output}
              min={1}
              onChange={(e) => setEdit({ ...edit, max_output: Number(e.target.value) || 0 })}
            />
          </label>
          <label className="flex flex-col gap-1 text-2xs">
            {t("settings.gateway.form.temperature")}
            <input
              type="number"
              step="0.1"
              min={0}
              max={2}
              className="input input-xs w-full font-mono"
              placeholder={t("settings.gateway.form.temperaturePlaceholder")}
              value={edit.temperature ?? ""}
              onChange={(e) => {
                const v = e.target.value;
                setEdit({ ...edit, temperature: v === "" ? null : Number(v) });
              }}
            />
          </label>
          <label className="flex flex-col gap-1 text-2xs">
            {t("settings.gateway.form.timeout")}
            <input
              type="number"
              className="input input-xs w-full font-mono"
              value={edit.timeout_seconds}
              min={1}
              onChange={(e) => setEdit({ ...edit, timeout_seconds: Number(e.target.value) || 0 })}
            />
          </label>
          {/* 探测日志开关(2026-09-14) */}
          <label className="flex items-center gap-2 text-2xs">
            {t("settings.gateway.form.logEnabled")}
            <input
              type="checkbox"
              className="toggle toggle-xs"
              checked={edit.log_enabled !== false}
              onChange={(e) => setEdit({ ...edit, log_enabled: e.target.checked })}
            />
          </label>
        </div>
        <label className="mt-2 flex flex-col gap-1 text-2xs">
          {t("settings.gateway.form.systemPrompt")}
          <textarea
            className="textarea textarea-xs min-h-16 w-full font-mono"
            placeholder={t("settings.gateway.form.systemPromptPlaceholder")}
            value={edit.system_prompt}
            onChange={(e) => setEdit({ ...edit, system_prompt: e.target.value })}
          />
        </label>
      </div>

      {/* 组内模型 */}
      <div className="flex flex-col gap-1.5">
        <p className="text-xs font-semibold">{t("settings.gateway.form.modelsTitle")}</p>
        {edit.models.map((m, idx) => {
          const isRef = m.alias !== "";
          const health = status?.groups.find((g) => g.id === edit.id)?.models.find((x) => x.id === m.id);
          return (
            <div key={m.id || `row-${idx}`} className="flex flex-col gap-2 rounded-box border border-base-300 p-2.5">
              <div className="flex flex-wrap items-center gap-2">
                <select
                  className="select select-xs w-36"
                  aria-label={t("settings.gateway.model.sourceRef")}
                  value={isRef ? "ref" : "custom"}
                  onChange={(e) => {
                    const models = [...edit.models];
                    if (e.target.value === "ref") {
                      models[idx] = { ...m, alias: library[0]?.name ?? "", base_url: "", api_key: "", provider: "", model: "" };
                    } else {
                      models[idx] = { ...m, alias: "" };
                    }
                    setEdit({ ...edit, models });
                  }}
                >
                  <option value="ref">{t("settings.gateway.model.sourceRef")}</option>
                  <option value="custom">{t("settings.gateway.model.sourceCustom")}</option>
                </select>
                <label className="flex items-center gap-1 text-2xs">
                  {t("settings.gateway.model.weight")}
                  <input
                    type="text"
                    className="input input-xs w-16 font-mono"
                    value={m.weight}
                    title={t("settings.gateway.model.weightHint")}
                    onChange={(e) => {
                      const models = [...edit.models];
                      models[idx] = { ...m, weight: e.target.value as unknown as number };
                      setEdit({ ...edit, models });
                    }}
                    onBlur={(e) => {
                      const n = parseInt(e.target.value, 10);
                      const models = [...edit.models];
                      models[idx] = { ...m, weight: Number.isFinite(n) && n > 0 ? n : 1 };
                      setEdit({ ...edit, models });
                    }}
                  />
                </label>
                {health && (
                  <span className={`badge badge-soft badge-sm ${HEALTH_BADGE[health.health] ?? ""}`}>
                    {t(`gateway.health.${health.health}`)}
                  </span>
                )}
                <span className="flex-1" />
                <button
                  type="button"
                  className="btn btn-ghost btn-xs"
                  onClick={() => {
                    const models = [...edit.models];
                    models[idx] = { ...m, enabled: !m.enabled };
                    setEdit({ ...edit, models });
                  }}
                >
                  {t(m.enabled ? "settings.gateway.model.disable" : "settings.gateway.model.enable")}
                </button>
                <button
                  type="button"
                  className="btn btn-ghost btn-xs text-base-content/40 hover:text-error"
                  onClick={() => setEdit({ ...edit, models: edit.models.filter((_, i) => i !== idx) })}
                >
                  {t("settings.gateway.model.remove")}
                </button>
              </div>
              {isRef ? (
                library.length === 0 ? (
                  <p className="text-2xs text-base-content/50">{t("settings.gateway.model.emptyLibrary")}</p>
                ) : (
                  <select
                    className="select select-xs w-full font-mono"
                    aria-label={t("settings.gateway.model.alias")}
                    value={m.alias}
                    onChange={(e) => {
                      const models = [...edit.models];
                      models[idx] = { ...m, alias: e.target.value };
                      setEdit({ ...edit, models });
                    }}
                  >
                    {library.map((lm) => {
                      const d = modelDisplay({ name: lm.name, model: lm.model, source: lm.source });
                      return (
                        <option key={lm.name} value={lm.name}>
                          {d.label.trim() || lm.name} ({lm.model})
                        </option>
                      );
                    })}
                  </select>
                )
              ) : (
                <div className="grid grid-cols-2 gap-2">
                  <label className="flex flex-col gap-1 text-2xs">
                    {t("settings.gateway.model.provider")}
                    <select
                      className="select select-xs w-full"
                      value={m.provider}
                      onChange={(e) => {
                        const models = [...edit.models];
                        models[idx] = { ...m, provider: e.target.value };
                        setEdit({ ...edit, models });
                      }}
                    >
                      <option value="openai">OpenAI</option>
                      <option value="anthropic">Anthropic</option>
                      <option value="openai_responses">OpenAI Responses</option>
                    </select>
                  </label>
                  <label className="flex flex-col gap-1 text-2xs">
                    {t("settings.gateway.model.model")}
                    <div className="flex gap-1">
                      <input
                        className="input input-xs min-w-0 flex-1 font-mono"
                        list={`gw-models-${idx}`}
                        value={m.model}
                        onChange={(e) => {
                          const models = [...edit.models];
                          models[idx] = { ...m, model: e.target.value };
                          setEdit({ ...edit, models });
                        }}
                      />
                      <datalist id={`gw-models-${idx}`}>
                        {fetched[idx]?.ids.map((id) => (
                          <option key={id} value={id} />
                        ))}
                      </datalist>
                      <button
                        type="button"
                        className="btn btn-ghost btn-xs shrink-0"
                        disabled={fetching.has(idx)}
                        onClick={() => fetchList(idx)}
                        title={t("settings.models.fetch")}
                      >
                        {fetching.has(idx)
                          ? <span className="loading loading-spinner loading-xs" aria-hidden />
                          : <IconRefresh size={12} stroke={1.75} aria-hidden />}
                        {t("settings.models.fetch")}
                      </button>
                    </div>
                    {fetched[idx]?.error && (
                      <span className="text-error text-2xs">{fetched[idx]!.error}</span>
                    )}
                  </label>
                  <label className="flex flex-col gap-1 text-2xs">
                    {t("settings.gateway.model.baseUrl")}
                    <input
                      className="input input-xs w-full font-mono"
                      placeholder="https://…"
                      value={m.base_url}
                      onChange={(e) => {
                        const models = [...edit.models];
                        models[idx] = { ...m, base_url: e.target.value };
                        setEdit({ ...edit, models });
                      }}
                    />
                  </label>
                  <label className="flex flex-col gap-1 text-2xs">
                    {t("settings.gateway.model.apiKey")}
                    <input
                      type="password"
                      className="input input-xs w-full font-mono"
                      value={m.api_key}
                      onChange={(e) => {
                        const models = [...edit.models];
                        models[idx] = { ...m, api_key: e.target.value };
                        setEdit({ ...edit, models });
                      }}
                    />
                  </label>
                </div>
              )}
            </div>
          );
        })}
        <button
          type="button"
          className="btn btn-xs btn-outline w-fit"
          onClick={() => {
            const first = library[0]?.name;
            setEdit({
              ...edit,
              models: [...edit.models, first ? { ...emptyModel(), alias: first } : emptyModel()],
            });
          }}
        >
          <IconPlus size={13} stroke={2} aria-hidden />
          {t("settings.gateway.form.addModel")}
        </button>
        <button
          type="button"
          className="btn btn-xs btn-ghost w-fit"
          disabled={vendorPresets.length === 0}
          onClick={() => setImportDialog(edit.id || "edit")}
          title={t("settings.gateway.import.batchFromVendor")}
        >
          <IconDownload size={13} stroke={2} aria-hidden />
          {t("settings.gateway.import.batchFromVendor")}
        </button>
      </div>

      <div className="flex items-center gap-2">
        <button type="button" className="btn btn-primary btn-sm" disabled={busy || !edit.name.trim()} onClick={save}>
          {t("settings.gateway.save")}
        </button>
        <button type="button" className="btn btn-ghost btn-sm" disabled={busy} onClick={() => setEdit(null)}>
          {t("settings.gateway.cancel")}
        </button>
      </div>
    </div>
  );

  return (
    <>
    <section aria-label={t("settings.nav.gateway")} className="flex flex-col gap-2">
      <p className="text-xs leading-relaxed text-base-content/50">{t("settings.gateway.hint")}</p>
      {loadError && (
        <div role="alert" className="alert alert-error alert-soft max-w-md text-xs">
          {t("settings.gateway.loadFailed", { reason: loadError })}
        </div>
      )}
      {error && (
        <div role="alert" className="alert alert-error alert-soft max-w-md text-xs">
          {error}
        </div>
      )}

      {/* 服务总开关 + 端点 */}
      <div className="flex flex-col gap-2 rounded-box border border-base-300 bg-base-100 p-4">
        <div className="flex items-center justify-between gap-4">
          <div className="flex min-w-0 flex-col gap-0.5">
            <span className="text-sm font-medium">{t("settings.gateway.enable")}</span>
            <span className="text-xs text-base-content/50">{t("settings.gateway.enableHint")}</span>
          </div>
          <input
            type="checkbox"
            className="toggle toggle-sm"
            aria-label={t("settings.gateway.enable")}
            checked={enabled}
            disabled={busy}
            onChange={(e) => toggleEnabled(e.target.checked)}
          />
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <span className={`badge badge-soft badge-sm ${running ? "badge-success" : "badge-ghost"}`}>
            {running ? t("settings.gateway.running") : t("settings.gateway.stopped")}
          </span>
          {status?.error && <span className="badge badge-error badge-soft badge-sm">{status.error}</span>}
          <label className="flex items-center gap-1.5 text-xs">
            {t("settings.gateway.port")}
            <input
              type="number"
              className="input input-xs w-24 font-mono"
              min={1024}
              max={65535}
              value={portDraft ?? port}
              onChange={(e) => setPortDraft(e.target.value)}
            />
          </label>
          {portDirty && (
            <button type="button" className="btn btn-xs btn-primary" disabled={busy} onClick={applyPort}>
              {t("settings.save.confirm")}
            </button>
          )}
          <code className="rounded bg-base-200/70 px-2 py-1 font-mono text-xs">{gatewayEndpoint(port)}</code>
          <button
            type="button"
            className="btn btn-ghost btn-xs"
            onClick={() => copy(gatewayEndpoint(port))}
            title={t("settings.gateway.copy")}
          >
            <IconCopy size={13} stroke={1.75} aria-hidden />
            {t("settings.gateway.copy")}
          </button>
        </div>
      </div>

      {/* 模型组列表 */}
      <div className="mt-1 flex w-fit items-center gap-1.5 px-1 text-xs font-bold text-base-content/60">
        {t("settings.gateway.groups.title")}
      </div>
      {/* 解除所有弃用模型(2026-09-15) */}
      {hasAbandoned && (
        <button
          type="button"
          className="btn btn-ghost btn-xs w-fit text-warning"
          onClick={() => {
            const groups = status?.groups ?? [];
            Promise.all(
              groups.flatMap((g) =>
                g.models.filter((m) => m.health === "abandoned").map((m) => gatewayResetModelHealth(g.id, m.id))
              )
            ).then(() => refresh());
          }}
        >
          {t("settings.gateway.health.resetAll")}
        </button>
      )}
      {(status?.groups.length ?? 0) === 0 && (
        <div className="rounded-box border border-dashed border-base-300 px-4 py-6">
          <p className="text-center text-xs leading-relaxed text-base-content/50">{t("settings.gateway.groups.empty")}</p>
        </div>
      )}
      <ul className="list divide-y divide-base-300 overflow-hidden rounded-box border border-base-300 bg-base-100">
        {(status?.groups ?? []).map((g) => {
          const open = expanded === g.id && edit?.id !== g.id;
          const enabledModels = g.models.filter((m) => m.enabled).length;
          const openCount = g.models.filter((m) => m.health === "open").length;
          const confirming = confirmingDelete === g.id;
          return (
            <li key={g.id} className="flex flex-col">
              <div
                className="group list-row cursor-pointer items-center gap-2 rounded-none px-4 py-2 transition-colors hover:bg-base-200/40"
                onClick={() => {
                  setExpanded(open ? null : g.id);
                  setEdit(null);
                }}
              >
                <button type="button" className="list-col-grow flex min-w-0 cursor-pointer items-center gap-2 text-start">
                  <span className="shrink-0 truncate font-mono text-xs">{g.name}</span>
                  {!g.enabled && <span className="badge badge-ghost badge-sm shrink-0">{t("settings.gateway.group.disable")}</span>}
                  <span className="badge badge-ghost badge-sm shrink-0">
                    {t("settings.gateway.group.modelsBadge", { count: enabledModels })}
                  </span>
                  <span className="badge badge-ghost badge-sm shrink-0">
                    {t(("settings.gateway.group.strategy." + (g.strategy || "priority")) as "settings.gateway.group.strategy.priority")}
                  </span>
                  {openCount > 0 && (
                    <span className="badge badge-error badge-soft badge-sm shrink-0">{t("gateway.health.open")}</span>
                  )}
                  <span className="min-w-0 flex-1 truncate text-2xs text-base-content/40">
                    {t("gateway.counters.total")} {g.counters.total} · {t("gateway.counters.ok")} {g.counters.ok} ·{" "}
                    {t("gateway.counters.fail")} {g.counters.fail} · {t("gateway.counters.failovers")} {g.counters.failovers}
                  </span>
                </button>
                <button
                  type="button"
                  className="btn btn-ghost btn-xs shrink-0 text-base-content/60 opacity-0 transition-opacity group-hover:opacity-100 group-focus-within:opacity-100"
                  disabled={busy}
                  onClick={(e) => {
                    e.stopPropagation();
                    setTestTimeoutValue("5000");
                    setTestTimeoutOpen(g.id);
                  }}
                >
                  {probing.has(g.id) ? <span className="loading loading-spinner loading-xs" aria-hidden /> : null}
                  {t("settings.gateway.group.test")}
                </button>
                <button
                  type="button"
                  className="btn btn-ghost btn-xs shrink-0 text-base-content/60 opacity-0 transition-opacity group-hover:opacity-100 group-focus-within:opacity-100"
                  onClick={(e) => {
                    e.stopPropagation();
                    setExpanded(g.id);
                    setEdit({
                      id: g.id,
                      name: g.name,
                      enabled: g.enabled,
                      key: g.key,
                      strategy: g.strategy,
                      context_window: g.context_window,
                      max_output: g.max_output,
                      temperature: g.temperature,
                      system_prompt: g.system_prompt,
                      timeout_seconds: g.timeout_seconds,
                      log_enabled: g.log_enabled,
                      models: g.models.map((m) => ({
                        id: m.id,
                        enabled: m.enabled,
                        weight: m.weight,
                        alias: m.alias,
                        provider: m.provider,
                        base_url: m.base_url,
                        api_key: m.api_key,
                        model: m.model,
                      })),
                    });
                  }}
                >
                  {t("settings.gateway.group.edit")}
                </button>
                <button
                  type="button"
                  className={`btn btn-ghost btn-xs shrink-0 opacity-0 transition-opacity group-hover:opacity-100 group-focus-within:opacity-100 ${
                    confirming ? "text-error" : "text-base-content/40 hover:text-error"
                  }`}
                  disabled={busy}
                  onClick={(e) => {
                    e.stopPropagation();
                    remove(g);
                  }}
                  onBlur={() => setConfirmingDelete(null)}
                >
                  <IconTrash size={13} stroke={1.75} aria-hidden />
                  {confirming ? t("settings.gateway.group.delete") : ""}
                </button>
                <IconChevronDown
                  size={14}
                  stroke={1.75}
                  aria-hidden
                  className={`shrink-0 text-base-content/40 transition-transform duration-150 ${open ? "rotate-180" : ""}`}
                />
              </div>
              {/* 测试结果横条 */}
              {testResult[g.id] && (
                <div className={`px-4 pb-2 text-xs ${testResult[g.id]!.ok ? "text-success" : "text-error"}`}>
                  {testResult[g.id]!.text}
                </div>
              )}
              {/* 展开视图:编辑表单优先,否则 Key/健康/模型明细 */}
              {expanded === g.id && edit?.id === g.id ? (
                <div className="border-t border-base-300 px-4 pt-2 pb-4">{editForm}</div>
              ) : (
                open && (
                  <div className="flex flex-col gap-2 border-t border-base-300 px-4 pt-2 pb-4 text-xs">
                    <div className="flex flex-wrap items-center gap-2">
                      <span className="text-base-content/50">{t("settings.gateway.group.key")}</span>
                      <code className="rounded bg-base-200/70 px-2 py-0.5 font-mono">{g.key}</code>
                      <button
                        type="button"
                        className="btn btn-ghost btn-xs"
                        onClick={() => copy(g.key)}
                        title={t("settings.gateway.copy")}
                      >
                        <IconCopy size={12} stroke={1.75} aria-hidden />
                      </button>
                      <button
                        type="button"
                        className={`btn btn-ghost btn-xs ${confirmingRegen === g.id ? "text-error" : "text-base-content/60"}`}
                        disabled={busy}
                        onClick={() => regenKey(g)}
                        onBlur={() => setConfirmingRegen(null)}
                      >
                        <IconRefresh size={12} stroke={1.75} aria-hidden />
                        {confirmingRegen === g.id ? t("settings.gateway.group.regenArm") : t("settings.gateway.group.regen")}
                      </button>
                    </div>
                    <p className="text-2xs text-base-content/40">{t("settings.gateway.group.keyHint")}</p>
                    <ul className="flex flex-col gap-1">
                      {g.models.map((m) => (
                        <li key={m.id} className="flex flex-wrap items-center gap-2">
                          <span className={`badge badge-soft badge-xs ${HEALTH_BADGE[m.health] ?? ""}`}>
                            {t(`gateway.health.${m.health}`)}
                          </span>
                          <span className="font-mono">
                            {m.alias ? modelDisplay({ name: m.alias, model: m.model, source: "monkeycode" }).label : m.model}
                          </span>
                          <span className="text-base-content/40">w{m.weight}</span>
                          {!m.enabled && <span className="badge badge-ghost badge-xs">{t("settings.gateway.group.disable")}</span>}
                          {m.unavailable && <span className="text-error">{m.unavailable}</span>}
                          {/* 永久弃用模型:显示「解除」按钮(2026-09-15) */}
                          {m.health === "abandoned" && (
                            <button
                              type="button"
                              className="btn btn-ghost btn-xs text-warning"
                              onClick={() => {
                                gatewayResetModelHealth(g.id, m.id).then(() => refresh());
                              }}
                            >
                              {t("settings.gateway.health.reset")}
                            </button>
                          )}
                          {/* 延迟数值:手动探测优先,否则用 status 里的后台延迟(fastest 模式) */}
                          {(() => {
                            const probe = probeResult[g.id]?.[m.id];
                            // 手动探测优先;否则用 status 里的后台延迟
                            // (null = 探测失败, undefined = 未探测)
                            const lat: string | number | null | undefined =
                              probe !== undefined ? probe : m.latency_ms;
                            if (lat === undefined || lat === null) {
                              // null = 探测过但失败 → ✕;undefined = 未探测 → 不显示
                              if (lat === null) {
                                return <span className="font-mono text-2xs text-error">✕</span>;
                              }
                              return null;
                            }
                            return (
                              <span className="font-mono text-2xs text-base-content/50">
                                {lat}ms
                              </span>
                            );
                          })()}
                        </li>
                      ))}
                    </ul>
                  </div>
                )
              )}
            </li>
          );
        })}
      </ul>

      {/* 新建表单在列表下方 */}
      {edit && edit.id === "" ? (
        editForm
      ) : (
        <div className="flex items-center gap-2">
          <button type="button" className="btn btn-sm btn-outline w-fit" onClick={() => { setExpanded(null); setEdit(emptyGroup()); }}>
            <IconPlus size={14} stroke={2} aria-hidden />
            {t("settings.gateway.groups.add")}
          </button>
          <button
            type="button"
            className="btn btn-sm btn-outline w-fit"
            disabled={vendorPresets.length === 0}
            onClick={() => setImportDialog("create")}
            title={t("settings.gateway.import.fromVendor")}
          >
            <IconDownload size={14} stroke={2} aria-hidden />
            {t("settings.gateway.import.fromVendor")}
          </button>
        </div>
      )}

      {/* 请求日志 */}
      <div className="mt-2 flex items-center gap-2 px-1 text-xs font-bold text-base-content/60">
        <IconArrowsExchange size={13} stroke={2} aria-hidden />
        {t("settings.gateway.log.title")}
        <button type="button" className="btn btn-ghost btn-xs" onClick={refresh} title={t("settings.gateway.log.refresh")}>
          <IconRefresh size={12} stroke={1.75} aria-hidden />
        </button>
        {logFilterActive && (
          <span className="badge badge-info badge-xs">{t("settings.gateway.log.filter.active")}</span>
        )}
      </div>

      {/* Filter bar */}
      <div className="mt-1 flex flex-wrap items-center gap-2 px-1">
        {/* Group filter dropdown */}
        <select
          className="select select-bordered select-xs w-auto"
          value={logFilter.group_id}
          onChange={(e) => setLogFilter((f) => ({ ...f, group_id: e.target.value }))}
          title={t("settings.gateway.log.filter.group")}
        >
          <option value="">{t("settings.gateway.log.filter.groupAll")}</option>
          {(status?.groups ?? []).map((g) => (
            <option key={g.id} value={g.id}>{g.name}</option>
          ))}
        </select>
        {/* Model filter input */}
        <input
          type="text"
          className="input input-bordered input-xs w-32"
          placeholder={t("settings.gateway.log.filter.modelPlaceholder")}
          value={logFilter.model}
          onChange={(e) => setLogFilter((f) => ({ ...f, model: e.target.value }))}
          title={t("settings.gateway.log.filter.model")}
        />
        {/* Status filter dropdown */}
        <select
          className="select select-bordered select-xs w-auto"
          value={logFilter.ok}
          onChange={(e) => setLogFilter((f) => ({ ...f, ok: e.target.value as "" | "true" | "false" }))}
          title={t("settings.gateway.log.filter.status")}
        >
          <option value="">{t("settings.gateway.log.filter.statusAll")}</option>
          <option value="true">{t("settings.gateway.log.filter.statusSuccess")}</option>
          <option value="false">{t("settings.gateway.log.filter.statusFail")}</option>
        </select>
        {/* Search input */}
        <div className="relative">
          <IconSearch size={12} stroke={1.75} aria-hidden className="absolute left-2 top-1/2 -translate-y-1/2 text-base-content/30" />
          <input
            type="text"
            className="input input-bordered input-xs w-48 pl-7"
            placeholder={t("settings.gateway.log.filter.searchPlaceholder")}
            value={logFilter.search}
            onChange={(e) => setLogFilter((f) => ({ ...f, search: e.target.value }))}
            title={t("settings.gateway.log.filter.search")}
          />
        </div>
        {/* Clear filters button */}
        {logFilterActive && (
          <button
            type="button"
            className="btn btn-ghost btn-xs"
            onClick={() => setLogFilter({ group_id: "", model: "", ok: "", search: "" })}
            title={t("settings.gateway.log.filter.clear")}
          >
            <IconX size={12} stroke={1.75} aria-hidden />
            {t("settings.gateway.log.filter.clear")}
          </button>
        )}
      </div>

      {log.length === 0 ? (
        <p className="px-1 text-2xs text-base-content/40">{t("settings.gateway.log.empty")}</p>
      ) : (
        <div className="overflow-x-auto rounded-box border border-base-300 bg-base-100">
          <table className="table table-zebra table-xs">
            <thead>
              <tr>
                <th>{t("settings.gateway.log.time")}</th>
                <th>{t("settings.gateway.log.group")}</th>
                <th>{t("settings.gateway.log.model")}</th>
                <th>{t("settings.gateway.log.status")}</th>
                <th>{t("settings.gateway.log.latency")}</th>
                <th>{t("settings.gateway.log.attempts")}</th>
                <th>{t("settings.gateway.log.tokens")}</th>
                <th>{t("settings.gateway.log.detail.button")}</th>
              </tr>
            </thead>
            <tbody>
              {log.map((e, i) => (
                <tr
                  key={e.id ?? `${e.ts_ms}-${i}`}
                  title={e.error ?? undefined}
                  className={`cursor-pointer hover:bg-base-200 ${e.pending ? "animate-pulse" : ""}`}
                  onClick={() => { void openLogDetail(e); }}
                >
                  <td className="whitespace-nowrap font-mono text-2xs">{formatLogTime(e.ts_ms)}</td>
                  <td className="max-w-32 truncate font-mono text-2xs">{e.group_name}</td>
                  <td className="max-w-40 truncate font-mono text-2xs">
                    {e.pending ? (
                      <span className="inline-flex items-center gap-1">
                        <span className="loading loading-spinner loading-xs" aria-hidden />
                        <span className="truncate">{e.model || t("settings.gateway.log.pending")}</span>
                      </span>
                    ) : (
                      <>
                        {e.model} {e.stream && <span className="badge badge-ghost badge-xs">{t("settings.gateway.log.streamBadge")}</span>}
                      </>
                    )}
                  </td>
                  <td>
                    {e.pending ? (
                      <span className="badge badge-warning badge-soft badge-xs">{t("settings.gateway.log.pending")}</span>
                    ) : e.ok ? (
                      <span className="badge badge-success badge-soft badge-xs">{e.status ?? 200}</span>
                    ) : (
                      <span className="badge badge-error badge-soft badge-xs">{e.status ?? "ERR"}</span>
                    )}
                  </td>
                  <td className="font-mono text-2xs">
                    {e.pending ? "…" : `${e.latency_ms}ms`}
                  </td>
                  <td className="font-mono text-2xs">{e.pending ? "—" : e.attempts}</td>
                  <td className="font-mono text-2xs">
                    {e.pending ? "—" : `${e.prompt_tokens ?? "—"}/${e.completion_tokens ?? "—"}`}
                  </td>
                  <td>
                    <button
                      type="button"
                      className="btn btn-ghost btn-xs"
                      disabled={e.pending}
                      onClick={(event) => {
                        event.stopPropagation();
                        void openLogDetail(e);
                      }}
                    >
                      {t("settings.gateway.log.detail.button")}
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {logTotal > LOG_PAGE_SIZE && (
        <div className="mt-1 flex items-center gap-2 px-1 text-2xs">
          <button
            type="button"
            className="btn btn-ghost btn-xs"
            disabled={logPage === 0}
            onClick={() => setLogPage((p) => Math.max(0, p - 1))}
          >
            {t("settings.gateway.log.page.prev")}
          </button>
          <span className="text-base-content/50">
            {t("settings.gateway.log.page.info", {
              page: logPage + 1,
              total: Math.ceil(logTotal / LOG_PAGE_SIZE),
              count: logTotal,
            })}
          </span>
          <button
            type="button"
            className="btn btn-ghost btn-xs"
            disabled={(logPage + 1) * LOG_PAGE_SIZE >= logTotal}
            onClick={() => setLogPage((p) => p + 1)}
          >
            {t("settings.gateway.log.page.next")}
          </button>
        </div>
      )}

      {/* Log detail modal */}
      {logDetail && (
        <div className="modal modal-open" onClick={() => setLogDetail(null)}>
          <div
            className="modal-box modal-bottom sm:modal-middle max-w-4xl"
            onClick={(e) => e.stopPropagation()}
          >
            <h3 className="text-sm font-bold">{t("settings.gateway.log.detail.title")}</h3>
            <div className="py-2 space-y-3 max-h-[80vh] overflow-y-auto">
              {/* Basic info */}
              <div className="grid grid-cols-2 gap-2 text-xs">
                <div>
                  <span className="text-base-content/50">{t("settings.gateway.log.time")}:</span>{" "}
                  <span className="font-mono">{formatLogTime(logDetail.ts_ms)}</span>
                </div>
                <div>
                  <span className="text-base-content/50">{t("settings.gateway.log.group")}:</span>{" "}
                  <span className="font-mono">{logDetail.group_name}</span>
                </div>
                <div>
                  <span className="text-base-content/50">{t("settings.gateway.log.model")}:</span>{" "}
                  <span className="font-mono">{logDetail.model || "—"}</span>
                </div>
                <div>
                  <span className="text-base-content/50">{t("settings.gateway.log.status")}:</span>{" "}
                  {logDetail.ok ? (
                    <span className="badge badge-success badge-soft badge-xs">{logDetail.status ?? 200}</span>
                  ) : (
                    <span className="badge badge-error badge-soft badge-xs">{logDetail.status ?? "ERR"}</span>
                  )}
                  {logDetail.stream && (
                    <span className="badge badge-ghost badge-xs ml-1">{t("settings.gateway.log.streamBadge")}</span>
                  )}
                </div>
              </div>
              {/* Timing */}
              <div className="grid grid-cols-2 gap-2 text-xs">
                <div>
                  <span className="text-base-content/50">{t("settings.gateway.log.detail.latency")}:</span>{" "}
                  <span className="font-mono">{logDetail.latency_ms}ms</span>
                </div>
                <div>
                  <span className="text-base-content/50">{t("settings.gateway.log.detail.attempts")}:</span>{" "}
                  <span className="font-mono">{logDetail.attempts}</span>
                </div>
              </div>
              {/* Token usage */}
              <div className="grid grid-cols-2 gap-2 text-xs">
                <div>
                  <span className="text-base-content/50">{t("settings.gateway.log.detail.promptTokens")}:</span>{" "}
                  <span className="font-mono">{logDetail.prompt_tokens ?? "—"}</span>
                </div>
                <div>
                  <span className="text-base-content/50">{t("settings.gateway.log.detail.completionTokens")}:</span>{" "}
                  <span className="font-mono">{logDetail.completion_tokens ?? "—"}</span>
                </div>
              </div>
              {/* Error */}
              {logDetail.error && (
                <div className="text-xs">
                  <div className="font-bold text-error">{t("settings.gateway.log.detail.error")}</div>
                  <pre className="mt-1 whitespace-pre-wrap break-words rounded bg-base-200 p-2 font-mono text-2xs text-error">
                    {logDetail.error}
                  </pre>
                </div>
              )}
              {logDetail.request_content && !logDetail.raw_request && (
                <div className="text-xs">
                  <div className="font-bold">{t("settings.gateway.log.detail.request")}</div>
                  <pre className="mt-1 max-h-96 overflow-auto whitespace-pre-wrap break-words rounded bg-base-200 p-2 font-mono text-2xs">
                    {logDetail.request_content}
                  </pre>
                </div>
              )}
              {logDetail.raw_request ? (
                <div className="text-xs">
                  <div className="flex items-center gap-2">
                    <span className="font-bold">{t("settings.gateway.log.detail.rawRequest")}</span>
                    <button
                      type="button"
                      className="btn btn-ghost btn-xs"
                      onClick={() => copyText(logDetail.raw_request ?? "")}
                      title={t("settings.gateway.log.detail.copy")}
                    >
                      <IconCopy size={12} stroke={1.75} aria-hidden />
                    </button>
                    {logDetail.raw_request.includes("…(truncated") && (
                      <span className="text-2xs text-warning">{t("settings.gateway.log.detail.truncated")}</span>
                    )}
                  </div>
                  <pre className="mt-1 max-h-96 overflow-auto whitespace-pre-wrap break-words rounded bg-base-200 p-2 font-mono text-2xs">
                    {formatLogBody(logDetail.raw_request)}
                  </pre>
                </div>
              ) : !logDetail.request_content ? (
                <div className="text-xs text-base-content/40">{t("settings.gateway.log.detail.noBody")}</div>
              ) : null}
              {logDetail.raw_response ? (
                <div className="text-xs">
                  <div className="flex items-center gap-2">
                    <span className="font-bold">{t("settings.gateway.log.detail.rawResponse")}</span>
                    <button
                      type="button"
                      className="btn btn-ghost btn-xs"
                      onClick={() => copyText(logDetail.raw_response ?? "")}
                      title={t("settings.gateway.log.detail.copy")}
                    >
                      <IconCopy size={12} stroke={1.75} aria-hidden />
                    </button>
                    {logDetail.raw_response.includes("…(truncated") && (
                      <span className="text-2xs text-warning">{t("settings.gateway.log.detail.truncated")}</span>
                    )}
                  </div>
                  <pre className="mt-1 max-h-96 overflow-auto whitespace-pre-wrap break-words rounded bg-base-200 p-2 font-mono text-2xs">
                    {formatLogBody(logDetail.raw_response)}
                  </pre>
                </div>
              ) : logDetail.response_content ? (
                <div className="text-xs">
                  <div className="font-bold">{t("settings.gateway.log.detail.response")}</div>
                  <pre className="mt-1 max-h-96 overflow-auto whitespace-pre-wrap break-words rounded bg-base-200 p-2 font-mono text-2xs">
                    {logDetail.response_content}
                  </pre>
                </div>
              ) : null}
            </div>
            <div className="modal-action">
              <button type="button" className="btn btn-ghost btn-sm" onClick={() => setLogDetail(null)}>
                {t("settings.gateway.log.detail.close")}
              </button>
            </div>
          </div>
        </div>
      )}
    </section>

      {/* 测试超时弹窗(2026-09-14):点测试按钮先弹此框输入超时时间 */}
      {testTimeoutOpen && (
        <div className="modal modal-open" onClick={() => setTestTimeoutOpen(null)}>
          <div className="modal-box modal-bottom sm:modal-middle" onClick={(e) => e.stopPropagation()}>
            <h3 className="text-sm font-bold">{t("settings.gateway.testTimeout.title")}</h3>
            <p className="py-2 text-xs text-base-content/50">{t("settings.gateway.testTimeout.hint")}</p>
            <label className="flex items-center gap-2">
              <input
                type="number"
                className="input input-sm w-32 font-mono"
                value={testTimeoutValue}
                onChange={(e) => setTestTimeoutValue(e.target.value)}
                min={500}
                step={500}
                autoFocus
                onKeyDown={(e) => { if (e.key === "Enter") runTest(testTimeoutOpen); }}
              />
              <span className="text-xs text-base-content/50">ms</span>
            </label>
            <div className="modal-action">
              <button type="button" className="btn btn-ghost btn-sm" onClick={() => setTestTimeoutOpen(null)}>
                {t("settings.gateway.cancel")}
              </button>
              <button type="button" className="btn btn-primary btn-sm" onClick={() => runTest(testTimeoutOpen)}>
                {t("settings.gateway.group.test")}
              </button>
            </div>
          </div>
        </div>
      )}

      {/* 从厂商批量导入弹窗(2026-09-14) */}
      {importDialog && (
        <VendorImportDialog
          vendors={vendorPresets}
          onClose={() => setImportDialog(null)}
          onConfirm={async (models, preset) => {
            if (importDialog === "create") {
              // 新建组:填入模型 + 预设参数 + 展开编辑表单
              setExpanded(null);
              // 三级 fallback:models.dev → 厂商预设 → 默认值
              const safeModels = preset
                ? models.map((m) => ({
                    ...m,
                    api_key: m.api_key || preset.api_key,
                  }))
                : models;
              const g = { ...emptyGroup(), models: safeModels };
              if (preset) {
                // 组级 context_window/max_output:厂商预设值(models.dev 的是模型级)
                if (preset.context_window) g.context_window = preset.context_window;
                if (preset.max_output) g.max_output = preset.max_output;
              }
              setEdit(g);
            } else if (edit) {
              // 编辑组:追加模型(防御性:从 preset 补 api_key)
              const safeModels = preset
                ? models.map((m) => ({ ...m, api_key: m.api_key || preset.api_key }))
                : models;
              setEdit({ ...edit, models: [...edit.models, ...safeModels] });
            }
            setImportDialog(null);
          }}
        />
      )}
    </>
  );
}
