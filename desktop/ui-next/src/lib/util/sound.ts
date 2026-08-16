import type { MessageKey } from "@/lib/i18n";
import { readSoundConfig } from "@/lib/util/prefs";

/** 事件音效:每事件单独开关 + 可换自定义文件(data URL)。
 *  全局总开关沿用壳的 sound_enabled(设置-音效页 + 托盘同步)。 */

export type SoundEvent = "startup" | "task-done" | "task-error" | "ask";

export const SOUND_EVENTS: { id: SoundEvent; labelKey: MessageKey }[] = [
  { id: "startup", labelKey: "settings.sound.startup" },
  { id: "task-done", labelKey: "settings.sound.taskDone" },
  { id: "task-error", labelKey: "settings.sound.taskError" },
  { id: "ask", labelKey: "settings.sound.ask" },
];

let globalEnabled = true;
export function setGlobalSoundEnabled(on: boolean): void {
  globalEnabled = on;
}
export function isGlobalSoundEnabled(): boolean {
  return globalEnabled;
}

/** 默认提示音:WebAudio 生成的双音(无内置文件,无需额外资源)。 */
function playBeep(high: boolean): void {
  try {
    const Ctx = window.AudioContext || (window as unknown as { webkitAudioContext: typeof AudioContext }).webkitAudioContext;
    const ctx = new Ctx();
    const osc = ctx.createOscillator();
    const gain = ctx.createGain();
    osc.type = "sine";
    osc.frequency.value = high ? 880 : 440;
    gain.gain.setValueAtTime(0.0001, ctx.currentTime);
    gain.gain.exponentialRampToValueAtTime(0.25, ctx.currentTime + 0.02);
    gain.gain.exponentialRampToValueAtTime(0.0001, ctx.currentTime + (high ? 0.18 : 0.28));
    osc.connect(gain);
    gain.connect(ctx.destination);
    osc.start();
    osc.stop(ctx.currentTime + (high ? 0.2 : 0.3));
    void osc.onended;
  } catch {
    // 静默:无音频环境不报错
  }
}

// 自动播放限制:启动音效在无用户手势时可能被浏览器策略拦截,
// play() 被拒后延迟到首个用户手势(pointerdown/keydown)再补播。
let startupPending = false;
function retryStartup(): void {
  if (!startupPending) return;
  startupPending = false;
  window.removeEventListener("pointerdown", retryStartup);
  window.removeEventListener("keydown", retryStartup);
  playEventSound("startup");
}

/** 播放事件音效:全局开 && 事件没被显式关闭 → 播放(自定义文件优先,否则默认音)。 */
export function playEventSound(ev: SoundEvent): void {
  if (!globalEnabled) return;
  const cfg = readSoundConfig();
  const entry = cfg[ev];
  if (entry && !entry.enabled) return; // 未配置 = 默认开;显式关才静音
  if (entry?.file) {
    try {
      const a = new Audio(entry.file);
      const p = a.play();
      if (p) {
        p.catch(() => {
          if (ev === "startup") {
            startupPending = true;
            window.addEventListener("pointerdown", retryStartup);
            window.addEventListener("keydown", retryStartup);
          } else {
            playBeep(ev === "task-error");
          }
        });
      }
      return;
    } catch {
      // 文件播放失败,退回默认
    }
  }
  playBeep(ev === "task-error");
}
