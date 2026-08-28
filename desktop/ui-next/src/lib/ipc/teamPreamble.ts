// 团队编排指令生成:读取团队角色配置,生成 [mc-team]...[/mc-team] 前缀。
// 当团队模式开启且有角色定义时,在用户消息前注入编排提示,
// 引擎识别后按角色分派任务(协调者/成员)。
//
// 格式:
// [mc-team]
// 你是任务协调者。本会话配置了以下团队成员(统一使用会话主模型):
// - Alice: 职责: 后端开发; 技能: rust(构建系统)
// - Bob: 职责: 前端开发; 技能: react(UI组件)
// 请拆解用户的任务,分派给合适成员执行(用 Agent 工具,子代理指令注明角色、职责、指定技能与任务),
// 需要时并行执行,最后汇总结果回复用户。
// [/mc-team]

import { readTeamRoles } from "@/lib/util/prefs";

/**
 * 构建团队编排指令前缀。
 * @param enabledSkills 当前会话启用的技能名列表(从 settings.json)
 * @returns 团队指令文本;若无角色或未启用则返回空串。
 */
export function buildTeamPreamble(enabledSkills: string[]): string {
  const roles = readTeamRoles();
  if (roles.length === 0) return "";

  const members = roles
    .map((r) => {
      const parts: string[] = [];
      if (r.skill.trim()) parts.push(`职责: ${r.skill.trim()}`);
      const allSkills = r.skills.filter((s) => s.trim());
      // 若会话级技能列表不为空,取交集;否则用角色自定义技能
      const effectiveSkills = enabledSkills.length > 0
        ? allSkills.filter((s) => enabledSkills.includes(s))
        : allSkills;
      if (effectiveSkills.length > 0) parts.push(`技能: ${effectiveSkills.join(", ")}`);
      return `- ${r.name}${parts.length > 0 ? ": " + parts.join("; ") : ": （未填写职责与技能）"}`;
    })
    .join("\n");

  return (
    "[团队协调] 你是任务协调者。本会话配置了以下团队成员(统一使用会话主模型):\n" +
    members +
    "\n请拆解用户的任务,分派给合适成员执行(用 Agent 工具,子代理指令注明角色、职责、指定技能与任务),执行时成员职责与指定技能都生效、优先使用指定技能,需要时并行执行,最后汇总结果回复用户。\n"
  );
}
