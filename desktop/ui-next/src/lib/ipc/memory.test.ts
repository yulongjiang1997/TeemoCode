// 工作区记忆 IPC 契约:命令名/参数对表 desktop/src/memory.rs;浏览器降级。
import { afterEach, describe, expect, it, vi } from "vitest";

import { memoryRead, memoryWrite } from "./memory";

afterEach(() => vi.unstubAllGlobals());

describe("memory 契约", () => {
  it("浏览器模式:读 null,写抛错", async () => {
    vi.stubGlobal("window", {});
    expect(await memoryRead("C:/w")).toBeNull();
    await expect(memoryWrite("C:/w", "x")).rejects.toThrow("浏览器模式");
  });

  it("桌面模式:命令名与参数形状", async () => {
    const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = [];
    vi.stubGlobal("window", {
      __TAURI__: {
        core: {
          invoke: (cmd: string, args?: Record<string, unknown>) => {
            calls.push({ cmd, args });
            return Promise.resolve(cmd === "memory_read" ? "# 记忆" : null);
          },
        },
      },
    });
    expect(await memoryRead("C:/work")).toBe("# 记忆");
    expect(calls[0]).toEqual({ cmd: "memory_read", args: { workdir: "C:/work" } });
    await memoryWrite("C:/work", "新内容");
    expect(calls[1]).toEqual({ cmd: "memory_write", args: { workdir: "C:/work", content: "新内容" } });
  });
});
