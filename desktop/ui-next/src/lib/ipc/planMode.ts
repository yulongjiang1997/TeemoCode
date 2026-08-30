// 计划模式(Plan Mode,对标 ZCode):开启后发送注入 [mc-plan]…[/mc-plan]
// 前缀,本轮**只调研与产出实施计划、不动手改**——对应引擎已有的 plan 帧
// 呈现(计划清单钉在 composer 上方),等用户确认后再切回普通模式执行。
//
// 本模块同时提供组合剥离 stripInjectedPreambles:用户气泡不应显示任何
// 注入前缀(计划/团队可同时开启,顺序任意),LogList 统一走它。

import { stripTeamPreamble } from "./teamPreamble";

export const PLAN_PREAMBLE_START = "[mc-plan]";
export const PLAN_PREAMBLE_END = "[/mc-plan]";

/** 构建计划模式前缀;内容是面向模型的契约,固定中文。 */
export function buildPlanPreamble(): string {
  return (
    [
      PLAN_PREAMBLE_START,
      "计划模式:本轮**只做调研与方案设计,不执行任何改动**——不写文件、不运行会产生副作用的命令、不提交代码。",
      "- 先只读地调查相关代码与文档,弄清现状;",
      "- 然后产出一份实施计划:目标、分步步骤(每步涉及的文件)、风险与取舍、验证方式;",
      "- 计划输出后停下来,等我确认(我回复「按计划执行」后你再开始动手)。",
      PLAN_PREAMBLE_END,
    ].join("\n") + "\n"
  );
}

/** 剥离单个 [tag]…[/tag] 块(含尾随空行)。 */
function stripTagged(text: string, start: string, end: string): string {
  const re = new RegExp(`^\\${start}[\\s\\S]*?\\${end}\\n*\\s*`);
  const m = text.match(re);
  return m ? text.slice(m[0].length) : text;
}

/**
 * 组合剥离所有注入前缀(计划/团队,任意顺序、可叠加,循环至稳定)。
 * 旧版团队格式([团队协调])由 stripTeamPreamble 内部兼容。
 */
export function stripInjectedPreambles(text: string): string {
  let cur = text;
  for (let i = 0; i < 4; i += 1) {
    const next = stripTagged(stripTagged(cur, PLAN_PREAMBLE_START, PLAN_PREAMBLE_END), "[mc-team]", "[/mc-team]");
    const legacy = stripTeamPreamble(next);
    if (legacy === cur) break;
    cur = legacy;
  }
  return cur;
}
