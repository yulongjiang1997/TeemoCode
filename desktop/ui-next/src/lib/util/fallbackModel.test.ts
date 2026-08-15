import { describe, expect, it } from "vitest";
import { nextFallbackModel } from "@/lib/util/fallbackModel";

describe("nextFallbackModel", () => {
  const backups = ["deepseek-v4", "gpt-4o", "claude-3.5"];

  it("主模型不在链里时取第一个备用", () => {
    expect(nextFallbackModel("primary-model", backups)).toBe("deepseek-v4");
  });

  it("当前是第 n 个备用时取第 n+1 个(链能一路走完)", () => {
    expect(nextFallbackModel("deepseek-v4", backups)).toBe("gpt-4o");
    expect(nextFallbackModel("gpt-4o", backups)).toBe("claude-3.5");
  });

  it("最后一个备用没有下一格", () => {
    expect(nextFallbackModel("claude-3.5", backups)).toBeUndefined();
  });

  it("空链:任何当前模型都没有下一格", () => {
    expect(nextFallbackModel("anything", [])).toBeUndefined();
  });
});
