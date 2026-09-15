// 从厂商批量导入模型的弹窗(2026-09-14)。
//
// 流程:选厂商预设 → 拉取模型列表 → 勾选 → 确认
// 用法:<VendorImportDialog vendors={presets} onConfirm={(models) => ...} onClose={() => ...} />
import { useState } from "react";

import { useI18n } from "@/lib/i18n";
import { fetchModelIds } from "@/lib/ipc/config";
import type { VendorPreset, GroupModel, ModelGroup } from "@/lib/ipc/gateway";

const errMsg = (e: unknown): string => (e instanceof Error ? e.message : String(e));

function emptyModel(preset: VendorPreset): GroupModel {
  return {
    id: "",
    enabled: true,
    weight: 1,
    alias: "",
    provider: preset.provider,
    base_url: preset.base_url,
    api_key: preset.api_key,
    model: "",
  };
}

/** 从厂商预设提取组级参数(2026-09-15):导入时填入新组的 context_window 等。 */
export function presetGroupParams(preset: VendorPreset): Partial<ModelGroup> {
  return {
    context_window: preset.context_window || 0,
    max_output: preset.max_output || 0,
    system_prompt: "",
  };
}

export function VendorImportDialog({
  vendors,
  onConfirm,
  onClose,
}: {
  vendors: VendorPreset[];
  onConfirm: (models: GroupModel[], preset?: VendorPreset) => void;
  onClose: () => void;
}) {
  const { t } = useI18n();
  const [step, setStep] = useState<"select" | "models">("select");
  const [selectedPreset, setSelectedPreset] = useState<VendorPreset | null>(null);
  const [modelIds, setModelIds] = useState<string[]>([]);
  const [checked, setChecked] = useState<Set<string>>(new Set());
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [search, setSearch] = useState("");

  const selectVendor = async (v: VendorPreset) => {
    setSelectedPreset(v);
    setLoading(true);
    setError(null);
    try {
      const ids = await fetchModelIds(v.provider, v.base_url, v.api_key);
      setModelIds(ids);
      setStep("models");
    } catch (e) {
      setError(errMsg(e));
    } finally {
      setLoading(false);
    }
  };

  const toggle = (id: string) => {
    setChecked((prev) => {
      const n = new Set(prev);
      if (n.has(id)) n.delete(id); else n.add(id);
      return n;
    });
  };

  const toggleAll = () => {
    if (checked.size === filteredIds.length) {
      setChecked(new Set());
    } else {
      setChecked(new Set(filteredIds));
    }
  };

  const filteredIds = search
    ? modelIds.filter((id) => id.toLowerCase().includes(search.toLowerCase()))
    : modelIds;

  const confirm = () => {
    if (!selectedPreset) return;
    const models = Array.from(checked).map((id) => ({ ...emptyModel(selectedPreset), model: id }));
    onConfirm(models, selectedPreset);
  };

  return (
    <div className="modal modal-open" onClick={onClose}>
      <div className="modal-box max-w-2xl" onClick={(e) => e.stopPropagation()}>
        {/* 步骤 1:选厂商 */}
        {step === "select" && (
          <>
            <h3 className="text-sm font-bold">{t("settings.gateway.import.selectVendor")}</h3>
            <div className="py-2" />
            {vendors.length === 0 ? (
              <p className="py-4 text-center text-sm text-base-content/40">
                {t("settings.gateway.vendors.empty")}
              </p>
            ) : loading ? (
              <div className="flex items-center gap-2 py-4 text-sm text-base-content/40">
                <span className="loading loading-spinner loading-sm" />
                {t("settings.models.fetch")}
              </div>
            ) : (
              <div className="flex flex-col gap-1.5">
                {vendors.map((v) => (
                  <button
                    key={v.id}
                    type="button"
                    className="flex items-center gap-2 rounded-box border border-base-300 px-3 py-2 text-left text-sm hover:bg-base-200"
                    onClick={() => selectVendor(v)}
                  >
                    <span className="min-w-0 flex-1 truncate font-medium">{v.name}</span>
                    <span className="badge badge-ghost badge-xs shrink-0">{v.provider}</span>
                    <span className="hidden max-w-40 truncate font-mono text-2xs text-base-content/40 sm:block">{v.base_url}</span>
                  </button>
                ))}
              </div>
            )}
            {error && <p className="py-2 text-xs text-error">{error}</p>}
            <div className="modal-action">
              <button type="button" className="btn btn-ghost btn-sm" onClick={onClose}>
                {t("settings.gateway.cancel")}
              </button>
            </div>
          </>
        )}

        {/* 步骤 2:勾选模型 */}
        {step === "models" && (
          <>
            <div className="flex items-center gap-2">
              <h3 className="text-sm font-bold">{t("settings.gateway.import.selectModels")}</h3>
              <span className="badge badge-ghost badge-xs">{selectedPreset?.name}</span>
            </div>
            <div className="flex items-center gap-2 py-2">
              <input
                className="input input-xs flex-1"
                placeholder={t("settings.gateway.import.searchPlaceholder")}
                value={search}
                onChange={(e) => setSearch(e.target.value)}
              />
              <span className="text-2xs text-base-content/50">
                {checked.size}/{filteredIds.length}
              </span>
              <button type="button" className="btn btn-ghost btn-xs" onClick={toggleAll}>
                {checked.size === filteredIds.length && filteredIds.length > 0
                  ? t("settings.gateway.import.deselectAll")
                  : t("settings.gateway.import.selectAll")}
              </button>
            </div>
            {loading ? (
              <div className="flex items-center gap-2 py-4 text-sm text-base-content/40">
                <span className="loading loading-spinner loading-sm" />
                {t("settings.models.fetch")}
              </div>
            ) : filteredIds.length === 0 ? (
              <p className="py-4 text-center text-sm text-base-content/40">
                {t("settings.models.fetch.empty")}
              </p>
            ) : (
              <div className="max-h-80 overflow-y-auto rounded-box border border-base-300">
                {filteredIds.map((id) => (
                  <label
                    key={id}
                    className="flex cursor-pointer items-center gap-2 border-b border-base-200 px-3 py-1.5 hover:bg-base-200"
                  >
                    <input
                      type="checkbox"
                      className="checkbox checkbox-xs"
                      checked={checked.has(id)}
                      onChange={() => toggle(id)}
                    />
                    <span className="min-w-0 flex-1 truncate font-mono text-xs">{id}</span>
                  </label>
                ))}
              </div>
            )}
            <div className="modal-action">
              <button type="button" className="btn btn-ghost btn-sm" onClick={() => { setStep("select"); setChecked(new Set()); setModelIds([]); }}>
                {t("settings.gateway.import.back")}
              </button>
              <button type="button" className="btn btn-primary btn-sm" disabled={checked.size === 0} onClick={confirm}>
                {t("settings.gateway.import.import", { n: checked.size })}
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
