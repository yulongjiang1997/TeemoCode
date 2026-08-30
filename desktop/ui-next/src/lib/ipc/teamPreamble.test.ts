// 团队编排指令 v2 测试:契约结构、技能交集、退化形态、上限截断、
// strip 新旧格式 round-trip、预设实例化。(纯函数,无 localStorage 依赖)
import { describe, expect, it } from "vitest";

import {
  buildTeamPreambleFrom,
  effectiveWorkflow,
  stripTeamPreamble,
  TEAM_MAX_ROLES,
} from "./teamPreamble";
import { instantiatePreset, TEAM_PRESETS } from "@/lib/util/teamPresets";
import type { TeamRole, TeamWorkflowStep } from "@/lib/util/prefs";

function role(name: string, skill = "", skills: string[] = []): TeamRole {
  return { id: `r-${name}`, name, skill, skills };
}

const step = (title: string, roleIds: string[]): TeamWorkflowStep => ({
  id: `w-${title}`,
  title,
  roleIds,
});

describe("buildTeamPreambleFrom:编排契约结构", () => {
  it("基本结构:标签包裹 + 协调者定位 + 成员清单 + 协作规则", () => {
    const p = buildTeamPreambleFrom([role("策划", "需求分析与方案设计", ["feature-design"])], []);
    expect(p.startsWith("[mc-team]\n")).toBe(true);
    expect(p.trimEnd().endsWith("[/mc-team]")).toBe(true);
    expect(p).toContain("任务协调者");
    expect(p).toContain("- 策划 — 需求分析与方案设计");
    expect(p).toContain("技能: feature-design");
    // 关键协作规则:上下文自带(子代理看不到父会话)+ 并行/防冲突 + 先验收
    expect(p).toContain("子代理看不到我们的对话");
    expect(p).toContain("并行");
    expect(p).toContain("验收");
  });

  it("无角色返回空串(useComposer 据此跳过注入)", () => {
    expect(buildTeamPreambleFrom([], [])).toBe("");
  });

  it("会话启用集非空时技能取交集,交集为空则不写技能行", () => {
    const roles = [role("后端", "接口实现", ["golang-patterns", "golang-testing"])];
    const p = buildTeamPreambleFrom(roles, ["golang-patterns"]);
    expect(p).toContain("技能: golang-patterns");
    expect(p).not.toContain("golang-testing");
    const p2 = buildTeamPreambleFrom(roles, ["完全不相关的技能"]);
    expect(p2).not.toContain("技能:");
  });

  it("职责为空的角色给保守提示", () => {
    const p = buildTeamPreambleFrom([role("神秘人")], []);
    expect(p).toContain("- 神秘人 — （职责待定,谨慎分派核心任务）");
  });

  it("工作流按序渲染,悬空 roleId 与空标题步骤被过滤", () => {
    const roles = [role("A"), role("B")];
    const wf = [step("方案设计", ["r-A"]), step("实现", ["r-A", "r-B"]), step("幽灵阶段", ["r-ghost"]), step("   ", ["r-A"])];
    const p = buildTeamPreambleFrom(roles, [], wf);
    expect(p).toContain("## 工作流");
    expect(p).toContain("1. 方案设计: A");
    expect(p).toContain("2. 实现: A、B");
    expect(p).not.toContain("幽灵阶段");
  });

  it("无工作流时不出现工作流段", () => {
    const p = buildTeamPreambleFrom([role("A")], [], []);
    expect(p).not.toContain("## 工作流");
  });

  it(`角色超过 ${TEAM_MAX_ROLES} 截断并提示`, () => {
    const roles = Array.from({ length: TEAM_MAX_ROLES + 3 }, (_, i) => role(`R${i}`));
    const p = buildTeamPreambleFrom(roles, []);
    expect(p).toContain(`仅列前 ${TEAM_MAX_ROLES} 名成员`);
    expect(p).toContain("- R7 —");
    expect(p).not.toContain("- R8 —");
  });
});

describe("effectiveWorkflow", () => {
  it("悬空角色与无名角色被丢弃", () => {
    const roles = [role("A")];
    const out = effectiveWorkflow(roles, [
      step("有效", ["r-A", "r-x"]),
      step("全悬空", ["r-x"]),
      step("", ["r-A"]),
    ]);
    expect(out).toEqual([{ title: "有效", roleNames: ["A"] }]);
  });
});

describe("stripTeamPreamble:新旧格式", () => {
  it("v2 标签格式完整剥离", () => {
    const inner = "你是本会话的任务协调者。\n## 团队成员\n- A";
    const text = `[mc-team]\n${inner}\n[/mc-team]\n\n真正的用户任务`;
    expect(stripTeamPreamble(text)).toBe("真正的用户任务");
  });

  it("旧版 [团队协调] 无标签格式兼容剥离", () => {
    const legacy =
      "[团队协调] 你是任务协调者。本会话配置了以下团队成员(统一使用会话主模型):\n" +
      "- Alice: 职责: 后端开发; 技能: rust\n" +
      "请拆解用户的任务,分派给合适成员执行(用 Agent 工具,子代理指令注明角色、职责、指定技能与任务),执行时成员职责与指定技能都生效、优先使用指定技能,需要时并行执行,最后汇总结果回复用户。\n" +
      "真正的用户任务";
    expect(stripTeamPreamble(legacy)).toBe("真正的用户任务");
  });

  it("旧格式缺收尾句时退化为头行+成员行", () => {
    const legacy = "[团队协调] 你是任务协调者。成员如下:\n- Alice: 职责: 开发\n修个登录 bug";
    expect(stripTeamPreamble(legacy)).toBe("修个登录 bug");
  });

  it("普通消息原样返回", () => {
    expect(stripTeamPreamble("修复登录超时问题")).toBe("修复登录超时问题");
  });

  it("round-trip:生成的前缀经 strip 后不残留标记", () => {
    const p = buildTeamPreambleFrom([role("A", "职责")], []);
    const rest = stripTeamPreamble(p + "开发一个功能");
    expect(rest).toBe("开发一个功能");
    expect(rest).not.toContain("[mc-team]");
  });
});

describe("内置团队预设", () => {
  it("四套预设:角色非空、实例化后工作流 roleId 全部有效", () => {
    expect(TEAM_PRESETS.length).toBe(4);
    for (const preset of TEAM_PRESETS) {
      const { roles, workflow } = instantiatePreset(preset, () => crypto.randomUUID());
      expect(roles.length).toBeGreaterThan(0);
      expect(roles.every((r) => r.id && r.name && r.skill)).toBe(true);
      const ids = new Set(roles.map((r) => r.id));
      for (const s of workflow) {
        expect(s.title).toBeTruthy();
        expect(s.roleIds.length).toBeGreaterThan(0);
        expect(s.roleIds.every((id) => ids.has(id))).toBe(true);
      }
      // 预设可直接生成编排指令
      const p = buildTeamPreambleFrom(roles, [], workflow);
      expect(p.startsWith("[mc-team]")).toBe(true);
    }
  });

  it("精简单兵无工作流;全栈交付有 3 个阶段", () => {
    const solo = TEAM_PRESETS.find((p) => p.id === "solo")!;
    expect(solo.workflow).toHaveLength(0);
    const fullstack = TEAM_PRESETS.find((p) => p.id === "fullstack")!;
    expect(fullstack.workflow).toHaveLength(3);
  });
});
