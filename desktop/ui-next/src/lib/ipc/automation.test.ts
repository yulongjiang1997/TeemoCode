import { afterEach, describe, expect, it, vi } from "vitest";

import { automationDelete, automationList, automationRunNow, automationSave } from "./automation";

afterEach(() => vi.unstubAllGlobals());

function stubInvoke(impl: (cmd: string, args?: Record<string, unknown>) => Promise<unknown>) {
  const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = [];
  vi.stubGlobal("window", {
    __TAURI__: {
      core: {
        invoke: (cmd: string, args?: Record<string, unknown>) => {
          calls.push({ cmd, args });
          return impl(cmd, args);
        },
      },
    },
  });
  return calls;
}

const auto = {
  id: "", name: "test", enabled: true, kind: "cron", cron: "0 9 * * *",
  fire_at_ms: 0, prompt: "测试", kind_session: "chat", workdir: "",
  model: "", last_fire_ms: 0, last_result: "",
};

describe("automation 契约", () => {
  it("浏览器模式:列表空,动作抛错", async () => {
    vi.stubGlobal("window", {});
    expect(await automationList()).toEqual([]);
    await expect(automationSave(auto)).rejects.toThrow();
  });

  it("桌面模式:命令与参数形状", async () => {
    const calls = stubInvoke((cmd) => {
      if (cmd === "automation_list") return Promise.resolve([]);
      if (cmd === "automation_save") return Promise.resolve({ ...auto, id: "a1" });
      if (cmd === "automation_delete") return Promise.resolve(null);
      if (cmd === "automation_run_now") return Promise.resolve({ ok: true, detail: "ok:s1", latency_ms: 42 });
      return Promise.resolve(null);
    });
    const list = await automationList();
    expect(list).toEqual([]);
    expect(calls[0]).toEqual({ cmd: "automation_list", args: undefined });

    const saved = await automationSave(auto);
    expect(saved.id).toBe("a1");
    expect(calls[1]).toEqual({ cmd: "automation_save", args: { automation: auto } });

    await automationDelete("a1");
    expect(calls[2]).toEqual({ cmd: "automation_delete", args: { id: "a1" } });

    const run = await automationRunNow("a1");
    expect(run.ok).toBe(true);
    expect(run.latency_ms).toBe(42);
    expect(calls[3]).toEqual({ cmd: "automation_run_now", args: { id: "a1" } });
  });
});
