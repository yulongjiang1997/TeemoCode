import { IconRotate } from "@tabler/icons-react";
import { useEffect, useRef, useState } from "react";

import { useI18n } from "@/lib/i18n";
import { getSoundEnabled, onSoundEnabled, setSoundEnabled } from "@/lib/ipc/config";
import { inDesktopShell } from "@/lib/ipc/ipc";
import { setGlobalSoundEnabled, SOUND_EVENTS, type SoundEvent } from "@/lib/util/sound";
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

  const pickFile = (ev: SoundEvent, file: File | null) => {
    if (!file) return;
    const reader = new FileReader();
    reader.onload = () => {
      const dataUrl = String(reader.result ?? "");
      commit({ ...cfg, [ev]: { ...(cfg[ev] ?? { enabled: true }), file: dataUrl } });
    };
    reader.readAsDataURL(file);
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
                  className="btn btn-ghost btn-xs shrink-0 text-base-content/50"
                  onClick={() => fileRefs.current[id]?.click()}
                >
                  {entry.file ? t("settings.sound.replace") : t("settings.sound.pick")}
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
                {entry.file && (
                  <button
                    type="button"
                    className="btn btn-ghost btn-square btn-xs shrink-0 text-base-content/40"
                    title={t("settings.sound.reset")}
                    onClick={() => {
                      const { file: _file, ...rest } = entry;
                      void _file;
                      commit({ ...cfg, [id]: rest });
                    }}
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
