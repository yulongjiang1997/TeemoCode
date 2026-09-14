// 模型配置独立 tab(2026-09-14:从设置页迁移到网关空间)。
//
// ModelsSection 依赖 SettingsView 的 config 草稿机制(save_config 全量写回)。
// 本组件自建最小 config 草稿:只加载 models 字段,保存时回写全量 config
// (壳自有偏好以磁盘合并,不会丢其他字段)。与 SettingsView 的 models 分区
// 完全同源——同样的 getConfig/draftFromConfig/saveConfig 链路。
import { useCallback, useEffect, useRef, useState } from "react";

import { useI18n } from "@/lib/i18n";
import { getConfig, saveConfig } from "@/lib/ipc/config";
import { ModelsSection } from "@/features/settings/ModelsSection";
import {
  buildPayload,
  draftFromConfig,
  payloadEquals,
  validateDraft,
  type SettingsDraft,
} from "@/features/settings/settingsForm";

const errMsg = (e: unknown): string => (e instanceof Error ? e.message : String(e));

export function ModelsTab() {
  const { t } = useI18n();
  const [draft, setDraft] = useState<SettingsDraft | null>(null);
  const [cfg, setCfg] = useState<Awaited<ReturnType<typeof getConfig>> | null>(null);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState("");
  const draftRef = useRef<SettingsDraft | null>(null);
  const savingRef = useRef(false);

  useEffect(() => {
    let alive = true;
    getConfig()
      .then((loaded) => {
        if (!alive || !loaded) return;
        setCfg(loaded);
        setDraft(draftFromConfig(loaded));
      })
      .catch(() => {});
    return () => { alive = false; };
  }, []);

  // 同步 draftRef(saving 循环中读最新值)
  useEffect(() => { draftRef.current = draft; }, [draft]);

  const updateDraft = useCallback((up: (d: SettingsDraft) => SettingsDraft) => {
    setDraft((d) => (d ? up(d) : d));
  }, []);

  const dirty = (() => {
    if (!cfg || !draft) return false;
    return !payloadEquals(buildPayload(cfg, draftFromConfig(cfg)), buildPayload(cfg, draft));
  })();

  const handleSave = useCallback(async () => {
    const conf = cfg;
    let d = draftRef.current;
    if (!conf || !d) return;
    const invalid = validateDraft(d);
    if (invalid) { setSaveError(JSON.stringify(invalid)); return; }
    setSaving(true);
    savingRef.current = true;
    setSaveError("");
    try {
      for (let round = 0; ; round++) {
        const p = buildPayload(conf, d);
        await saveConfig(p);
        setCfg(p);
        const cur = draftRef.current;
        if (!cur || payloadEquals(buildPayload(conf, cur), p)) {
          setDraft((c) => (c === d || c === cur ? draftFromConfig(p) : c));
          break;
        }
        if (round >= 2 || validateDraft(cur)) break;
        d = cur;
      }
    } catch (e) {
      setSaveError(errMsg(e));
    } finally {
      setSaving(false);
      savingRef.current = false;
    }
  }, [cfg]);

  if (!draft) {
    return (
      <div className="flex items-center gap-2 py-4 text-sm text-base-content/40">
        <span className="loading loading-spinner loading-sm" />
        {t("settings.models.save")}
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-3">
      <ModelsSection draft={draft} onDraft={updateDraft} />
      {/* 保存条:与 SettingsView 同款(dirty → 显示,保存 → 收起) */}
      <div className="flex items-center gap-2">
        {dirty && (
          <button
            type="button"
            className="btn btn-primary btn-sm"
            disabled={saving}
            onClick={handleSave}
          >
            {saving ? <span className="loading loading-spinner loading-xs" /> : null}
            {t("settings.models.save")}
          </button>
        )}
        {saveError && <span className="text-xs text-error">{saveError}</span>}
      </div>
    </div>
  );
}
