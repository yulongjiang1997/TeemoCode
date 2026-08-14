// 每会话 composer 暂存与后台补投的测试。
import { beforeEach, describe, expect, it, vi } from "vitest";

import { b64encode } from "@/lib/protocol/codec";
import type { QueuedInstr } from "./useComposer";
import { bindActiveComposer, deliverQueued, dropStash, resetStashForTests, stashGet, stashSet } from "./stash";

function stubSend(impl: () => Promise<unknown> | unknown) {
  const calls: unknown[] = [];
  (window as unknown as { __TAURI__?: unknown }).__TAURI__ = {
    core: { invoke: (cmd: string, args?: unknown) => (calls.push({ cmd, args }), Promise.resolve().then(impl)) },
  };
  return calls;
}

const qi = (text: string, state: QueuedInstr["state"] = "pending"): QueuedInstr => ({
  id: `q-${text}`,
  text,
  atts: [],
  state,
});

const flush = () => new Promise((r) => setTimeout(r, 0));

beforeEach(() => {
  resetStashForTests();
  delete (window as unknown as { __TAURI__?: unknown }).__TAURI__;
});

describe("stash 留档", () => {
  it("全空即清条目;dropStash 清档", () => {
    stashSet("a", { draft: "x", queue: [], atts: [] });
    expect(stashGet("a")?.draft).toBe("x");
    stashSet("a", { draft: "", queue: [], atts: [] });
    expect(stashGet("a")).toBeUndefined();
    stashSet("b", { draft: "", queue: [qi("q")], atts: [] });
    dropStash("b");
    expect(stashGet("b")).toBeUndefined();
  });
});

describe("deliverQueued 后台补投", () => {
  it("轮未结束(running/created)不投;现场会话不投", () => {
    const calls = stubSend(() => null);
    stashSet("a", { draft: "", queue: [qi("排着的")], atts: [] });
    deliverQueued("a", "running");
    deliverQueued("a", "created");
    const off = bindActiveComposer("a", () => false);
    deliverQueued("a", "idle");
    off();
    expect(calls).toHaveLength(0);
    expect(stashGet("a")?.queue[0]?.text).toBe("排着的");
  });

  it("轮结束:乐观出队投队头,成功回调;draft/atts 留档不动", async () => {
    const calls = stubSend(() => null);
    stashSet("a", { draft: "草稿", queue: [qi("排着的")], atts: [{ path: "p.png", name: "p.png", isImage: true }] });
    const delivered = vi.fn();
    deliverQueued("a", "idle", delivered);
    expect(stashGet("a")?.queue[0]?.text).toBeUndefined(); // 乐观出队
    await flush();
    expect(calls[0]).toEqual({ cmd: "session_send", args: { id: "a", ftype: "user-input", payload: { content: b64encode("排着的") } } });
    expect(delivered).toHaveBeenCalledWith("a", "排着的");
    expect(stashGet("a")?.draft).toBe("草稿");
    expect(stashGet("a")?.atts).toHaveLength(1);
  });

  it("投递失败回栈(队头仍是该条);队尾后续指令不受影响", async () => {
    stubSend(() => {
      throw new Error("busy");
    });
    stashSet("a", { draft: "", queue: [qi("旧消息"), qi("后续")], atts: [] });
    deliverQueued("a", "idle");
    await flush();
    expect(stashGet("a")?.queue.map((x) => x.text)).toEqual(["旧消息", "后续"]); // 失败回栈,顺序保持
  });

  it("投递失败且人已切进来:回活动队列槽,不回暂存", async () => {
    stubSend(() => {
      throw new Error("busy");
    });
    stashSet("a", { draft: "", queue: [qi("排着的")], atts: [] });
    deliverQueued("a", "idle");
    const requeue = vi.fn().mockReturnValue(true);
    bindActiveComposer("a", requeue); // 补投在途中切进来
    await flush();
    expect(requeue).toHaveBeenCalledWith("排着的");
    expect(stashGet("a")).toBeUndefined();
  });
});
