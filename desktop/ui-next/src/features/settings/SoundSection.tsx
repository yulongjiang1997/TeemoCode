import { IconRotate } from "@tabler/icons-react";
import { useEffect, useRef, useState } from "react";

import { useI18n } from "@/lib/i18n";
import { getSoundEnabled, onSoundEnabled, setSoundEnabled } from "@/lib/ipc/config";
import { inDesktopShell } from "@/lib/ipc/ipc";
import { setGlobalSoundEnabled, playEventSound, SOUND_EVENTS, type SoundEvent } from "@/lib/util/sound";
import { saveSoundFile, deleteSoundFile } from "@/lib/util/soundFile";
import { readSoundConfig, writeSoundConfig, type SoundConfig } from "@/lib/util/prefs";

/** 音效:全局开关(壳 sound_enabled) + 每事件单独开关 + 可替换音效文件。 */
export function SoundSection() {
  const { t } = useI18n();
  const [masterOn, setMasterOn] = useState(true);
  const [cfg, setCfg] = useState<SoundConfig>(readSoundConfig);
  const fileRefs = useRef<Record<string, HTMLInputElement | null>>({});

  useEffect(() => {
    if (!inDesktopShell()) return;
    let alive = true;
    void getSoundEnabled().then((on) => {
      if (alive) {
        setMasterOn(on);
        setGlobalSoundEnabled(on);
      }
    });
    const off = onSoundEnabled((on) => {
      setMasterOn(on);
      setGlobalSoundEnabled(on);
    });
    return () => {
      alive = false;
      off();
    };
  }, []);

  const commit = (next: SoundConfig) => {
    setCfg(next);
    writeSoundConfig(next);
  };

  const toggleMaster = (on: boolean) => {
    setMasterOn(on);
    setGlobalSoundEnabled(on);
    if (inDesktopShell()) void setSoundEnabled(on);
  };

  const pickFile = async (ev: SoundEvent, file: File | null) => {
    if (!file) return;
    // 音频文件存 IndexedDB(localStorage 放 base64 有配额限制),配置只记标志
    try {
      const name = await saveSoundFile(ev, file);
      commit({ ...cfg, [ev]: { ...(cfg[ev] ?? { enabled: true }), hasFile: true, name } });
    } catch {
      // 保存失败:不改配置
    }
  };

  const clearFile = (ev: SoundEvent) => {
    void deleteSoundFile(ev);
    const { hasFile: _hasFile, name: _name, ...rest } = cfg[ev] ?? { enabled: true };
    void _hasFile;
    void _name;
    commit({ ...cfg, [ev]: rest });
  };

  return (
    <section aria-label={t("settings.nav.sound")} className="flex flex-col gap-2">
      <div className="rounded-box border border-base-300">
        <div className="flex flex-col gap-1 border-b border-base-300/70 p-3">
          <div className="text-sm font-semibold">{t("settings.sound.title")}</div>
          <p className="text-xs text-base-content/60">{t("settings.sound.hint")}</p>
        </div>
        {/* 全局总开关(与托盘同步) */}
        <div className="flex items-center gap-2 border-b border-base-300/70 px-3 py-2">
          <span className="min-w-0 flex-1 text-xs">{t("settings.sound.master")}</span>
          <input
            type="checkbox"
            className="toggle toggle-sm shrink-0"
            aria-label={t("settings.sound.master")}
            checked={masterOn}
            onChange={(e) => toggleMaster(e.target.checked)}
          />
        </div>
        <ul className="flex flex-col gap-2 p-3">
          {SOUND_EVENTS.map(({ id, labelKey }) => {
            const entry = cfg[id] ?? { enabled: true };
            return (
              <li key={id} className="flex items-center gap-2 rounded-box border border-base-300/70 bg-base-200/40 px-3 py-2">
                <input
                  type="checkbox"
                  className="checkbox checkbox-xs shrink-0"
                  aria-label={t(labelKey)}
                  checked={entry.enabled}
                  onChange={(e) => commit({ ...cfg, [id]: { ...entry, enabled: e.target.checked } })}
                />
                <span className="min-w-0 flex-1 text-xs">{t(labelKey)}</span>
                <button
                  type="button"
                  className="btn btn-ghost btn-xs shrink-0 text-base-content/60"
                  title={t("settings.sound.previewTip")}
                  onClick={() => void playEventSound(id)}
                >
                  {t("settings.sound.preview")}
                </button>
                <button
                  type="button"
                  className="btn btn-ghost btn-xs shrink-0 text-base-content/50"
                  onClick={() => fileRefs.current[id]?.click()}
                >
                  {entry.hasFile ? t("settings.sound.replace") : t("settings.sound.pick")}
                </button>
                <input
                  ref={(el) => {
                    fileRefs.current[id] = el;
                  }}
                  type="file"
                  accept="audio/*,.wav,.mp3,.ogg"
                  className="hidden"
                  aria-label={`${t(labelKey)} ${t("settings.sound.pick")}`}
                  onChange={(e) => pickFile(id, e.target.files?.[0] ?? null)}
                />
                {entry.hasFile && (
                  <button
                    type="button"
                    className="btn btn-ghost btn-square btn-xs shrink-0 text-base-content/40"
                    title={t("settings.sound.reset")}
                    onClick={() => clearFile(id)}
                  >
                    <IconRotate size={12} stroke={1.75} aria-hidden />
                  </button>
                )}
              </li>
            );
          })}
        </ul>
      </div>
    </section>
  );
}
