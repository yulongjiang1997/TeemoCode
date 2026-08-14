// useComposer 数据面:指令队列(追加/顺序补投/失败标红+暂停/重试/移除)。
// 并发语义:running 为真=壳在跑轮,直发必被拒,只能进队列;running 假但
// 无帧水位(lastSeq 不动)=上行在途,同样只进队列;帧水位一到才代表壳已
// 把上行物化成帧,可以继续补投。
import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { useComposer, type ComposerFeed, type QueuedInstr } from "./useComposer";
import { resetStashForTests, stashGet, stashSet } from "./stash";

/** 数据面默认信号:历史已落地、无新帧;各用例只覆写关心的那一项。 */
const feed = (over: Partial<ComposerFeed> = {}): ComposerFeed => ({
  running: false,
  historyLoaded: true,
  lastSeq: 0,
  turnEnded: false,
  ...over,
});

const qi = (text: string, state: QueuedInstr["state"] = "pending"): QueuedInstr => ({
  id: `q-${text}`,
  text,
  atts: [],
  state,
});

/** 壳侧 stub:回执可编程(默认立即成功);录制所有 session_send。 */
function stubSend(impl?: (cmd: string) => unknown) {
  const calls: { cmd: string; args: { id: string; payload: { content: string } } }[] = [];
  (window as unknown as { __TAURI__?: unknown }).__TAURI__ = {
    core: {
      invoke: (cmd: string, args?: { id: string; payload: { content: string } }) => {
        if (cmd !== "session_send") return Promise.resolve(null);
        calls.push({ cmd, args: args! });
        return Promise.resolve().then(() => {
          const r = impl?.(cmd);
          if (r === undefined) return null;
          throw r;
        });
      },
    },
  };
  const sends = () => calls.filter((c) => c.cmd === "session_send");
  return { calls, sends };
}

/** 一轮完整周期:投出后引擎开轮(running true)、回合结束(running false)。 */
function turnCycle(rerender: (p: { running: boolean; turnEnded: boolean }) => void, ended = true) {
  rerender({ running: true, turnEnded: false });
  rerender({ running: false, turnEnded: ended });
}

const settle = () => act(async () => {
  await Promise.resolve();
});

const textOf = (c: { payload: { content: string } }) => decodeURIComponent(escape(atob(c.payload.content)));

beforeEach(() => {
  resetStashForTests();
  vi.useRealTimers();
});

afterEach(() => {
  delete (window as unknown as { __TAURI__?: unknown }).__TAURI__;
});

describe("useComposer:留档与恢复", () => {
  it("切会话留档草稿与队列,切回恢复;新会话是干净的", async () => {
    stubSend();
    const { result, rerender } = renderHook(({ id, running }) => useComposer(id, feed({ running })), {
      initialProps: { id: "a", running: true },
    });
    // send 的闭包按渲染帧取 draft:setDraft 与 send 必须分属两个 act
    act(() => result.current.setDraft("排我"));
    act(() => {
      result.current.send();
    });
    act(() => result.current.setDraft("A 的草稿"));
    expect(result.current.queue[0]?.text).toBe("排我");

    rerender({ id: "b", running: false });
    expect(result.current.draft).toBe("");
    expect(result.current.queue).toHaveLength(0);

    rerender({ id: "a", running: true });
    expect(result.current.draft).toBe("A 的草稿");
    expect(result.current.queue[0]?.text).toBe("排我");
  });

  it("运行中发送无限追加队尾(按顺序执行)", () => {
    stubSend();
    const { result } = renderHook(() => useComposer("a", feed({ running: true })));
    for (const txt of ["第一条", "第二条", "第三条"]) {
      act(() => result.current.setDraft(txt));
      act(() => {
        result.current.send();
      });
    }
    expect(result.current.queue.map((x) => x.text)).toEqual(["第一条", "第二条", "第三条"]);
    expect(result.current.queue.every((x) => x.state === "pending")).toBe(true);
  });
});

describe("useComposer:队列补投", () => {
  it("轮结束自动补投队头;成功出队后继续投下一条", async () => {
    const { calls, sends } = stubSend();
    const { result, rerender } = renderHook(
      ({ running, turnEnded }) => useComposer("a", feed({ running, turnEnded })),
      { initialProps: { running: true, turnEnded: false } },
    );
    act(() => result.current.setDraft("排队中"));
    act(() => {
      result.current.send();
    });
    rerender({ running: false, turnEnded: true }); // 轮结束 → 投出队头
    await waitFor(() => expect(sends().length).toBe(1));
    expect(textOf(calls[0]!.args)).toBe("排队中");
    turnCycle(rerender, true); // 新轮跑完(task-ended)→ 成功出队
    await waitFor(() => expect(result.current.queue).toHaveLength(0));
  });

  it("task-error 失败结束:当前指令标 failed + 自动暂停,不再自动补投", async () => {
    const { sends } = stubSend();
    const { result, rerender } = renderHook(
      ({ running, turnEnded }) => useComposer("a", feed({ running, turnEnded })),
      { initialProps: { running: true, turnEnded: false } },
    );
    act(() => result.current.setDraft("会失败的"));
    act(() => {
      result.current.send();
    });
    rerender({ running: false, turnEnded: true }); // 投出
    await waitFor(() => expect(sends().length).toBe(1));
    turnCycle(rerender, false); // 新轮以 task-error 结束(turnEnded 未置)
    await waitFor(() => expect(result.current.queue[0]?.state).toBe("failed"));
    expect(result.current.paused).toBe(true); // 自动暂停
  });

  it("重试:failed 项复位 pending + 解除暂停,立即补投", async () => {
    const { sends } = stubSend();
    const { result, rerender } = renderHook(
      ({ running, turnEnded }) => useComposer("a", feed({ running, turnEnded })),
      { initialProps: { running: true, turnEnded: false } },
    );
    act(() => result.current.setDraft("要重试的"));
    act(() => {
      result.current.send();
    });
    rerender({ running: false, turnEnded: true });
    await waitFor(() => expect(sends().length).toBe(1));
    turnCycle(rerender, false); // 失败
    await waitFor(() => expect(result.current.queue[0]?.state).toBe("failed"));

    act(() => result.current.retryInstr(result.current.queue[0]!.id));
    await waitFor(() => expect(sends().length).toBe(2)); // 立即重投
    turnCycle(rerender, true); // 重试成功
    await waitFor(() => expect(result.current.queue).toHaveLength(0));
    expect(result.current.paused).toBe(false);
  });

  it("移除:failed 项移除后,后续指令照常补投", async () => {
    const { sends } = stubSend();
    const { result, rerender } = renderHook(
      ({ running, turnEnded }) => useComposer("a", feed({ running, turnEnded })),
      { initialProps: { running: true, turnEnded: false } },
    );
    act(() => result.current.setDraft("失败项"));
    act(() => {
      result.current.send();
    });
    act(() => result.current.setDraft("后续项"));
    act(() => {
      result.current.send();
    });
    rerender({ running: false, turnEnded: true });
    await waitFor(() => expect(sends().length).toBe(1));
    turnCycle(rerender, false); // 首条失败
    await waitFor(() => expect(result.current.queue[0]?.state).toBe("failed"));

    act(() => result.current.removeInstr(result.current.queue[0]!.id));
    act(() => result.current.togglePaused()); // 解除暂停
    await waitFor(() => expect(sends().length).toBe(2));
    expect(textOf(sends()[1]!.args)).toBe("后续项");
    turnCycle(rerender, true);
    await waitFor(() => expect(result.current.queue).toHaveLength(0));
  });

  it("暂停:轮结束不自动补投;恢复后才投", async () => {
    const { sends } = stubSend();
    const { result, rerender } = renderHook(
      ({ running, turnEnded }) => useComposer("a", feed({ running, turnEnded })),
      { initialProps: { running: true, turnEnded: false } },
    );
    act(() => result.current.setDraft("排队中"));
    act(() => {
      result.current.send();
    });
    act(() => result.current.togglePaused()); // 先暂停
    rerender({ running: false, turnEnded: true });
    await settle();
    expect(sends().length).toBe(0); // 暂停不投

    act(() => result.current.togglePaused()); // 恢复
    await waitFor(() => expect(sends().length).toBe(1));
  });

  it("切会话不把上一个会话的队列消息投进新会话", async () => {
    const { sends } = stubSend();
    const { result, rerender } = renderHook(({ id, running }) => useComposer(id, feed({ running })), {
      initialProps: { id: "a", running: true },
    });
    act(() => result.current.setDraft("给 A 的话"));
    act(() => {
      result.current.send();
    });
    // 切到空闲的 b:这一帧里 sessionId 已是 b,而 queue 还属于 A
    rerender({ id: "b", running: false });
    await settle();
    expect(sends().filter((c) => c.args.id === "b")).toHaveLength(0);

    // 消息还在 A 的队列里,切回来照样在
    rerender({ id: "a", running: true });
    expect(result.current.queue[0]?.text).toBe("给 A 的话");
  });

  it("首份历史落地前不抢投恢复出来的队列消息(running 还不可信)", async () => {
    const { sends } = stubSend();
    stashSet("a", { draft: "", queue: [qi("切回来要补投的")], atts: [] });
    const { result, rerender } = renderHook(({ historyLoaded, running, turnEnded }) => useComposer("a", feed({ historyLoaded, running, turnEnded })), {
      initialProps: { historyLoaded: false, running: false, turnEnded: false },
    });
    await settle();
    expect(result.current.queue[0]?.text).toBe("切回来要补投的");
    expect(sends()).toHaveLength(0);

    rerender({ historyLoaded: true, running: false, turnEnded: false });
    await settle();
    expect(sends()).toHaveLength(1);
    // 完整轮次:开轮 → task-ended 结束 → 成功出队
    rerender({ historyLoaded: true, running: true, turnEnded: false });
    rerender({ historyLoaded: true, running: false, turnEnded: true });
    await waitFor(() => expect(result.current.queue).toHaveLength(0));
  });

  it("上行在途(壳已 ack、回显帧未到)时第二条进队列;帧到达才补投", async () => {
    const { sends } = stubSend();
    const { result, rerender } = renderHook(({ lastSeq, running, turnEnded }) => useComposer("a", feed({ lastSeq, running, turnEnded })), {
      initialProps: { lastSeq: 0, running: false, turnEnded: false },
    });
    act(() => result.current.setDraft("第一条"));
    act(() => {
      result.current.send();
    });
    act(() => result.current.setDraft("第二条"));
    act(() => {
      result.current.send();
    });
    await settle();
    expect(result.current.queue[0]?.text).toBe("第二条");
    expect(sends()).toHaveLength(1);

    rerender({ lastSeq: 7, running: false, turnEnded: false }); // 回显帧到达
    await settle();
    expect(sends()).toHaveLength(2);
    turnCycle(rerender, true);
    await waitFor(() => expect(result.current.queue).toHaveLength(0));
  });
});

describe("useComposer:投递被壳拒", () => {
  it("session_send 拒绝:该指令标 failed + 自动暂停,不再自动重试", async () => {
    const { sends } = stubSend(() => {
      throw new Error("engine busy");
    });
    const { result, rerender } = renderHook(({ running }) => useComposer("a", feed({ running })), {
      initialProps: { running: true },
    });
    act(() => result.current.setDraft("会被拒的"));
    act(() => {
      result.current.send();
    });
    rerender({ running: false });
    await settle();
    expect(sends()).toHaveLength(1);
    await waitFor(() => expect(result.current.queue[0]?.state).toBe("failed"));
    expect(result.current.paused).toBe(true);
  });
});

describe("useComposer:附件上传的纪元守卫", () => {
  it("上传落地时人已切走:附件归原会话留档,不落进当前 composer", async () => {
    let finish: (v: { path: string }) => void = () => {};
    const pending = new Promise<{ path: string }>((r) => {
      finish = r;
    });
    (window as unknown as { __TAURI__?: unknown }).__TAURI__ = {
      core: {
        invoke: (cmd: string) => (cmd === "upload_file_path" ? pending : Promise.resolve(null)),
      },
    };
    const { result, rerender } = renderHook(({ id }) => useComposer(id, feed()), {
      initialProps: { id: "a" },
    });
    let done: Promise<void> = Promise.resolve();
    act(() => {
      done = result.current.addPaths(["/proj-a/图.png"]);
    });
    rerender({ id: "b" }); // 大文件上传数秒,期间切走了

    await act(async () => {
      finish({ path: ".monkeycode/uploads/图.png" });
      await done;
    });
    // 落进当前 composer 的话,path 是**旧工作区**的相对路径,发出去读不到
    expect(result.current.atts).toHaveLength(0);
    expect(stashGet("a")?.atts.map((a) => a.path)).toEqual([".monkeycode/uploads/图.png"]);

    rerender({ id: "a" });
    expect(result.current.atts.map((a) => a.path)).toEqual([".monkeycode/uploads/图.png"]);
  });
});
