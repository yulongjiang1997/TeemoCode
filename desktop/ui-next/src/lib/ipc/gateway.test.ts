import { afterEach, describe, expect, it, vi } from "vitest";

import {
  gatewayDeleteGroup,
  gatewayEndpoint,
  gatewayLog,
  gatewayRegenKey,
  gatewaySaveGroup,
  gatewayStatus,
  gatewayTestGroup,
  gatewayUpdateSettings,
} from "./gateway";

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

describe("gateway 契约:浏览器模式降级", () => {
  it("状态/日志类返回空态", async () => {
    vi.stubGlobal("window", {});
    expect(await gatewayStatus()).toBeNull();
    expect(await gatewayLog()).toEqual([]);
  });

  it("动作类抛「非桌面壳环境」", async () => {
    vi.stubGlobal("window", {});
    const actions: Array<() => Promise<unknown>> = [
      () => gatewaySaveGroup({ id: "", name: "g", enabled: true, key: "", strategy: "priority", context_window: 1, max_output: 1, temperature: null, system_prompt: "", timeout_seconds: 1, models: [] }),
      () => gatewayDeleteGroup("mg-1"),
      () => gatewayUpdateSettings(true, 8317),
      () => gatewayRegenKey("mg-1"),
      () => gatewayTestGroup("mg-1"),
    ];
    for (const act of actions) {
      await expect(act()).rejects.toThrow("非桌面壳环境");
    }
  });
});

describe("gateway 契约:命令与参数", () => {
  it("gatewayStatus / gatewayLog 直传命令名", async () => {
    const calls = stubInvoke((cmd) => Promise.resolve(cmd === "gateway_status" ? { running: true, port: 8317, enabled: true, error: null, groups: [] } : []));
    const status = await gatewayStatus();
    expect(status?.running).toBe(true);
    expect(calls[0]?.cmd).toBe("gateway_status");
    await gatewayLog(20);
    expect(calls[1]?.cmd).toBe("gateway_log");
    expect(calls[1]?.args).toEqual({ limit: 20 });
  });

  it("save/update/delete/regen/test 的参数形状", async () => {
    const calls = stubInvoke((cmd) =>
      Promise.resolve(cmd === "gateway_regen_key" ? "tgk-new" : cmd === "gateway_test_group" ? { ok: true, model: "m", latency_ms: 5 } : null),
    );
    const group = { id: "mg-1", name: "组", enabled: true, key: "tgk-x", strategy: "priority", context_window: 1000, max_output: 100, temperature: null, system_prompt: "", timeout_seconds: 10, models: [] };
    await gatewaySaveGroup(group);
    expect(calls[0]).toEqual({ cmd: "gateway_save_group", args: { group } });
    await gatewayUpdateSettings(false, 9000);
    expect(calls[1]?.args).toEqual({ enabled: false, port: 9000 });
    await gatewayDeleteGroup("mg-1");
    expect(calls[2]?.args).toEqual({ id: "mg-1" });
    expect(await gatewayRegenKey("mg-1")).toBe("tgk-new");
    expect(calls[3]?.args).toEqual({ id: "mg-1" });
    const result = await gatewayTestGroup("mg-1");
    expect(result.ok).toBe(true);
    expect(calls[4]?.args).toEqual({ id: "mg-1" });
  });

  it("gatewayEndpoint 拼接 /v1", () => {
    expect(gatewayEndpoint(8317)).toBe("http://127.0.0.1:8317/v1");
    expect(gatewayEndpoint(9000)).toBe("http://127.0.0.1:9000/v1");
  });
});
