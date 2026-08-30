// 计划模式测试:前缀契约结构、组合剥离(计划/团队任意顺序叠加)、普通消息不受影响。
import { describe, expect, it } from "vitest";

import { buildPlanPreamble, stripInjectedPreambles } from "./planMode";
import { buildTeamPreambleFrom } from "./teamPreamble";
import type { TeamRole } from "@/lib/util/prefs";

const role = (name: string): TeamRole => ({ id: `r-${name}`, name, skill: "职责", skills: [] });

describe("buildPlanPreamble", () => {
  it("标签包裹 + 只调研不动手 + 等确认语义", () => {
    const p = buildPlanPreamble();
    expect(p.startsWith("[mc-plan]\n")).toBe(true);
    expect(p.trimEnd().endsWith("[/mc-team]") || p.trimEnd().endsWith("[/mc-plan]")).toBe(true);
    expect(p).toContain("不执行任何改动");
    expect(p).toContain("实施计划");
    expect(p).toContain("等我确认");
  });
});

describe("stripInjectedPreambles", () => {
  it("仅计划块", () => {
    expect(stripInjectedPreambles(buildPlanPreamble() + "修复登录 bug")).toBe("修复登录 bug");
  });

  it("计划 + 团队叠加(计划在外层,与拼装顺序一致)", () => {
    const text = buildPlanPreamble() + buildTeamPreambleFrom([role("A")], []) + "做这个功能";
    expect(stripInjectedPreambles(text)).toBe("做这个功能");
  });

  it("顺序颠倒也能剥干净", () => {
    const text = buildTeamPreambleFrom([role("A")], []) + buildPlanPreamble() + "做这个功能";
    expect(stripInjectedPreambles(text)).toBe("做这个功能");
  });

  it("旧版团队格式([团队协调])仍兼容", () => {
    const legacy =
      "[团队协调] 你是任务协调者。本会话配置了以下团队成员(统一使用会话主模型):\n" +
      "- Alice: 职责: 开发\n" +
      "请拆解用户的任务,分派给合适成员执行,最后汇总结果回复用户。\n" +
      "真正任务";
    expect(stripInjectedPreambles(legacy)).toBe("真正任务");
  });

  it("普通消息原样返回", () => {
    expect(stripInjectedPreambles("修复登录超时问题")).toBe("修复登录超时问题");
    expect(stripInjectedPreambles("[mc-plan] 未闭合的块不算注入")).toBe("[mc-plan] 未闭合的块不算注入");
  });
});
