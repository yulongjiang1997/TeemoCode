import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { GatewayLogEntry, GatewayStatus } from "@/lib/ipc/gateway";
import { GatewaySection } from "./GatewaySection";

afterEach(() => {
  delete (window as unknown as { __TAURI__?: unknown }).__TAURI__;
  vi.unstubAllGlobals();
});

const groupStatus = (over: Partial<GatewayStatus> = {}): GatewayStatus => ({
  running: true,
  enabled: true,
  port: 8317,
  error: null,
  groups: [
    {
      id: "mg-1",
      name: "主力组",
      enabled: true,
      key: "tgk-abc",
      strategy: "priority",
      context_window: 128_000,
      max_output: 32_768,
      temperature: null,
      system_prompt: "",
      timeout_seconds: 120,
      counters: { total: 3, ok: 2, fail: 1, failovers: 2 },
      models: [
        {
          id: "gm-1",
          enabled: true,
          weight: 9,
          alias: "库模型",
          provider: "",
          base_url: "",
          api_key: "",
          model: "m-lib",
          label: "库模型",
          upstream_model: "m-lib",
          unavailable: null,
          health: "healthy",
        },
        {
          id: "gm-2",
          enabled: true,
          weight: 5,
          alias: "旧模型",
          provider: "",
          base_url: "",
          api_key: "",
          model: "",
          label: "旧模型",
          upstream_model: "",
          unavailable: "模型库中不存在「旧模型」(可能已删除或改名)",
          health: "open",
        },
      ],
    },
  ],
  ...over,
});

function stubShell(status: GatewayStatus | Error, extra: Record<string, () => unknown> = {}) {
  const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = [];
  (window as unknown as { __TAURI__?: unknown }).__TAURI__ = {
    core: {
      invoke: (cmd: string, args?: Record<string, unknown>) => {
        calls.push({ cmd, args });
        if (cmd in extra) return Promise.resolve(extra[cmd]!());
        if (cmd === "gateway_status") return status instanceof Error ? Promise.reject(status) : Promise.resolve(status);
        if (cmd === "gateway_log") return Promise.resolve([]);
        if (cmd === "get_config") return Promise.resolve({ models: [{ name: "库模型", provider: "openai", base_url: "https://x", api_key: "k", model: "m-lib" }] });
        return Promise.resolve(null);
      },
    },
    event: { listen: () => Promise.resolve(() => {}) },
  };
  return { calls };
}

describe("GatewaySection", () => {
  it("运行态:端点/端口/组行渲染,展开可见熔断与不可用明细", async () => {
    stubShell(groupStatus());
    render(<GatewaySection />);
    expect(await screen.findByText("http://127.0.0.1:8317/v1")).toBeDefined();
    expect(await screen.findByText("主力组")).toBeDefined();
    // 行内即有熔断徽标;明细(不可用原因)在展开视图里
    expect(screen.getByText("熔断中")).toBeDefined();
    await userEvent.click(screen.getByText("主力组"));
    expect(await screen.findByText("模型库中不存在「旧模型」(可能已删除或改名)")).toBeDefined();
    expect(screen.getByText("tgk-abc")).toBeDefined();
  });

  it("总开关切换走 gateway_update_settings(true+端口)", async () => {
    const { calls } = stubShell(groupStatus());
    render(<GatewaySection />);
    const toggle = await screen.findByRole("checkbox", { name: "启用模型网关" });
    await userEvent.click(toggle);
    await waitFor(() => expect(calls.some((c) => c.cmd === "gateway_update_settings" && c.args?.enabled === false && c.args?.port === 8317)).toBe(true));
  });

  it("新建组:填名保存,gateway_save_group 收到表单载荷", async () => {
    const { calls } = stubShell(groupStatus({ groups: [] }), { gateway_save_group: () => ({ id: "mg-new", name: "新组" }) });
    render(<GatewaySection />);
    await userEvent.click(await screen.findByRole("button", { name: "新建模型组" }));
    const nameInput = screen.getByLabelText("组名称");
    await userEvent.type(nameInput, "新组");
    await userEvent.click(screen.getByRole("button", { name: "保存组" }));
    await waitFor(() => {
      const call = calls.find((c) => c.cmd === "gateway_save_group");
      expect(call).toBeDefined();
      expect((call!.args!.group as { name: string }).name).toBe("新组");
    });
  });

  it("删除走两段确认:第一下只布防,第二下才真删", async () => {
    const { calls } = stubShell(groupStatus());
    render(<GatewaySection />);
    await screen.findByText("主力组");
    // 悬停才可见,但按钮一直在 DOM:删除按钮的类里有 hover:text-error
    const del = screen.getAllByRole("button").find((b) => b.className.includes("hover:text-error"));
    expect(del).toBeDefined();
    await userEvent.click(del!);
    expect(calls.some((c) => c.cmd === "gateway_delete_group")).toBe(false);
    await userEvent.click(del!);
    await waitFor(() => expect(calls.some((c) => c.cmd === "gateway_delete_group" && c.args?.id === "mg-1")).toBe(true));
  });

  it("请求日志按新到旧排列并带流式徽标", async () => {
    const entries: GatewayLogEntry[] = [
      { ts_ms: 1_000, group_id: "mg-1", group_name: "主力组", stream: false, ok: true, status: 200, latency_ms: 120, model: "m-lib", attempts: 1, prompt_tokens: 3, completion_tokens: 5, error: null },
      { ts_ms: 2_000, group_id: "mg-1", group_name: "主力组", stream: true, ok: false, status: 502, latency_ms: 900, model: "", attempts: 2, prompt_tokens: 3, completion_tokens: null, error: "全部失败" },
    ];
    const calls: Array<{ cmd: string }> = [];
    (window as unknown as { __TAURI__?: unknown }).__TAURI__ = {
      core: {
        invoke: (cmd: string) => {
          calls.push({ cmd });
          if (cmd === "gateway_status") return Promise.resolve(groupStatus());
          if (cmd === "gateway_log") return Promise.resolve(entries);
          if (cmd === "get_config") return Promise.resolve({ models: [] });
          return Promise.resolve(null);
        },
      },
      event: { listen: () => Promise.resolve(() => {}) },
    };
    render(<GatewaySection />);
    const rows = await screen.findAllByRole("row");
    const bodyRows = rows.slice(1);
    expect(bodyRows).toHaveLength(2);
    expect(bodyRows[0]!.textContent).toContain("502");
    expect(bodyRows[0]!.textContent).toContain("流式");
    expect(bodyRows[1]!.textContent).toContain("200");
  });
});
