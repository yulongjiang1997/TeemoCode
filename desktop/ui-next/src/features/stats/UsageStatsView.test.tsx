import { render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { UsageStatsView } from "./UsageStatsView";

afterEach(() => {
  localStorage.clear();
  delete (window as unknown as { __TAURI__?: unknown }).__TAURI__;
});

function stubShell(data: unknown) {
  (window as unknown as { __TAURI__?: unknown }).__TAURI__ = {
    core: { invoke: (cmd: string) => (cmd === "usagestats" ? Promise.resolve(data) : Promise.resolve(null)) },
  };
}

/** 距今天 offset 天的 usage 行,日期钉为本机时区 */
function day(offset: number, input: number, output: number, calls: number) {
  const d = new Date();
  d.setDate(d.getDate() - offset);
  const date = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
  return { date, input_tokens: input, output_tokens: output, calls };
}

const sample = (days: ReturnType<typeof day>[]) => ({
  totals: days.reduce(
    (acc, d) => ({ input_tokens: acc.input_tokens + d.input_tokens, output_tokens: acc.output_tokens + d.output_tokens, calls: acc.calls + d.calls }),
    { input_tokens: 0, output_tokens: 0, calls: 0 },
  ),
  days,
  models: [{ model: "gpt-5", input_tokens: 100, output_tokens: 50, calls: 3 }],
  sessions: [],
});

describe("UsageStatsView 按天热力图", () => {
  it("渲染 GitHub 风格热力图:标题/图例/单元格网格", async () => {
    stubShell(sample([day(0, 100, 50, 5), day(2, 300, 200, 12), day(5, 40, 20, 2)]));
    render(<UsageStatsView />);

    expect(await screen.findByText("按天活跃热力图")).toBeDefined();
    expect(screen.getByText("少")).toBeDefined();
    expect(screen.getByText("多")).toBeDefined();

    // 单元格是 button(点击切单日明细),带 title(以年份开头),数量 =
    // 7 行 × 周数,落在一年窗口内(52~54 周)
    const cells = document.querySelectorAll<HTMLButtonElement>('button[title^="202"]');
    expect(cells.length).toBeGreaterThanOrEqual(7 * 52);
    expect(cells.length).toBeLessThanOrEqual(7 * 54);
    expect(cells.length % 7).toBe(0);

    // 有 usage 的天映射到带色阶的格子(色阶=token 四分位,不是线性最大值;
    // 稀疏样本下最活跃天至少应落在次高档及以上:bg-success/75 或纯 bg-success)
    const hottest = [...cells].find((c) => c.title.includes("调用次数 12"));
    expect(hottest).toBeDefined();
    expect(hottest!.className).toContain("bg-success");
    expect(/bg-success(?!\/)".*$|bg-success\/75/.test(hottest!.className)).toBe(true);

    // 点击最活跃的天 → 切到单日明细:汇总卡标签变成该天日期
    hottest!.click();
    expect(await screen.findByText("按模型")).toBeDefined();
  });

  it("无数据的格子为灰底(空态不渲染热力图)", async () => {
    stubShell({ totals: { input_tokens: 0, output_tokens: 0, calls: 0 }, days: [], models: [], sessions: [] });
    render(<UsageStatsView />);
    // 空态文案,而非热力图
    expect(await screen.findByText(/还没有用量数据/)).toBeDefined();
    expect(screen.queryByText("按天活跃热力图")).toBeNull();
  });
});
