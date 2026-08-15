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
  skill: string;
}

export function readTeamRoles(): TeamRole[] {
  try {
    const v = JSON.parse(localStorage.getItem("mc.teamRoles") ?? "[]");
    return Array.isArray(v)
      ? v.filter((r): r is TeamRole => Boolean(r && typeof r.name === "string" && typeof r.skill === "string"))
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
