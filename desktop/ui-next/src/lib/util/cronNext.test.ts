import { describe, expect, it } from "vitest";

import { cronNextPreview } from "./cronNext";

describe("cronNextPreview", () => {
  it("返回最近未来时间 + 可读标签", () => {
    const r = cronNextPreview("0 9 * * 1-5");
    expect(r).not.toBeNull();
    expect(r!.date).toBeInstanceOf(Date);
    expect(r!.date.getTime()).toBeGreaterThan(Date.now());
    expect(typeof r!.label).toBe("string");
    expect(r!.label.length).toBeGreaterThan(0);
  });

  it("全通配表达式返回几分钟后", () => {
    const r = cronNextPreview("* * * * *");
    expect(r).not.toBeNull();
    expect(r!.label).toMatch(/分钟/);
  });

  it("非法表达式返回 null", () => {
    expect(cronNextPreview("0 9 * *")).toBeNull();
    expect(cronNextPreview("60 * * * *")).toBeNull();
    expect(cronNextPreview("a * * * *")).toBeNull();
    expect(cronNextPreview("0 25 * * *")).toBeNull();
  });

  it("周末-only 表达式不会在工作日返回", () => {
    // 遍历足够保证命中周末(周六或周日)的下一次触发
    const r = cronNextPreview("0 12 * * 0");
    expect(r).not.toBeNull();
    expect([0, 6]).toContain(r!.date.getDay());
  });
});
