// 厂商预设管理 tab(2026-09-14)。
//
// 用户自建厂商接入预设:名称 + provider + base_url + api_key,
// 存 config.json(gateway_save_vendors 全量写回)。
// 保存后折叠为摘要行(名称 + 协议 + 地址),点击展开可编辑。
import { IconChevronDown, IconPlus, IconRefresh, IconTrash } from "@tabler/icons-react";
import { useCallback, useEffect, useState } from "react";

import { useI18n } from "@/lib/i18n";
import { getConfig, fetchModelIds } from "@/lib/ipc/config";
import { gatewaySaveVendors, type VendorPreset } from "@/lib/ipc/gateway";

const errMsg = (e: unknown): string => (e instanceof Error ? e.message : String(e));

function emptyPreset(): VendorPreset {
  return { id: "", name: "", provider: "openai", base_url: "", api_key: "" };
}

export function VendorsTab() {
  const { t } = useI18n();
  const [vendors, setVendors] = useState<VendorPreset[]>([]);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /** 展开的厂商行 index(null = 全折叠) */
  const [expanded, setExpanded] = useState<number | null>(null);
  /** 各行拉取的模型列表(key = row index) */
  const [fetched, setFetched] = useState<Record<number, { ids: string[]; error?: string }>>({});
  const [fetching, setFetching] = useState<Set<number>>(new Set());
  /** 标记新增未保存的行 */
  const [dirty, setDirty] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const cfg = await getConfig();
      if (cfg?.gateway?.vendor_presets) {
        setVendors(cfg.gateway.vendor_presets);
      }
    } catch (e) {
      setError(errMsg(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => { void load(); }, [load]);

  const save = async () => {
    setSaving(true);
    setError(null);
    try {
      const saved = await gatewaySaveVendors(vendors);
      setVendors(saved);
      setDirty(false);
      setExpanded(null);
    } catch (e) {
      setError(errMsg(e));
    } finally {
      setSaving(false);
    }
  };

  const update = (idx: number, patch: Partial<VendorPreset>) => {
    setVendors((prev) => prev.map((v, i) => (i === idx ? { ...v, ...patch } : v)));
    setDirty(true);
  };

  const add = () => {
    setVendors((prev) => [...prev, emptyPreset()]);
    setDirty(true);
    setExpanded(vendors.length);
  };

  const remove = (idx: number) => {
    setVendors((prev) => prev.filter((_, i) => i !== idx));
    setFetched((prev) => { const n = { ...prev }; delete n[idx]; return n; });
    setDirty(true);
    setExpanded(null);
  };

  const fetchList = async (idx: number) => {
    const v = vendors[idx];
    if (!v || fetching.has(idx)) return;
    const next = new Set(fetching); next.add(idx); setFetching(next);
    try {
      const ids = await fetchModelIds(v.provider, v.base_url, v.api_key);
      setFetched((prev) => ({ ...prev, [idx]: ids.length ? { ids } : { ids: [], error: t("settings.models.fetch.empty") } }));
    } catch (e) {
      setFetched((prev) => ({ ...prev, [idx]: { ids: [], error: errMsg(e) } }));
    } finally {
      const rest = new Set(fetching); rest.delete(idx); setFetching(rest);
    }
  };

  if (loading) {
    return (
      <div className="flex items-center gap-2 py-4 text-sm text-base-content/40">
        <span className="loading loading-spinner loading-sm" />
        {t("settings.models.save")}
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-3">
      <p className="text-xs leading-relaxed text-base-content/50">
        {t("settings.gateway.vendors.hint")}
      </p>

      {vendors.length === 0 ? (
        <p className="py-4 text-center text-sm text-base-content/40">
          {t("settings.gateway.vendors.empty")}
        </p>
      ) : (
        <div className="flex flex-col gap-1.5">
          {vendors.map((v, idx) => {
            const isOpen = expanded === idx;
            return (
              <div key={v.id || `new-${idx}`} className="rounded-box border border-base-300 bg-base-100">
                {/* 折叠态:一行摘要(名称 + 协议 + 地址) */}
                <button
                  type="button"
                  className="flex w-full items-center gap-2 px-3 py-2 text-left text-sm"
                  onClick={() => setExpanded(isOpen ? null : idx)}
                >
                  <IconChevronDown
                    size={13}
                    stroke={2}
                    className={`shrink-0 transition-transform ${isOpen ? "" : "-rotate-90"}`}
                    aria-hidden
                  />
                  <span className="min-w-0 flex-1 truncate font-medium">{v.name || t("settings.gateway.vendors.namePlaceholder")}</span>
                  <span className="badge badge-ghost badge-xs shrink-0">{v.provider}</span>
                  <span className="hidden max-w-40 truncate font-mono text-2xs text-base-content/40 sm:block">{v.base_url}</span>
                </button>
                {/* 展开态:完整编辑表单 */}
                {isOpen && (
                  <div className="flex flex-col gap-2 border-t border-base-300 p-3">
                    <div className="flex items-center gap-2">
                      <input
                        className="input input-sm min-w-0 flex-1"
                        placeholder={t("settings.gateway.vendors.namePlaceholder")}
                        value={v.name}
                        onChange={(e) => update(idx, { name: e.target.value })}
                      />
                      <button
                        type="button"
                        className="btn btn-ghost btn-xs shrink-0 text-error"
                        onClick={() => remove(idx)}
                      >
                        <IconTrash size={13} stroke={1.75} aria-hidden />
                      </button>
                    </div>
                    <div className="grid grid-cols-2 gap-2">
                      <label className="flex flex-col gap-1 text-2xs">
                        {t("settings.gateway.model.provider")}
                        <select
                          className="select select-xs w-full"
                          value={v.provider}
                          onChange={(e) => update(idx, { provider: e.target.value })}
                        >
                          <option value="openai">OpenAI</option>
                          <option value="anthropic">Anthropic</option>
                          <option value="openai_responses">OpenAI Responses</option>
                        </select>
                      </label>
                      <label className="flex flex-col gap-1 text-2xs">
                        {t("settings.gateway.model.baseUrl")}
                        <input
                          className="input input-xs w-full font-mono"
                          placeholder="https://…"
                          value={v.base_url}
                          onChange={(e) => update(idx, { base_url: e.target.value })}
                        />
                      </label>
                      <label className="col-span-2 flex flex-col gap-1 text-2xs">
                        {t("settings.gateway.model.apiKey")}
                        <input
                          type="password"
                          className="input input-xs w-full font-mono"
                          placeholder="sk-…"
                          value={v.api_key}
                          onChange={(e) => update(idx, { api_key: e.target.value })}
                        />
                      </label>
                    </div>
                    {/* 测试获取模型列表 */}
                    <div className="flex items-center gap-2">
                      <button
                        type="button"
                        className="btn btn-ghost btn-xs"
                        disabled={fetching.has(idx) || !v.base_url}
                        onClick={() => fetchList(idx)}
                      >
                        {fetching.has(idx) ? (
                          <span className="loading loading-spinner loading-xs" aria-hidden />
                        ) : (
                          <IconRefresh size={12} stroke={1.75} aria-hidden />
                        )}
                        {t("settings.models.fetch")}
                      </button>
                      {fetched[idx]?.ids && fetched[idx]!.ids.length > 0 && (
                        <span className="text-2xs text-base-content/50">
                          {t("settings.gateway.vendors.modelsFound", { n: fetched[idx]!.ids.length })}
                        </span>
                      )}
                      {fetched[idx]?.error && (
                        <span className="text-2xs text-error">{fetched[idx]!.error}</span>
                      )}
                    </div>
                  </div>
                )}
              </div>
            );
          })}
        </div>
      )}

      <div className="flex items-center gap-2">
        <button type="button" className="btn btn-outline btn-sm" onClick={add}>
          <IconPlus size={13} stroke={2} aria-hidden />
          {t("settings.gateway.vendors.add")}
        </button>
        {dirty && (
          <button
            type="button"
            className="btn btn-primary btn-sm"
            disabled={saving}
            onClick={save}
          >
            {saving ? <span className="loading loading-spinner loading-xs" /> : null}
            {t("settings.gateway.vendors.save")}
          </button>
        )}
        {error && <span className="text-xs text-error">{error}</span>}
      </div>
    </div>
  );
}
