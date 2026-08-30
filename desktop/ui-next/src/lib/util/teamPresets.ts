// 内置团队预设:参考开源多智能体项目的经典班组固化成可一键应用的配置
// (CrewAI 的角色分工 + MetaGPT 的 SOP 流水线)。预设内容(职责文本)即
// 编排指令的一部分,面向模型,固定中文;界面标签走 i18n。
//
// skills 引用的是官方技能库(plugins submodule)里的技能名,与
// skills.rs DEFAULT_ENABLED 的出厂启用集同源;会话未启用对应技能时,
// teamPreamble 生成时按会话启用集做交集过滤,不会产生空话指令。

import type { TeamRole, TeamWorkflowStep } from "@/lib/util/prefs";

export type TeamPresetId = "fullstack" | "review" | "docs" | "solo";

export interface TeamPreset {
  id: TeamPresetId;
  /** i18n 标签键 = `settings.team.preset.<id>`(id 为字面量联合,键类型可收窄) */
  roles: Array<Pick<TeamRole, "name" | "skill" | "skills">>;
  /** roleIdx 指向 roles 下标;应用时映射为生成的角色 id */
  workflow: Array<{ title: string; roleIdx: number[] }>;
}

export const TEAM_PRESETS: TeamPreset[] = [
  {
    id: "fullstack",
    roles: [
      { name: "策划", skill: "需求分析、功能规划、方案设计;输出精炼的实施方案与任务拆解", skills: ["feature-design", "implementation-planner"] },
      { name: "前端开发", skill: "前端页面与交互实现,遵循现有组件与样式约定", skills: ["tailwindcss-helper", "shadcnui-helper", "feature-implementer"] },
      { name: "后端开发", skill: "后端接口与数据实现,遵循仓库现有架构", skills: ["golang-patterns", "golang-testing", "feature-implementer"] },
      { name: "测试", skill: "编写与执行测试,验证验收标准并回报结果", skills: ["golang-testing"] },
    ],
    workflow: [
      { title: "方案设计", roleIdx: [0] },
      { title: "并行实现", roleIdx: [1, 2] },
      { title: "测试验证", roleIdx: [3] },
    ],
  },
  {
    id: "review",
    roles: [
      { name: "实现工程师", skill: "按需求完成任务,产出可评审的代码变更", skills: ["feature-implementer"] },
      { name: "代码评审员", skill: "逐文件评审变更:正确性、边界条件、可维护性", skills: ["golang-code-review"] },
      { name: "安全审查员", skill: "审查注入/越权/敏感信息泄露等安全风险", skills: ["security-review"] },
    ],
    workflow: [
      { title: "实现", roleIdx: [0] },
      { title: "评审与安全审查", roleIdx: [1, 2] },
    ],
  },
  {
    id: "docs",
    roles: [
      { name: "资料整理员", skill: "梳理仓库结构与现有文档,产出要点清单", skills: ["project-wiki"] },
      { name: "技术作者", skill: "撰写面向使用者的文档与示例", skills: ["feature-design"] },
      { name: "校对员", skill: "核对文档与实际代码行为一致,修正过时描述", skills: [] },
    ],
    workflow: [
      { title: "资料整理", roleIdx: [0] },
      { title: "撰写", roleIdx: [1] },
      { title: "校对", roleIdx: [2] },
    ],
  },
  {
    id: "solo",
    roles: [{ name: "执行者", skill: "独立完成任务的通用工程师,自行拆解并汇报进度", skills: ["feature-implementer"] }],
    workflow: [],
  },
];

/** 预设 → 可持久化的角色 + 工作流(生成新 id,roleIdx 映射为角色 id)。 */
export function instantiatePreset(
  preset: TeamPreset,
  newId: () => string,
): { roles: TeamRole[]; workflow: TeamWorkflowStep[] } {
  const roles: TeamRole[] = preset.roles.map((r) => ({ id: newId(), name: r.name, skill: r.skill, skills: [...r.skills] }));
  const workflow: TeamWorkflowStep[] = preset.workflow
    .map((s) => ({
      id: newId(),
      title: s.title,
      roleIds: s.roleIdx.map((i) => roles[i]?.id).filter((id): id is string => Boolean(id)),
    }))
    .filter((s) => s.roleIds.length > 0);
  return { roles, workflow };
}
