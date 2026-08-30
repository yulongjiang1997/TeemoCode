// 团队编排指令生成 v2:读取团队角色配置,生成 [mc-team]…[/mc-team] 前缀。
// 主模型作为协调者(coordinator),按职责把任务分派给成员(引擎 Agent 工具),
// 参考开源多智能体模式(Claude Code subagents 的触发式委派、CrewAI 的
// role/goal + manager 验收、MetaGPT 的 SOP 工作流、AutoGen 的并行/终止条件)
// 固化成一份编排契约:
//   - 成员清单(名称 — 职责 — 技能绑定)是委派决策依据;
//   - 工作流(SOP)可选,按序推进、阶段验收;
//   - 协作规则强制"委派 prompt 自带完整上下文"(子代理看不到父会话)、
//     并行/串行、文件权属、先验收后汇总。
//
// 标记格式 [mc-team]…[/mc-team] 与 LogList 的 stripTeamPreamble 同源;
// 旧版无标签格式([团队协调] 开头)在 strip 中兼容,生成器已不再产出。

import { readTeamRoles, readTeamWorkflow, type TeamRole, type TeamWorkflowStep } from "@/lib/util/prefs";

export const TEAM_PREAMBLE_START = "[mc-team]";
export const TEAM_PREAMBLE_END = "[/mc-team]";
/// 角色数上限:超出后委派决策质量下降(prompt 臃肿),截断并提示。
export const TEAM_MAX_ROLES = 8;

/**
 * 构建团队编排指令前缀(从本地偏好读取角色与工作流)。
 * @param enabledSkills 当前会话启用的技能名列表(空 = 不做交集过滤)
 * @returns 团队指令文本;无角色则返回空串。
 */
export function buildTeamPreamble(enabledSkills: string[]): string {
  return buildTeamPreambleFrom(readTeamRoles(), enabledSkills, readTeamWorkflow());
}

/** 纯函数形态(测试与预览用):给定角色/技能/工作流,产出编排前缀。 */
export function buildTeamPreambleFrom(
  roles: TeamRole[],
  enabledSkills: string[],
  workflow: TeamWorkflowStep[] = [],
): string {
  const valid = roles.filter((r) => r.name.trim());
  if (valid.length === 0) return "";
  const truncated = valid.length > TEAM_MAX_ROLES;
  const members = valid.slice(0, TEAM_MAX_ROLES).map((r) => memberLine(r, enabledSkills)).join("\n");

  const lines: string[] = [];
  lines.push(TEAM_PREAMBLE_START);
  lines.push("你是本会话的任务协调者。你的职责是拆解、分派、验收、汇总,不是独自完成全部工作。");
  lines.push("");
  lines.push("## 团队成员");
  lines.push(members);
  if (truncated) {
    lines.push(`(仅列前 ${TEAM_MAX_ROLES} 名成员,其余忽略)`);
  }
  const steps = effectiveWorkflow(valid, workflow);
  if (steps.length > 0) {
    lines.push("");
    lines.push("## 工作流(按序推进,每阶段完成并验收后再进入下一阶段)");
    steps.forEach((s, i) => {
      lines.push(`${i + 1}. ${s.title}: ${s.roleNames.join("、")}`);
    });
  }
  lines.push("");
  lines.push("## 协作规则");
  lines.push("- 用 Agent 工具分派成员执行,委派指令中注明成员角色与指定技能");
  lines.push(
    "- 每次委派的 prompt 必须自带完整上下文:任务目标、工作区路径、相关文件、已知约束、验收标准——子代理看不到我们的对话",
  );
  lines.push("- 相互独立的任务并行执行,有依赖的串行;每个成员只改自己负责的部分,避免互相覆盖");
  lines.push("- 子代理返回后先对照验收标准验收,不合格明确指出问题打回重做");
  lines.push(TEAM_PREAMBLE_END);
  return lines.join("\n") + "\n";
}

function memberLine(r: TeamRole, enabledSkills: string[]): string {
  const duty = r.skill.trim() || "（职责待定,谨慎分派核心任务）";
  const allSkills = r.skills.filter((s) => s.trim());
  // 会话级技能列表非空时取交集:角色绑定的技能须真的在本会话启用,
  // 否则子代理拿到的技能目录里没有它,指令就是空话。
  const effective = enabledSkills.length > 0 ? allSkills.filter((s) => enabledSkills.includes(s)) : allSkills;
  const head = `- ${r.name.trim()} — ${duty}`;
  if (effective.length === 0) return head;
  return `${head}\n  技能: ${effective.join(", ")}(分派给它时优先按这些技能执行)`;
}

/** 生效工作流:过滤悬空 roleId 与空步骤(角色可能已被删除)。 */
export function effectiveWorkflow(
  roles: TeamRole[],
  workflow: TeamWorkflowStep[],
): Array<{ title: string; roleNames: string[] }> {
  const nameOf = new Map(roles.map((r) => [r.id, r.name.trim()] as const));
  const out: Array<{ title: string; roleNames: string[] }> = [];
  for (const step of workflow) {
    const title = step.title.trim();
    const roleNames = (step.roleIds ?? []).map((id) => nameOf.get(id)).filter((n): n is string => Boolean(n));
    if (!title || roleNames.length === 0) continue;
    out.push({ title, roleNames });
  }
  return out;
}

/**
 * 剥掉发送时注入的团队编排块,消息气泡只显示原文。
 * 兼容两种历史格式:v2 标签格式与新 v2 之前的 [团队协调] 无标签块。
 */
export function stripTeamPreamble(text: string): string {
  const tagged = text.match(/^\[mc-team\][\s\S]*?\[\/mc-team\]\n*\s*/);
  if (tagged) return text.slice(tagged[0].length);
  // 旧版:[团队协调] 头行 + 若干 "- 成员" 行 + 固定收尾句。优先按收尾句
  // 截,格式漂移时退化为头行+成员行。
  const legacyFull = text.match(/^\[团队协调\][\s\S]*?汇总结果回复用户。\n*/);
  if (legacyFull) return text.slice(legacyFull[0].length);
  const legacyLoose = text.match(/^\[团队协调\][^\n]*(?:\n-[^\n]*)*\n*/);
  if (legacyLoose) return text.slice(legacyLoose[0].length).replace(/^\s+/, "");
  return text;
}