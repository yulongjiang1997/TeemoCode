import { describe, expect, it } from "vitest";
import { nextFallbackModel } from "@/lib/util/fallbackModel";

// 用户配置:多个模型共用同一 model 字段(deepseek-v4-flash),名字各不相同;
// 会话 meta 可能是剥后缀的短名,备用链存全名
const resolve = (v: string) =>
  ({
    "主模型": "deepseek-v4-flash",
    "deepseek-v4-flash": "deepseek-v4-flash",
    "gpt-4o": "gpt-4o",
    "openAi-ds4-flash@monkeycode#764bc6aa": "deepseek-v4-flash",
    "opencodeai-ds4": "deepseek-v4-flash",
    "minimaxai/minimax-m3@monkeycode#587cbae9": "gpt-4o",
  })[v];

describe("nextFallbackModel", () => {
  const backups = [
    "minimaxai/minimax-m3@monkeycode#587cbae9",
    "openAi-ds4-flash@monkeycode#764bc6aa",
    "opencodeai-ds4",
  ];

  it("meta 是剥后缀短名,备用链是全名 → 仍按名字前进(不再永远取第一个)", () => {
    expect(nextFallbackModel("minimaxai/minimax-m3", backups, resolve)).toBe("openAi-ds4-flash@monkeycode#764bc6aa");
  });

  it("meta 是全名 → 同样前进", () => {
    expect(nextFallbackModel("minimaxai/minimax-m3@monkeycode#587cbae9", backups, resolve)).toBe("openAi-ds4-flash@monkeycode#764bc6aa");
    expect(nextFallbackModel("openAi-ds4-flash@monkeycode#764bc6aa", backups, resolve)).toBe("opencodeai-ds4");
  });

  it("主模型(不在链里)→ 取第一个备用,不因共用 model 字段跳过", () => {
    expect(nextFallbackModel("主模型", backups, resolve)).toBe("minimaxai/minimax-m3@monkeycode#587cbae9");
  });

  it("多个备用共用同一 model 字段时,ID 形态的当前视为主模型取第一个(身份无法区分)", () => {
    expect(nextFallbackModel("deepseek-v4-flash", backups, resolve)).toBe("minimaxai/minimax-m3@monkeycode#587cbae9");
  });

  it("当前身份唯一命中一个备用 → 取其下一个(引擎返回 ID 形态的兜底)", () => {
    // gpt-4o 只对应 minimax-m3
    expect(nextFallbackModel("gpt-4o", backups, resolve)).toBe("openAi-ds4-flash@monkeycode#764bc6aa");
  });

  it("最后一个备用没有下一格", () => {
    expect(nextFallbackModel("opencodeai-ds4", backups, resolve)).toBeUndefined();
  });

  it("空链:任何当前模型都没有下一格", () => {
    expect(nextFallbackModel("anything", [], resolve)).toBeUndefined();
  });
});
