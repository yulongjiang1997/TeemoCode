// 模型网关:统一大模型调度平台(模型组调度/故障切换/组级共享上下文)。
// 与 SkillsSection 同类:网关有自己的命令面(gateway_*),不进设置页
// save_config 保存条——本分区"改了即生效",没有脏状态管理。
// 行形态照 SkillsSection(list-row + 行内展开编辑);删除用两段确认
// (第一次点变红为"确认删除",失焦/超时还原),不引入弹窗。
import { IconArrowsExchange, IconChevronDown, IconCopy, IconPlus, IconRefresh, IconTrash } from "@tabler/icons-react";
import { useCallback, useEffect, useRef, useState } from "react";

import { useI18n } from "@/lib/i18n";
import { getConfig, type HostModel } from "@/lib/ipc/config";
import {
  gatewayDeleteGroup,
  gatewayEndpoint,
  gatewayLog,
  gatewayRegenKey,
  gatewaySaveGroup,
  gatewayStatus,
  gatewayTestGroup,
  gatewayUpdateSettings,
  type GatewayLogEntry,
  type GatewayStatus,
  type GroupModel,
  type ModelGroup,
} from "@/lib/ipc/gateway";
import { inDesktopShell } from "@/lib/ipc/ipc";
import { copyText } from "@/lib/util/clipboard";

function errText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

const HEALTH_BADGE: Record<string, string> = {
  healthy: "badge-success",
  degraded: "badge-warning",
  open: "badge-error",
  probing: "badge-info",
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

/** 毫秒时间戳 → HH:mm:ss(日志表用,不引 dayjs 的重格式化)。 */
function hhmmss(tsMs: number): string {
  const d = new Date(tsMs);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

export function GatewaySection() {
  const { t } = useI18n();
  const [status, setStatus] = useState<GatewayStatus | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [library, setLibrary] = useState<HostModel[]>([]);
  // 展开的组(查看态);编辑中的组 id(edit 非 null 时显示表单)
  const [expanded, setExpanded] = useState<string | null>(null);
  const [edit, setEdit] = useState<ModelGroup | null>(null);
  const [testResult, setTestResult] = useState<Record<string, { ok: boolean; text: string }>>({});
  const [log, setLog] = useState<GatewayLogEntry[]>([]);
  // 端口草稿:与保存值不同时出现「应用」按钮
  const [portDraft, setPortDraft] = useState<string | null>(null);
  // 两段确认删除:记下待确认的组 id
  const [confirmingDelete, setConfirmingDelete] = useState<string | null>(null);
  const confirmTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const refresh = useCallback(() => {
    gatewayStatus()
      .then((s) => {
        setStatus(s);
        setLoadError(null);
      })
      .catch((e) => setLoadError(errText(e)));
    gatewayLog(50)
      .then(setLog)
      .catch(() => {});
  }, []);
  useEffect(refresh, [refresh]);

  // 模型库清单(引用条目的下拉来源;浏览器模式为空)
  useEffect(() => {
    if (!inDesktopShell()) return;
    getConfig()
      .then((cfg) => setLibrary(cfg?.models ?? []))
      .catch(() => {});
  }, []);

  // 运行态/日志 5s 轮询(分区可见期间),卸载即停
  useEffect(() => {
    const timer = setInterval(refresh, 5000);
    return () => clearInterval(timer);
  }, [refresh]);

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

  const testGroup = (id: string) => {
    setTestResult((prev) => ({ ...prev, [id]: { ok: true, text: t("settings.gateway.group.testing") } }));
    gatewayTestGroup(id)
      .then((r) => {
        setTestResult((prev) => ({
          ...prev,
          [id]: r.ok
            ? { ok: true, text: t("settings.gateway.group.testOk", { model: r.model ?? "", latency: r.latency_ms }) }
            : { ok: false, text: t("settings.gateway.group.testFailed", { error: r.error ?? "" }) },
        }));
        refresh();
      })
      .catch((e) =>
        setTestResult((prev) => ({ ...prev, [id]: { ok: false, text: t("settings.gateway.group.testFailed", { error: errText(e) }) } })),
      );
  };

  const regenKey = (g: ModelGroup) => {
    if (!window.confirm(t("settings.gateway.group.regenConfirm"))) return;
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
                    type="number"
                    className="input input-xs w-16 font-mono"
                    min={1}
                    max={100}
                    value={m.weight}
                    title={t("settings.gateway.model.weightHint")}
                    onChange={(e) => {
                      const models = [...edit.models];
                      models[idx] = { ...m, weight: Number(e.target.value) || 1 };
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
                    {library.map((lm) => (
                      <option key={lm.name} value={lm.name}>
                        {lm.name} ({lm.model})
                      </option>
                    ))}
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
                    <input
                      className="input input-xs w-full font-mono"
                      value={m.model}
                      onChange={(e) => {
                        const models = [...edit.models];
                        models[idx] = { ...m, model: e.target.value };
                        setEdit({ ...edit, models });
                      }}
                    />
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
                    {t(g.strategy === "weighted" ? "settings.gateway.group.strategy.weighted" : "settings.gateway.group.strategy.priority")}
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
                    testGroup(g.id);
                  }}
                >
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
                      <button type="button" className="btn btn-ghost btn-xs" disabled={busy} onClick={() => regenKey(g)}>
                        <IconRefresh size={12} stroke={1.75} aria-hidden />
                        {t("settings.gateway.group.regen")}
                      </button>
                    </div>
                    <p className="text-2xs text-base-content/40">{t("settings.gateway.group.keyHint")}</p>
                    <ul className="flex flex-col gap-1">
                      {g.models.map((m) => (
                        <li key={m.id} className="flex flex-wrap items-center gap-2">
                          <span className={`badge badge-soft badge-xs ${HEALTH_BADGE[m.health] ?? ""}`}>
                            {t(`gateway.health.${m.health}`)}
                          </span>
                          <span className="font-mono">{m.alias || m.model}</span>
                          <span className="text-base-content/40">w{m.weight}</span>
                          {!m.enabled && <span className="badge badge-ghost badge-xs">{t("settings.gateway.group.disable")}</span>}
                          {m.unavailable && <span className="text-error">{m.unavailable}</span>}
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
        </div>
      )}

      {/* 请求日志 */}
      <div className="mt-2 flex items-center gap-2 px-1 text-xs font-bold text-base-content/60">
        <IconArrowsExchange size={13} stroke={2} aria-hidden />
        {t("settings.gateway.log.title")}
        <button type="button" className="btn btn-ghost btn-xs" onClick={refresh} title={t("settings.gateway.log.refresh")}>
          <IconRefresh size={12} stroke={1.75} aria-hidden />
        </button>
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
              </tr>
            </thead>
            <tbody>
              {[...log].reverse().map((e, i) => (
                <tr key={`${e.ts_ms}-${i}`} title={e.error ?? undefined}>
                  <td className="font-mono text-2xs">{hhmmss(e.ts_ms)}</td>
                  <td className="max-w-32 truncate font-mono text-2xs">{e.group_name}</td>
                  <td className="max-w-40 truncate font-mono text-2xs">
                    {e.model} {e.stream && <span className="badge badge-ghost badge-xs">{t("settings.gateway.log.streamBadge")}</span>}
                  </td>
                  <td>
                    {e.ok ? (
                      <span className="badge badge-success badge-soft badge-xs">{e.status ?? 200}</span>
                    ) : (
                      <span className="badge badge-error badge-soft badge-xs">{e.status ?? "ERR"}</span>
                    )}
                  </td>
                  <td className="font-mono text-2xs">{e.latency_ms}ms</td>
                  <td className="font-mono text-2xs">{e.attempts}</td>
                  <td className="font-mono text-2xs">
                    {e.prompt_tokens ?? "—"}/{e.completion_tokens ?? "—"}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}
