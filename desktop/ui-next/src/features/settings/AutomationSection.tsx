// 自动化定时任务:对标 ZCode 的 Scheduled Automations。
// 设置页分区:任务列表(名称/计划/上次结果/启停) + 编辑表单
// (目标/cron|一次性时间/提示词/模型) + 立即运行。
import { IconChevronDown, IconPlayerPlay, IconPlus, IconTrash } from "@tabler/icons-react";
import { useCallback, useEffect, useState } from "react";

import { useI18n } from "@/lib/i18n";
import { inDesktopShell } from "@/lib/ipc/ipc";
import { getConfig, type HostModel } from "@/lib/ipc/config";
import {
  automationDelete,
  automationList,
  automationRunNow,
  automationSave,
  type Automation,
} from "@/lib/ipc/automation";
import { cronNextPreview } from "@/lib/util/cronNext";

function errText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

function draft(): Automation {
  return {
    id: "",
    name: "",
    enabled: true,
    kind: "cron",
    cron: "0 9 * * *",
    fire_at_ms: 0,
    prompt: "",
    kind_session: "chat",
    workdir: "",
    model: "",
    last_fire_ms: 0,
    last_result: "",
  };
}

export function AutomationSection() {
  const { t } = useI18n();
  const [list, setList] = useState<Automation[]>([]);
  const [loadErr, setLoadErr] = useState<string | null>(null);
  const [edit, setEdit] = useState<Automation | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [models, setModels] = useState<HostModel[]>([]);
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);

  const refresh = useCallback(() => {
    automationList()
      .then((l) => { setList(l); setLoadErr(null); })
      .catch((e: unknown) => setLoadErr(errText(e)));
  }, []);
  useEffect(refresh, [refresh]);
  useEffect(() => {
    if (!inDesktopShell()) return;
    getConfig().then((cfg) => setModels(cfg?.models ?? [])).catch(() => {});
  }, []);

  const save = () => {
    if (!edit) return;
    const name = edit.name.trim();
    const prompt = edit.prompt.trim();
    if (!name || !prompt) {
      setError(t("automation.validation"));
      return;
    }
    if (edit.kind_session === "local" && !edit.workdir.trim()) {
      setError(t("automation.validation"));
      return;
    }
    if (edit.kind === "cron" && !edit.cron.trim()) {
      setError(t("automation.validation"));
      return;
    }
    if (edit.kind === "once" && !edit.fire_at_ms) {
      setError(t("automation.validation"));
      return;
    }
    setBusy(true);
    setError(null);
    automationSave(edit)
      .then(() => { setEdit(null); refresh(); })
      .catch((e: unknown) => setError(errText(e)))
      .finally(() => setBusy(false));
  };

  const remove = (a: Automation) => {
    if (confirmDelete !== a.id) {
      setConfirmDelete(a.id);
      return;
    }
    setConfirmDelete(null);
    setBusy(true);
    automationDelete(a.id)
      .then(refresh)
      .catch((e: unknown) => setError(errText(e)))
      .finally(() => setBusy(false));
  };

  const runNow = (a: Automation) => {
    setError(null);
    setNotice(t("automation.running"));
    automationRunNow(a.id)
      .then((r) => {
        setNotice(r.ok ? t("automation.runOk") : t("automation.runFailed", { msg: r.detail }));
        refresh();
      })
      .catch((e: unknown) => { setError(errText(e)); setNotice(null); });
  };

  const cronPreview = (expr: string) => {
    const r = cronNextPreview(expr);
    return r ? r.label : t("automation.cron.invalid");
  };

  // ---- 编辑表单 ----
  const editForm = edit && (
    <div className="flex flex-col gap-2.5 rounded-box border border-base-300 bg-base-100 p-4">
      <fieldset className="fieldset gap-1.5">
        <legend className="fieldset-legend">{t("automation.name")}</legend>
        <input
          className="input input-sm w-full"
          value={edit.name}
          onChange={(e) => setEdit({ ...edit, name: e.target.value })}
        />
      </fieldset>
      <div className="grid grid-cols-2 gap-2.5">
        <fieldset className="fieldset gap-1.5">
          <legend className="fieldset-legend">{t("automation.target")}</legend>
          <select
            className="select select-sm w-full"
            value={edit.kind_session}
            onChange={(e) => setEdit({ ...edit, kind_session: e.target.value })}
          >
            <option value="chat">{t("automation.targetChat")}</option>
            <option value="local">{t("automation.targetLocal")}</option>
          </select>
        </fieldset>
        <fieldset className="fieldset gap-1.5">
          <legend className="fieldset-legend">{t("automation.plan")}</legend>
          <select
            className="select select-sm w-full"
            value={edit.kind}
            onChange={(e) => setEdit({ ...edit, kind: e.target.value as Automation["kind"] })}
          >
            <option value="cron">{t("automation.planCron")}</option>
            <option value="once">{t("automation.planOnce")}</option>
          </select>
        </fieldset>
      </div>
      {edit.kind === "local" && (
        <fieldset className="fieldset gap-1.5">
          <legend className="fieldset-legend">{t("automation.workdir")}</legend>
          <input
            className="input input-sm w-full font-mono text-xs"
            value={edit.workdir}
            placeholder="C:\projects\my-repo"
            onChange={(e) => setEdit({ ...edit, workdir: e.target.value })}
          />
        </fieldset>
      )}
      {edit.kind === "cron" ? (
        <div className="flex flex-col gap-1">
          <input
            className="input input-sm w-full font-mono text-xs"
            value={edit.cron}
            placeholder="0 9 * * 1-5"
            onChange={(e) => setEdit({ ...edit, cron: e.target.value })}
          />
          <span className="text-[10px] text-base-content/50">{t("automation.next")}: {cronPreview(edit.cron)}</span>
        </div>
      ) : (
        <fieldset className="fieldset gap-1.5">
          <legend className="fieldset-legend">{t("automation.onceAt")}</legend>
          <input
            className="input input-sm w-full"
            type="datetime-local"
            value={edit.fire_at_ms ? toLocalDatetime(edit.fire_at_ms) : ""}
            onChange={(e) => setEdit({ ...edit, fire_at_ms: e.target.value ? new Date(e.target.value).getTime() : 0 })}
          />
        </fieldset>
      )}
      <fieldset className="fieldset gap-1.5">
        <legend className="fieldset-legend">{t("automation.model")}</legend>
        <select
          className="select select-sm w-full"
          value={edit.model}
          onChange={(e) => setEdit({ ...edit, model: e.target.value })}
        >
          <option value="">{t("automation.modelDefault")}</option>
          {models.map((m) => (
            <option key={m.name} value={m.name}>{m.name}</option>
          ))}
        </select>
      </fieldset>
      <fieldset className="fieldset gap-1.5">
        <legend className="fieldset-legend">{t("automation.prompt")}</legend>
        <textarea
          className="textarea textarea-xs min-h-24 w-full resize-y text-xs"
          value={edit.prompt}
          onChange={(e) => setEdit({ ...edit, prompt: e.target.value })}
        />
      </fieldset>
      <div className="flex gap-1.5">
        <button type="button" className="btn btn-primary btn-sm" disabled={busy} onClick={save}>{t("automation.save")}</button>
        <button type="button" className="btn btn-ghost btn-sm" onClick={() => setEdit(null)}>{t("automation.cancel")}</button>
      </div>
    </div>
  );

  return (
    <section aria-label={t("settings.nav.automation")} className="flex flex-col gap-2">
      <div className="rounded-box border border-base-300">
        <div className="flex flex-col gap-1 border-b border-base-300/70 p-3">
          <div className="text-sm font-semibold">{t("settings.nav.automation")}</div>
          <p className="text-xs text-base-content/60">{t("automation.hint")}</p>
        </div>
        {loadErr && <div className="px-3 pt-2 text-[11px] text-error">{loadErr}</div>}
        {error && <div className="px-3 pt-2 text-[11px] text-error">{error}</div>}
        {notice && <div className="px-3 pt-2 text-[11px] text-success">{notice}</div>}
        <ul className="flex flex-col gap-1.5 p-3">
          {list.length === 0 && !edit && (
            <li className="text-xs text-base-content/40">{t("automation.empty")}</li>
          )}
          {list.map((a) => {
            const isOpen = expandedId === a.id;
            return (
              <li key={a.id} className="flex flex-col gap-1 rounded-box border border-base-300/70 bg-base-200/40 px-3 py-2">
                <div className="flex items-center gap-2">
                  <input
                    type="checkbox"
                    className="toggle toggle-xs"
                    checked={a.enabled}
                    aria-label={t("automation.enableToggle")}
                    onChange={() => {
                      const next = { ...a, enabled: !a.enabled };
                      automationSave(next).then(refresh).catch((e: unknown) => setError(errText(e)));
                    }}
                  />
                  <div className="min-w-0 flex-1 cursor-pointer" onClick={() => setExpandedId(isOpen ? null : a.id)}>
                    <span className="text-xs font-semibold truncate">{a.name || t("automation.unnamed")}</span>
                    <span className={`badge badge-soft badge-xs ml-1.5 ${a.kind === "cron" ? "badge-info" : "badge-warning"}`}>{a.kind === "cron" ? a.cron : t("automation.planOnce")}</span>
                    {a.expired && <span className="badge badge-ghost badge-xs ml-1 text-warning">{t("automation.expired")}</span>}
                    {a.last_result && <span className="ml-1.5 max-w-40 truncate text-[10px] text-base-content/50" title={a.last_result}>{a.last_result.split(":")[0]}{a.last_fire_ms ? ` ${new Date(a.last_fire_ms).toLocaleString()}` : ""}</span>}
                  </div>
                  <button type="button" className="btn btn-ghost btn-xs" disabled={busy} title={t("automation.runNow")} onClick={() => runNow(a)}>
                    <IconPlayerPlay size={12} stroke={1.75} aria-hidden />
                  </button>
                  <button type="button" className="btn btn-ghost btn-xs text-base-content/60" onClick={() => { setExpandedId(a.id); setEdit({ ...a }); setConfirmDelete(null); }}>{t("automation.edit")}</button>
                  <button
                    type="button"
                    className={`btn btn-ghost btn-xs ${confirmDelete === a.id ? "text-error" : "text-base-content/40 hover:text-error"}`}
                    disabled={busy}
                    onClick={() => remove(a)}
                    onBlur={() => setConfirmDelete(null)}
                  >
                    <IconTrash size={12} stroke={1.75} aria-hidden />
                    {confirmDelete === a.id ? t("automation.deleteConfirm") : ""}
                  </button>
                  <IconChevronDown size={14} stroke={1.75} aria-hidden
                    className={`shrink-0 cursor-pointer text-base-content/40 transition-transform ${isOpen ? "rotate-180" : ""}`}
                    onClick={() => setExpandedId(isOpen ? null : a.id)}
                  />
                </div>
                {isOpen && !edit?.id && (
                  <div className="border-t border-base-300/70 px-2 pt-2 pb-1 text-xs text-base-content/60">
                    <span className="mr-2">{t("automation.detailSession")}: {a.kind_session}</span>
                    {a.kind_session === "local" && <span className="mr-2 font-mono">{a.workdir || "—"}</span>}
                    {a.model && <span>{t("automation.detailModel")}: {a.model}</span>}
                  </div>
                )}
              </li>
            );
          })}
        </ul>
        {editForm}
        {!edit && (
          <div className="border-t border-base-300/70 p-3">
            <button type="button" className="btn btn-outline btn-sm w-fit" onClick={() => { setExpandedId(null); setEdit(draft()); }}>
              <IconPlus size={14} stroke={2} aria-hidden />
              {t("automation.add")}
            </button>
          </div>
        )}
      </div>
    </section>
  );
}

function toLocalDatetime(ms: number): string {
  const d = new Date(ms);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
}
