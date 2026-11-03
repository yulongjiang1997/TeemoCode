import { beforeEach, describe, expect, it, vi } from "vitest";

import { sessionOpen, sessionSend } from "@/lib/ipc/sessions";
import { b64decode } from "@/lib/protocol/codec";

import {
  getRelentlessKeys,
  isUserStopped,
  kickRelentless,
  markUserStop,
  relentlessCandidates,
  relentlessDue,
  resetRelentlessForTests,
  setRelentlessSession,
} from "./relentless";

vi.mock("@/lib/ipc/sessions", () => ({
  sessionOpen: vi.fn(),
  sessionSend: vi.fn(),
}));

const session = (patch: Partial<{ id: string; workdir: string; status: string; archived: boolean; waiting_ask: boolean }>) => ({
  id: "s1",
  workdir: "D:\\repo",
  status: "error",
  archived: false,
  waiting_ask: false,
  ...patch,
});

describe("不死不休", () => {
  beforeEach(() => {
    resetRelentlessForTests();
    vi.mocked(sessionSend).mockReset();
    vi.mocked(sessionOpen).mockReset();
  });

  it("只重试勾选会话里出错或中断、且没在等用户的任务(按对话隔离,同工作区不同会话互不影响)", () => {
    // 勾选会话 err(同工作区还有其他会话,但只有 err 被勾选 → 只有 err 命中)
    setRelentlessSession("err", true);
    setRelentlessSession("stop", true);
    const ids = relentlessCandidates(
      [
        session({ id: "err" }),
        session({ id: "stop", status: "interrupted" }),
        session({ id: "idle", status: "idle" }),
        session({ id: "ask", waiting_ask: true }),
        session({ id: "arch", archived: true }),
        // 同工作区但未勾选的会话:不补发(旧版按 workdir 会误补)
        session({ id: "other", workdir: "D:/repo" }),
      ],
      getRelentlessKeys(),
      new Set(),
      new Set(),
    );
    expect(ids).toEqual(["err", "stop"]);
  });

  it("同一工作区:只勾选的那个会话被补发,同工作区其他会话不受影响", () => {
    // 只勾选 err;同工作区的 stop 不勾选 → 只有 err 命中
    setRelentlessSession("err", true);
    const ids = relentlessCandidates(
      [
        session({ id: "err" }),
        session({ id: "stop", status: "interrupted" }),
      ],
      getRelentlessKeys(),
      new Set(),
      new Set(),
    );
    expect(ids).toEqual(["err"]);
  });

  it("用户点停止后不再自动重开", () => {
    setRelentlessSession("s1", true);
    markUserStop("s1");
    expect(isUserStopped("s1")).toBe(true);
    expect(relentlessCandidates([session({})], getRelentlessKeys(), new Set(["s1"]), new Set())).toEqual([]);
  });

  it("第一次看见失败先等 2 秒,到点才重试", () => {
    expect(relentlessDue("s1", "error", 1_000)).toBe(false);
    expect(relentlessDue("s1", "error", 2_999)).toBe(false);
    expect(relentlessDue("s1", "error", 3_000)).toBe(true);
    expect(relentlessDue("s1", "idle", 4_000)).toBe(false);
    expect(relentlessDue("s1", "error", 4_000)).toBe(false);
  });

  it("会话没打开时先打开再补发继续", async () => {
    vi.mocked(sessionSend)
      .mockRejectedValueOnce(new Error("会话未打开"))
      .mockResolvedValueOnce(undefined);
    vi.mocked(sessionOpen).mockResolvedValue({ frames: [], cursor: 0, has_more: false });
    await kickRelentless("s1", "继续", () => 10_000);
    expect(sessionOpen).toHaveBeenCalledWith("s1", 1);
    const payload = vi.mocked(sessionSend).mock.calls[1]?.[2] as { content: string };
    expect(b64decode(payload.content)).toBe("继续");
  });
});
