// 实时任务面板(plan 帧驱动):钉在 composer 上方,不进对话流。
// 收起 = 一行摘要(进度 + 当前项),展开 = 限高滚动的只读勾选清单 +
// 并行子代理执行卡;整卡随 plan 全量重发更新(daisyUI collapse 强制开合态)。
import { IconChevronRight, IconX } from "@tabler/icons-react";
import { useState } from "react";

import { useI18n } from "@/lib/i18n";
import type { PlanEntry, ToolItem } from "@/lib/protocol/types";

function statusTone(status: "run" | "ok" | "fail"): string {
  return status === "run"
    ? "status-running"
    : status === "ok"
      ? "status-done"
      : "status-fail";
}

/** feed 里最后一段子代理文本(流式预览取末条)。 */
function feedLastText(s: ToolItem): string {
  const feed = s.feed;
  if (!feed || feed.length === 0) return "";
  for (let i = feed.length - 1; i >= 0; i--) {
    const e = feed[i];
    if (e && e.kind === "text") return e.text;
  }
  return "";
}

/** 模型名剥来源后缀,卡片上只显示短名。 */
function stripModelName(name: string): string {
  const i = name.indexOf("@");
  return i > 0 ? name.slice(0, i) : name;
}

export function TaskPanel({
  entries,
  subagents,
  onDismiss,
}: {
  entries: PlanEntry[];
  /** 正在/已完成执行的子代理工具卡(并行编排:每卡 = 一个子代理) */
  subagents?: ToolItem[];
  onDismiss: () => void;
}) {
  const { t } = useI18n();
  const [open, setOpen] = useState(false);
  const done = entries.filter((e) => e.status === "completed").length;
  const current =
    entries.find((e) => e.status === "in_progress") ?? entries.find((e) => e.status === "pending");
  // 依赖提示(上游 todo_update 携带 id/depends_on 时):id → 序号,blocked
  // 缺省按「有未完成依赖」本地推导(旧 taskPanel.tsx 同款)
  const byId = new Map(entries.map((e, i) => [e.id ?? "", { idx: i + 1, entry: e }]));
  const unfinishedDeps = (e: PlanEntry) =>
    (e.depends_on ?? []).filter((d) => byId.get(d)?.entry.status !== "completed");
  const isBlocked = (e: PlanEntry) =>
    e.status !== "completed" && (e.blocked ?? unfinishedDeps(e).length > 0);
  const depHint = (e: PlanEntry) => {
    const names = unfinishedDeps(e)
      .map((d) => byId.get(d))
      .filter((x): x is NonNullable<typeof x> => Boolean(x))
      .map((x) => `#${x.idx}`);
    return names.length ? t("chat.plan.waitDeps", { list: names.join(" ") }) : null;
  };
  // 有任何依赖关系时全员编号,「等 #N」才有落点
  const numbered = entries.some((e) => e.depends_on?.length);

  return (
    <div
      className={`collapse rounded-box border border-base-300 bg-base-100 ${open ? "collapse-open" : "collapse-close"}`}
    >
      <div className="collapse-title flex min-h-0 items-center gap-2 px-3 py-2 text-xs">
        <button
          type="button"
          aria-expanded={open}
          className="flex min-w-0 flex-1 items-center gap-2 text-left"
          onClick={() => setOpen(!open)}
        >
          <span className="shrink-0 font-semibold">
            {t("chat.plan.progress", { done, total: entries.length })}
          </span>
          {!open && current && (
            <span className="min-w-0 flex-1 truncate text-left text-base-content/60">
              · {current.status === "in_progress" ? t("chat.plan.doing") : t("chat.plan.next")}:
              {current.content}
            </span>
          )}
          <IconChevronRight
            size={14}
            stroke={1.75}
            aria-hidden
            className={`ml-auto shrink-0 text-base-content/40 transition-transform ${open ? "rotate-90" : ""}`}
          />
        </button>
        {/* 手动关闭:任务结束步骤未全部完成时面板会保留(供回顾),可随时收起;
            任务继续跑(新 plan/执行中)时自动重新出现 */}
        <button
          type="button"
          className="btn btn-ghost btn-square btn-xs shrink-0 text-base-content/50"
          aria-label={t("chat.plan.dismiss")}
          title={t("chat.plan.dismiss")}
          onClick={onDismiss}
        >
          <IconX size={13} stroke={1.75} aria-hidden />
        </button>
      </div>
      <div className="collapse-content px-3">
        <ul className="flex max-h-44 flex-col gap-1 overflow-x-hidden overflow-y-auto pb-1 text-xs">
          {entries.map((e, i) => {
            const blocked = isBlocked(e);
            const hint = depHint(e);
            return (
              <li key={e.id ?? i} className="flex items-start gap-2">
                <input
                  type="checkbox"
                  className="checkbox checkbox-xs mt-px shrink-0"
                  checked={e.status === "completed"}
                  readOnly
                  aria-label={e.content}
                />
                {numbered && (
                  <span className="shrink-0 text-base-content/40 tabular-nums">#{i + 1}</span>
                )}
                <span
                  className={
                    e.status === "completed"
                      ? "line-through opacity-50"
                      : blocked
                        ? "opacity-60" // blocked 行降色(比 completed 略实,旧 t4/t5 层级)
                        : e.status === "in_progress"
                          ? "font-medium text-primary"
                          : ""
                  }
                >
                  {e.content}
                  {hint && <span className="ml-1.5 text-base-content/60">· {hint}</span>}
                </span>
              </li>
            );
          })}
        </ul>
        {/* 并行子代理执行卡:每卡 = 一个子代理(状态/流式预览/结果) */}
        {subagents && subagents.length > 0 && (
          <div className="mt-2 border-t border-base-300/60 pt-1.5">
            <div className="mb-1 text-[10px] font-bold text-base-content/50">
              {t("chat.plan.subagents", { n: subagents.length })}
            </div>
            <ul className="flex max-h-36 flex-col gap-1 overflow-y-auto">
              {subagents.map((s) => {
                const preview = feedLastText(s);
                return (
                  <li key={s.tcId} className="rounded-box border border-base-300/70 bg-base-200/40 px-2 py-1">
                    <div className="flex items-center gap-1.5">
                      <span aria-hidden className={`status ${statusTone(s.status)}`} />
                      <span className="min-w-0 flex-1 truncate text-xs">{s.title || t("chat.plan.subagentNameless")}</span>
                      {s.model && (
                        <span className="shrink-0 max-w-24 truncate rounded bg-base-300/60 px-1 py-px font-mono text-[9px] text-base-content/60" title={s.model}>
                          {stripModelName(s.model)}
                        </span>
                      )}
                      <span className="shrink-0 text-[10px] text-base-content/50">
                        {s.status === "run" ? t("chat.plan.subagentRunning") : s.status === "ok" ? t("chat.plan.subagentDone") : t("chat.plan.subagentFailed")}
                      </span>
                    </div>
                    {s.status === "run" && preview && (
                      <p className="mt-1 truncate text-[10px] text-base-content/60">{preview}</p>
                    )}
                    {s.status !== "run" && (s.result || s.out) && (
                      <p className="mt-1 line-clamp-2 whitespace-pre-wrap break-all text-[10px] text-base-content/60">
                        {s.result || s.out}
                      </p>
                    )}
                  </li>
                );
              })}
            </ul>
          </div>
        )}
      </div>
    </div>
  );
}
