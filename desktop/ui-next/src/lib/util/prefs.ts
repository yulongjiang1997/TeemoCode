// 本机 UI 偏好(mc.* 命名空间,键名与取值格式 = 旧 UI 契约)。
// 模块顶层不碰 localStorage,只用 getItem/setItem。

export type Space = "local" | "cloud" | "chat" | "stats";

/** 启动落点恒为本地任务(用户定案 2026-08-09:「应用打开后默认选本地项目,
 *  不要选云端项目」)。所以**没有 readSpace** ——上次停在哪儿不再决定这次开在
 *  哪儿:云端是可能未登录/断网的空间,拿它当开机首屏,每次启动都是一个坏屏幕。
 *  writeSpace 仍然写:mc.sidebarSpace 是与旧 UI 共用的契约键(ui/src/sidebar.tsx
 *  仍会读它),并行期不能单方面停写。 */
export function writeSpace(space: Space): void {
  try {
    localStorage.setItem("mc.sidebarSpace", space);
  } catch {
    // 只丢持久化
  }
}

export function readLastSession(): string | null {
  try {
    return localStorage.getItem("mc.lastSession");
  } catch {
    return null;
  }
}

export function writeLastSession(id: string): void {
  try {
    localStorage.setItem("mc.lastSession", id);
  } catch {
    // 只丢持久化
  }
}

export function readLastTaskModel(): string | null {
  try {
    return localStorage.getItem("mc.lastTaskModel");
  } catch {
    return null;
  }
}

export function rememberLastTaskModel(model: string): void {
  try {
    localStorage.setItem("mc.lastTaskModel", model);
  } catch {
    // 只丢持久化
  }
}

/** 侧栏折叠段开合态("1"/"0" 取值 = 旧 UI 契约):归档会话 / 归档项目 /
 * 云端历史 / 待办组内「已完成」小节(ui-next 新增,默认收起)。 */
export type FoldKey = "mc.archivedOpen" | "mc.projectArchiveOpen" | "mc.cloudHistoryOpen" | "mc.todoDoneOpen";

export function readFold(key: FoldKey): boolean {
  try {
    return localStorage.getItem(key) === "1";
  } catch {
    return false;
  }
}

export function writeFold(key: FoldKey, open: boolean): void {
  try {
    localStorage.setItem(key, open ? "1" : "0");
  } catch {
    // 只丢持久化
  }
}

/** 侧栏折叠态(会话列表收起成窄条;展开按钮在窄条上)。 */
export function readSidebarCollapsed(): boolean {
  try {
    return localStorage.getItem("mc.sidebarCollapsed") === "1";
  } catch {
    return false;
  }
}

export function writeSidebarCollapsed(c: boolean): void {
  try {
    localStorage.setItem("mc.sidebarCollapsed", c ? "1" : "0");
  } catch {
    // 只丢持久化
  }
}

/** 任务工作区(项目组)默认展开的任务数:超出部分折叠为「显示更多」。
 *  默认 2 = 只保留最后 2 个任务的对话内容直接可见,更早的收进折叠。
 *  设置页可改(1-20)。 */
export function readTaskExpandLimit(): number {
  try {
    const v = Number(localStorage.getItem("mc.taskExpandLimit"));
    return Number.isFinite(v) && v >= 1 ? Math.min(Math.round(v), 20) : 2;
  } catch {
    return 2;
  }
}

export function writeTaskExpandLimit(n: number): void {
  try {
    localStorage.setItem("mc.taskExpandLimit", String(Math.min(Math.max(Math.round(n), 1), 20)));
  } catch {
    // 只丢持久化
  }
}

/** 自定义背景图(data URL;空 = 未自定义,用默认纯色背景)。 */
export function readBgImage(): string {
  try {
    return localStorage.getItem("mc.bgImage") ?? "";
  } catch {
    return "";
  }
}

export function writeBgImage(dataUrl: string): void {
  try {
    if (dataUrl) localStorage.setItem("mc.bgImage", dataUrl);
    else localStorage.removeItem("mc.bgImage");
  } catch {
    // 只丢持久化
  }
}

/** 背景图透明度(1-100;100 = 完全显示图片,默认 60)。 */
export function readBgOpacity(): number {
  try {
    const v = Number(localStorage.getItem("mc.bgOpacity"));
    return Number.isFinite(v) ? Math.min(Math.max(Math.round(v), 1), 100) : 60;
  } catch {
    return 60;
  }
}

export function writeBgOpacity(pct: number): void {
  try {
    localStorage.setItem("mc.bgOpacity", String(Math.min(Math.max(Math.round(pct), 1), 100)));
  } catch {
    // 只丢持久化
  }
}

/** 遮罩层不透明度(0-100;工作区/侧栏/标题栏等所有区域的半透明底色)。
 *  默认 70 = 原硬编码值;调低让背景图更透,调高更实。 */
export function readMaskOpacity(): number {
  try {
    const v = Number(localStorage.getItem("mc.maskOpacity"));
    return Number.isFinite(v) ? Math.min(Math.max(Math.round(v), 0), 100) : 70;
  } catch {
    return 70;
  }
}

export function writeMaskOpacity(pct: number): void {
  try {
    localStorage.setItem("mc.maskOpacity", String(Math.min(Math.max(Math.round(pct), 0), 100)));
  } catch {
    // 只丢持久化
  }
}

/** 背景图模糊度(px;0-20,默认 4 = 原 backdrop-blur-xs 的磨砂感)。
 *  在根图层统一模糊一次,跨区域无缝(各区域各自 blur 会有接缝)。 */
export function readBgBlur(): number {
  try {
    const v = Number(localStorage.getItem("mc.bgBlur"));
    return Number.isFinite(v) ? Math.min(Math.max(v, 0), 20) : 4;
  } catch {
    return 4;
  }
}

export function writeBgBlur(px: number): void {
  try {
    localStorage.setItem("mc.bgBlur", String(Math.min(Math.max(px, 0), 20)));
  } catch {
    // 只丢持久化
  }
}

/** 团队角色(统一模型,按技能分派):策划/开发/测试等。 */
export interface TeamRole {
  id: string;
  name: string;
  /** 角色职责描述(手动填写,如:需求分析、方案设计) */
  skill: string;
  /** 指定技能(技能库里的技能名,可多选;执行时优先使用) */
  skills: string[];
}

export function readTeamRoles(): TeamRole[] {
  try {
    const v = JSON.parse(localStorage.getItem("mc.teamRoles") ?? "[]");
    return Array.isArray(v)
      ? v
          .filter((r): r is TeamRole => Boolean(r && typeof r.name === "string"))
          .map((r) => ({
            id: r.id,
            name: r.name,
            skill: typeof r.skill === "string" ? r.skill : "",
            skills: Array.isArray(r.skills) ? r.skills.filter((x): x is string => typeof x === "string") : [],
          }))
      : [];
  } catch {
    return [];
  }
}

export function writeTeamRoles(roles: TeamRole[]): void {
  try {
    localStorage.setItem("mc.teamRoles", JSON.stringify(roles));
  } catch {
    // 只丢持久化
  }
}

/** 会话的团队模式开关(发送任务时注入团队编排指令)。 */
export function readTeamMode(sid: string): boolean {
  try {
    return localStorage.getItem(`mc.teamMode.${sid}`) === "1";
  } catch {
    return false;
  }
}

export function writeTeamMode(sid: string, on: boolean): void {
  try {
    localStorage.setItem(`mc.teamMode.${sid}`, on ? "1" : "0");
  } catch {
    // 只丢持久化
  }
}

/** 已从本地删除的同步模型名(下次同步跳过,不重新拉回)。 */
export function readSyncedExcluded(): string[] {
  try {
    const v = JSON.parse(localStorage.getItem("mc.syncedExcluded") ?? "[]");
    return Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : [];
  } catch {
    return [];
  }
}

export function writeSyncedExcluded(names: string[]): void {
  try {
    localStorage.setItem("mc.syncedExcluded", JSON.stringify([...new Set(names)]));
  } catch {
    // 只丢持久化
  }
}

/** 事件音效配置:每事件 { enabled, file? }。
 *  自定义文件经壳复制到应用数据目录(sounds/),这里存存储路径,播放走 asset 协议。 */
export interface SoundEntry {
  enabled: boolean;
  file?: string;
}
export type SoundConfig = Partial<Record<"startup" | "task-done" | "task-error" | "ask" | "idle", SoundEntry>>;

export function readSoundConfig(): SoundConfig {
  try {
    const v = JSON.parse(localStorage.getItem("mc.sounds") ?? "{}");
    return v && typeof v === "object" ? v : {};
  } catch {
    return {};
  }
}

export function writeSoundConfig(cfg: SoundConfig): void {
  try {
    localStorage.setItem("mc.sounds", JSON.stringify(cfg));
  } catch {
    // 只丢持久化
  }
}

/** 指令队列项(本地持久化 + 内存队列共用):待发送 / 执行中 / 失败。 */
export type QueueItemState = "pending" | "executing" | "failed";
export interface QueueItem {
  id: string;
  text: string;
  /** 随指令入队的附件(后台补投 deliverQueued 也按它拼装正文)。 */
  atts: ComposerAtt[];
  state: QueueItemState;
}
export type ComposerAtt = {
  path: string;
  name: string;
  isImage: boolean;
};
/** 指令队列持久化(按会话)。重启后恢复 pending/executing/failed 项与暂停态。 */
export interface ComposerPersisted {
  queue: QueueItem[];
  paused: boolean;
  draft?: string;
  atts?: ComposerAtt[];
}
export const COMPOSER_QUEUE_KEY_PREFIX = "mc.queue.";

export function readComposerQueue(sid: string): ComposerPersisted | null {
  try {
    const raw = localStorage.getItem(COMPOSER_QUEUE_KEY_PREFIX + sid);
    if (!raw) return null;
    const v = JSON.parse(raw);
    if (!v || typeof v !== "object" || !Array.isArray(v.queue)) return null;
    // 旧格式(text/atts/state=pending|failed)向前兼容:跨重启不会真在"执行中",
    // executing 一律归 pending;非预期 state 同样归 pending;atts 缺失归空数组
    const queue: QueueItem[] = (v.queue as unknown[])
      .filter((x): x is { id: unknown; text: unknown; atts?: unknown; state?: unknown } => {
        if (!x || typeof x !== "object") return false;
        const o = x as Record<string, unknown>;
        return typeof o.id === "string" && typeof o.text === "string";
      })
      .map((x) => {
        const o = x as Record<string, unknown>;
        const st = o.state;
        const state: QueueItemState = st === "executing" || st === "failed" ? st : "pending";
        const atts = Array.isArray(o.atts)
          ? (o.atts as ComposerAtt[]).filter(
              (a): a is ComposerAtt => Boolean(a && typeof a === "object" && typeof (a as ComposerAtt).path === "string"),
            )
          : [];
        return { id: o.id as string, text: o.text as string, atts, state };
      });
    return {
      queue,
      paused: Boolean(v.paused),
      draft: typeof v.draft === "string" ? v.draft : undefined,
      atts: Array.isArray(v.atts) ? (v.atts as ComposerAtt[]) : undefined,
    };
  } catch {
    return null;
  }
}

export function writeComposerQueue(sid: string, val: ComposerPersisted | null): void {
  try {
    if (val && (val.queue.length > 0 || val.paused || val.draft)) {
      localStorage.setItem(COMPOSER_QUEUE_KEY_PREFIX + sid, JSON.stringify(val));
    } else {
      localStorage.removeItem(COMPOSER_QUEUE_KEY_PREFIX + sid);
    }
  } catch {
    // 只丢持久化
  }
}
