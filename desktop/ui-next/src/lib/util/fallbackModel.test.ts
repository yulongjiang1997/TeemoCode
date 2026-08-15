import { describe, expect, it } from "vitest";
import { nextFallbackModel } from "@/lib/util/fallbackModel";

const resolve = (v: string) =>
  ({ "deepseek-v4": "deepseek-v4", "DeepSeek V4": "deepseek-v4", "gpt-4o": "gpt-4o", "GPT-4o": "gpt-4o", "claude-3.5": "claude-3.5", "Claude 3.5": "claude-3.5" })[v];

describe("nextFallbackModel", () => {
  const backups = ["DeepSeek V4", "GPT-4o", "Claude 3.5"];

  it("主模型(不在链里)取第一个备用", () => {
    expect(nextFallbackModel("primary-model", backups, resolve)).toBe("DeepSeek V4");
  });

  it("当前是第 n 个备用时取第 n+1 个(链能一路走完)", () => {
    expect(nextFallbackModel("DeepSeek V4", backups, resolve)).toBe("GPT-4o");
    expect(nextFallbackModel("GPT-4o", backups, resolve)).toBe("Claude 3.5");
  });

  it("当前是引擎返回的模型 ID(非显示名)也能按身份匹配前进", () => {
    expect(nextFallbackModel("deepseek-v4", backups, resolve)).toBe("GPT-4o");
    expect(nextFallbackModel("gpt-4o", backups, resolve)).toBe("Claude 3.5");
  });

  it("最后一个备用没有下一格", () => {
    expect(nextFallbackModel("Claude 3.5", backups, resolve)).toBeUndefined();
  });

  it("空链:任何当前模型都没有下一格", () => {
    expect(nextFallbackModel("anything", [], resolve)).toBeUndefined();
  });
});
