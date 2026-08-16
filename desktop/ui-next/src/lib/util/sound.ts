import type { MessageKey } from "@/lib/i18n";
import { readSoundConfig } from "@/lib/util/prefs";
import { soundAssetUrl } from "@/lib/ipc/config";

/** 事件音效:每事件单独开关 + 可换自定义文件(文件存 IndexedDB,配置只存开关)。
 *  实际播放发生在桌宠窗口(pet.html),主窗口只负责设置与试听。
 *  默认音 = 内置 mp3(public 资源,与桌宠同一套)。 */

export type SoundEvent = "startup" | "task-done" | "task-error" | "ask" | "idle";

/** 事件 → 内置默认音效资源(public 根路径)。 */
export const SOUND_DEFAULTS: Record<SoundEvent, string> = {
  startup: "sound-app-start.mp3",
  "task-done": "sound-task-end.mp3",
  "task-error": "sound-task-error.mp3",
  ask: "sound-permission.mp3",
  idle: "sound-idle.mp3",
};

export const SOUND_EVENTS: { id: SoundEvent; labelKey: MessageKey }[] = [
  { id: "startup", labelKey: "settings.sound.startup" },
  { id: "task-done", labelKey: "settings.sound.taskDone" },
  { id: "task-error", labelKey: "settings.sound.taskError" },
  { id: "ask", labelKey: "settings.sound.ask" },
  { id: "idle", labelKey: "settings.sound.idle" },
];

let globalEnabled = true;
export function setGlobalSoundEnabled(on: boolean): void {
  globalEnabled = on;
}
export function isGlobalSoundEnabled(): boolean {
  return globalEnabled;
}

function playSrc(src: string): void {
  try {
    const a = new Audio(src);
    void a.play().catch(() => {});
  } catch {
    // 静默:音频不可用不报错
  }
}

/** 试听/主窗口播放:事件没被显式关闭 → 播放(自定义文件优先,否则内置默认 mp3)。 */
export function playEventSound(ev: SoundEvent): void {
  if (!globalEnabled) return;
  const cfg = readSoundConfig();
  const entry = cfg[ev];
  if (entry && !entry.enabled) return; // 未配置 = 默认开;显式关才静音
  if (entry?.file) {
    playSrc(soundAssetUrl(entry.file));
    return;
  }
  playSrc(SOUND_DEFAULTS[ev]);
}
