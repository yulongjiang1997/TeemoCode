import { describe, expect, it } from "vitest";

import { createChatState } from "@/lib/protocol/reduce";
import type { ChatItem, ChatState } from "@/lib/protocol/types";
import { projectTimelineRows } from "./model";

const stateOf = (items: ChatItem[]): ChatState => ({ ...createChatState(), items });
const opts = { openGroups: new Set<number>(), closedGroups: new Set<number>() };

describe("projectTimelineRows", () => {
  it("移除锚定审批与被覆盖模型行，不再制造隐藏 DOM", () => {
    const state = stateOf([
      { kind: "sys", tag: "model", text: "a" },
      { kind: "sys", tag: "model", text: "b" },
      { kind: "tool", tcId: "t", title: "Bash", status: "run", out: "" },
      { kind: "perm", id: "p", title: "x", tool: "Bash", state: "open", toolCallId: "t" },
    ]);
    const rows = projectTimelineRows(state, opts);
    expect(rows).toHaveLength(2);
    expect(rows.map((row) => row.rawIndex)).toEqual([1, 2]);
    expect(rows[1]?.perm?.id).toBe("p");
  });

  it("终态长工具组收成一行，展开后只产生真实可见成员", () => {
    const tools: ChatItem[] = [0, 1, 2, 3].map((i) => ({
      kind: "tool",
      tcId: `t${i}`,
      title: "Bash",
      status: "ok",
      out: "",
    }));
    const state = stateOf(tools);
    const collapsed = projectTimelineRows(state, opts);
    expect(collapsed).toHaveLength(1);
    expect(collapsed[0]?.type).toBe("group");
    const stackKey = Number(collapsed[0]?.key);
    const expanded = projectTimelineRows(state, { ...opts, openGroups: new Set([stackKey]) });
    expect(expanded).toHaveLength(4);
    expect(new Set(expanded.map((row) => row.key)).size).toBe(4);
  });

  it("加载更早时既有条目的 row key 保持不变", () => {
    const current = stateOf([{ kind: "user", text: "now" }]);
    const before = projectTimelineRows(current, opts)[0]!.key;
    const prepended: ChatState = {
      ...current,
      keyBase: -1,
      items: [{ kind: "user", text: "old" }, ...current.items],
    };
    expect(projectTimelineRows(prepended, opts)[1]!.key).toBe(before);
  });

  it("完整助手轮次只在最后一段挂合并后的 Markdown", () => {
    const done = stateOf([
      { kind: "user", text: "问" },
      { kind: "agent", text: "第一段" },
      { kind: "tool", tcId: "t", title: "Bash", status: "ok", out: "" },
      { kind: "agent", text: "第二段" },
      { kind: "sys", tag: "turn-end", text: "" },
    ]);
    const rows = projectTimelineRows(done, opts);
    const agents = rows.filter((row) => row.type === "item" && row.item.kind === "agent");
    expect(agents[0]?.type === "item" ? agents[0].agentCopySource : undefined).toBeUndefined();
    expect(agents[1]?.type === "item" ? agents[1].agentCopySource : undefined).toBe("第一段\n\n第二段");

    const running = { ...stateOf([{ kind: "agent", text: "半截" }]), running: true };
    const runningRow = projectTimelineRows(running, opts)[0];
    expect(runningRow?.type === "item" ? runningRow.agentCopySource : undefined).toBeUndefined();
  });
});
