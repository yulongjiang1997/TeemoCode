// 每会话 composer 暂存与后台补投(旧 useSession stash/deliverQueued 的移植):
// - 切会话时草稿/指令队列/附件按 sid 留档,切回恢复(仅内存,重启即丢;上传中
//   列表不入档——进度是瞬态,在途收尾回调按 id 过滤,清空无害);
// - 后台会话轮结束(session-status 非 running/created)自动补投其暂存队列的
//   队头指令——壳在 session_close 后仍按 id 持有会话,免连接可直投;
// - 乐观出栈、失败回栈;恰好又开跑/多客户端抢先由壳的忙碌守卫兜底拒掉
//   (壳契约 Err ⟺ 消息未入会话,回栈安全);
// - 补投失败时用户恰好切了进来:经 bindActiveComposer 的通道回到活动队列槽。
import { sessionSend } from "@/lib/ipc/sessions";
import { b64encode } from "@/lib/protocol/codec";
import type { ComposerAtt, QueuedInstr } from "./useComposer";

export interface StashEntry {
  draft: string;
  queue: QueuedInstr[];
  atts: ComposerAtt[];
  /** 队列暂停态(按会话保留) */
  paused?: boolean;
}

const stash = new Map<string, StashEntry>();

// 活跃 composer 的登记:deliverQueued 靠它跳过当前会话(其排队由 useComposer
// 自己的轮末 flush 负责),失败回投也靠它找回活动队列槽
let activeId: string | null = null;
let requeueActive: ((text: string) => boolean) | null = null;

export function stashGet(id: string): StashEntry | undefined {
  return stash.get(id);
}

/** 空档不占条目(与旧实现同口径:全空即清)。 */
export function stashSet(id: string, entry: StashEntry): void {
  if (entry.draft || entry.queue.length || entry.atts.length || entry.paused) stash.set(id, entry);
  else stash.delete(id);
}

/** 删除会话随之清档。 */
export function dropStash(id: string): void {
  stash.delete(id);
}

/** useComposer 挂载/切会话时登记;返回注销函数(React cleanup)。
 * requeue:补投失败且人已在现场时把消息放回活动队列槽,返回是否接住。 */
export function bindActiveComposer(id: string, requeue: (text: string) => boolean): () => void {
  activeId = id;
  requeueActive = requeue;
  return () => {
    if (activeId === id) {
      activeId = null;
      requeueActive = null;
    }
  };
}

/** 后台会话状态变更(App 的 session-event 接线):轮结束即补投暂存队列的
 * 队头指令;成功回调 onDelivered(出 toast + 侧栏 attention)。失败回栈
 * (队列头仍是该条,status 变化会再次触发补投——与旧单槽行为一致)。 */
export function deliverQueued(id: string, status: string, onDelivered?: (id: string, text: string) => void): void {
  if (status === "running" || status === "created") return; // 轮未结束
  if (id === activeId) return; // 现场会话走 useComposer 自己的 flush
  const entry = stash.get(id);
  const head = entry?.queue[0];
  if (!head) return;
  const payload = [head.text, ...head.atts.map((a) => a.path).filter(Boolean)].join("\n");
  stashSet(id, { ...entry, queue: entry.queue.slice(1) }); // 乐观出队(draft/atts/暂停态留档)
  void sessionSend(id, "user-input", { content: b64encode(payload) }).then(
    () => onDelivered?.(id, head.text),
    () => {
      // 失败回栈:补投期间用户切了进来 → 回活动队列槽;否则回暂存队头
      if (id === activeId && requeueActive?.(head.text)) return;
      const prev = stash.get(id);
      if (!prev?.queue.some((x) => x.text === head.text)) {
        stash.set(id, { draft: prev?.draft ?? "", queue: [head, ...(prev?.queue ?? [])], atts: prev?.atts ?? [] });
      }
    },
  );
}

/** 仅供测试:清空模块级状态(stash 与活跃登记)。 */
export function resetStashForTests(): void {
  stash.clear();
  activeId = null;
  requeueActive = null;
}
