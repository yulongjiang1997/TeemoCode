// 任务工作区「不死不休」:输入框下那排菜单里打开后,该对话里没做完的任务只要落到
// error / interrupted,就自动补一条「继续」直到做完。点停止是刹车,
// 不把用户主动取消立刻再开起来。
// 2026-11-02:改为以**会话(session id)**为单位隔离——同工作区的不同对话
// 互不影响(旧版按 workdir 全局开关,会跨对话误补发)。
import { b64encode } from "@/lib/protocol/codec";
import { sessionOpen, sessionSend, type SessionMeta } from "@/lib/ipc/sessions";
import { readArchivedProjects } from "@/lib/util/projects";

const STORAGE_KEY = "mc.relentlessSessions";
const FIRST_DELAY_MS = 2_000;
const MAX_DELAY_MS = 60_000;

export const RELENTLESS_STATUSES = new Set(["error", "interrupted"]);

type Clock = () => number;

let keys = loadKeys();
const listeners = new Set<() => void>();
const userStopped = new Set<string>();
const inflight = new Set<string>();
const nextAt = new Map<string, number>();
const delayMs = new Map<string, number>();

function loadKeys(): Set<string> {
  try {
    const raw: unknown = JSON.parse(localStorage.getItem(STORAGE_KEY) || "[]");
    if (!Array.isArray(raw)) return new Set();
    // 元素为会话 id(2026-11-02 起按对话隔离;不再存 projectKey)
    return new Set(raw.filter((item): item is string => typeof item === "string" && item.length > 0));
  } catch {
    return new Set();
  }
}

function persist() {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify([...keys]));
  } catch {
    // 只丢持久化,内存勾选仍有效
  }
  for (const listener of listeners) listener();
}

export function getRelentlessKeys(): ReadonlySet<string> {
  return keys;
}

export function isRelentlessSession(sessionId: string): boolean {
  return keys.has(sessionId);
}

export function setRelentlessSession(sessionId: string, on: boolean): void {
  const next = new Set(keys);
  if (on) next.add(sessionId);
  else next.delete(sessionId);
  if (next.size === keys.size && [...next].every((item) => keys.has(item))) return;
  keys = next;
  persist();
}

/** 旧 API 向前兼容(部分外部调用仍传 workdir)。映射到等价空操作:
 *  workdir 级全局开关已废弃,改用 setRelentlessSession(sessionId, on)。 */
export function isRelentlessWorkspace(_workdir: string): boolean {
  return false;
}

export function setRelentlessWorkspace(_workdir: string, _on: boolean): void {
  // no-op: 旧 workdir 级开关已废弃(2026-11-02 改为会话级隔离)
}

export function subscribeRelentless(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** 用户点了停止。这次中断不再自动重开,直到用户自己再发一条。 */
export function markUserStop(id: string): void {
  userStopped.add(id);
  nextAt.delete(id);
}

export function clearUserStop(id: string): void {
  userStopped.delete(id);
  // 用户刚自己发过。给状态切到 running 留窗口,避免立刻再补一条「继续」。
  nextAt.set(id, Date.now() + 8_000);
  delayMs.delete(id);
}

export function isUserStopped(id: string): boolean {
  return userStopped.has(id);
}

export function relentlessCandidates(
  sessions: readonly (Pick<SessionMeta, "id" | "workdir" | "status" | "archived" | "waiting_ask"> & { kind?: string })[],
  enabled: ReadonlySet<string> = keys,
  paused: ReadonlySet<string> = userStopped,
  archivedProjects: ReadonlySet<string> = readArchivedProjects(),
): string[] {
  const out: string[] = [];
  for (const session of sessions) {
    if (session.archived || session.waiting_ask || session.kind === "chat" || paused.has(session.id)) continue;
    if (!RELENTLESS_STATUSES.has(session.status)) continue;
    // 2026-11-02:按会话 id 隔离——只有该会话被用户勾选"不死不休"才补发
    if (!enabled.has(session.id)) continue;
    void archivedProjects;
    out.push(session.id);
  }
  return out;
}

/** 失败态第一次看见先等 2s,避免状态闪一下就补发。健康态清掉退避。 */
export function relentlessDue(id: string, status: string, now: number): boolean {
  if (!RELENTLESS_STATUSES.has(status)) {
    delayMs.delete(id);
    nextAt.delete(id);
    return false;
  }
  const at = nextAt.get(id);
  if (at === undefined) {
    nextAt.set(id, now + FIRST_DELAY_MS);
    return false;
  }
  return at <= now;
}

export async function kickRelentless(id: string, text: string, now: Clock = Date.now): Promise<void> {
  if (!text.trim() || inflight.has(id) || userStopped.has(id)) return;
  const stamp = now();
  if ((nextAt.get(id) ?? 0) > stamp) return;
  inflight.add(id);
  nextAt.set(id, stamp + FIRST_DELAY_MS);
  try {
    try {
      await sessionSend(id, "user-input", { content: b64encode(text) });
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      if (!message.includes("会话未打开")) throw err;
      await sessionOpen(id, 1);
      await sessionSend(id, "user-input", { content: b64encode(text) });
    }
    delayMs.delete(id);
    nextAt.set(id, stamp + 8_000);
  } catch {
    const wait = Math.min(MAX_DELAY_MS, (delayMs.get(id) ?? FIRST_DELAY_MS) * 2);
    delayMs.set(id, wait);
    nextAt.set(id, stamp + wait);
  } finally {
    inflight.delete(id);
  }
}

export function resetRelentlessForTests(): void {
  keys = new Set();
  userStopped.clear();
  inflight.clear();
  nextAt.clear();
  delayMs.clear();
  try {
    localStorage.removeItem(STORAGE_KEY);
  } catch {
    // 测试环境可能没有 storage
  }
}
