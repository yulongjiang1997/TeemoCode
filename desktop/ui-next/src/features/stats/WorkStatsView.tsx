//
// 工作统计面板:按任务维度统计 token 用量 + 活跃天数 + 代码修改量。
// 支持今日/近7日/累计范围切换,与用量统计体验一致。
// 聚合口径与 UsageStatsView 相同:按 days 过滤求和 + 子代理归并父任务 +
// 同 workdir 的代码修改只计一次(去重)。
//
import { IconRefresh } from "@tabler/icons-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { useI18n } from "@/lib/i18n";
import { sessionsList, type SessionMeta } from "@/lib/ipc/sessions";
import { usageStats, type SessionRow } from "@/lib/ipc/usageStats";
import { repoDiffNumstat, type DiffNumstatResult } from "@/lib/ipc/repo";

const fmtCompact = (n: number): string => {
  if (!Number.isFinite(n)) return "0";
  const sign = n < 0 ? "-" : "";
  n = Math.abs(n);
  if (n >= 1_000_000) { const v = n / 1_000_000; return `${sign}${v >= 10 ? Math.round(v) : Math.round(v * 10) / 10}M`; }
  if (n >= 1_000) { const v = n / 1_000; return `${sign}${v >= 10 ? Math.round(v) : Math.round(v * 10) / 10}K`; }
  return `${sign}${Math.round(n)}`;
};

const dateKey = (offset: number): string => {
  const d = new Date();
  d.setDate(d.getDate() - offset);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
};

type Range = "today" | "last7" | "total";

interface TaskRow {
  id: string;
  title: string;
  model: string;
  inputTokens: number;
  outputTokens: number;
  calls: number;
  durationMs: number;
  days: string[]; // 活跃日期键(热力图用)
  codeAdded: number;
  codeDeleted: number;
}

/** 模块级缓存:App 层预加载写入,切换 Tab 直接读。 */
let _cachedSessions: SessionRow[] | null = null;
let _cachedMeta: SessionMeta[] | null = null;
let _cachedDiffs: Map<string, DiffNumstatResult> | null = null;
let _cachedDays: DayRow[] | null = null;
let _cachedAt = 0;
const CACHE_TTL = 60_000;

type DayRow = { date: string; input_tokens: number; output_tokens: number; calls: number };

async function fetchRawData(): Promise<{ sessions: SessionRow[]; metas: SessionMeta[]; diffs: Map<string, DiffNumstatResult>; days: DayRow[] }> {
  const [stats, metas] = await Promise.all([
    usageStats(),
    sessionsList().catch((): SessionMeta[] => []),
  ]);

  // 代码修改按 workdir 去重:同工作区多个会话只查询一次
  const byWorkdir = new Map<string, string>(); // workdir → 代表会话 id
  for (const s of metas) {
    if (s.workdir && !byWorkdir.has(s.workdir)) byWorkdir.set(s.workdir, s.id);
  }
  const diffs = new Map<string, DiffNumstatResult>();
  await Promise.all([...byWorkdir].map(async ([, sid]) => {
    try { diffs.set(sid, await repoDiffNumstat(sid)); } catch { /* 忽略 */ }
  }));

  return { sessions: stats.sessions ?? [], metas, diffs, days: stats.days ?? [] };
}

export function prefetchWorkStats(): void {
  if (_cachedSessions && Date.now() - _cachedAt < CACHE_TTL) return;
  fetchRawData().then((d) => {
    _cachedSessions = d.sessions; _cachedMeta = d.metas; _cachedDiffs = d.diffs; _cachedDays = d.days; _cachedAt = Date.now();
  }).catch(() => {});
}

/** 范围内的日期键集合(total 不过滤 → null) */
const rangeDates = (range: Range): Set<string> | null => {
  if (range === "total") return null;
  const set = new Set<string>();
  const days = range === "today" ? 1 : 7;
  for (let i = 0; i < days; i++) set.add(dateKey(i));
  return set;
};

/** 范围内的天数(今天/近7日)对会话 days 求和——与 UsageStatsView.aggSessionInRange 同口径 */
function aggInRange(s: SessionRow, dates: Set<string> | null): { input: number; output: number; calls: number; activeDates: string[]; durationMs: number } | null {
  if (!dates) {
    return {
      input: s.input_tokens, output: s.output_tokens, calls: s.calls,
      activeDates: s.days.map((d) => d.date),
      durationMs: s.duration_ms ?? 0,
    };
  }
  const hit = s.days.filter((d) => dates.has(d.date));
  if (hit.length === 0) return null;
  const out = { input: 0, output: 0, calls: 0, activeDates: hit.map((d) => d.date), durationMs: 0 };
  for (const d of hit) { out.input += d.input_tokens; out.output += d.output_tokens; out.calls += d.calls; out.durationMs += d.duration_ms ?? 0; }
  return out;
}

/** 毫秒 → XhXmXs(全 0 隐藏单位,如 5s / 3m10s / 1h5m9s) */
export const fmtDuration = (ms: number): string => {
  if (!ms || ms <= 0) return "0s";
  const totalSec = Math.round(ms / 1000);
  const h = Math.floor(totalSec / 3600);
  const m = Math.floor((totalSec % 3600) / 60);
  const s = totalSec % 60;
  const parts: string[] = [];
  if (h > 0) parts.push(`${h}h`);
  if (m > 0 || h === 0) parts.push(`${m}m`);
  if (s > 0 || parts.length === 0) parts.push(`${s}s`);
  return parts.join("");
};

export function WorkStatsView() {
  const { t } = useI18n();
  const [loading, setLoading] = useState(!_cachedSessions);
  const [sessions, setSessions] = useState<SessionRow[]>(_cachedSessions ?? []);
  const [metas, setMetas] = useState<SessionMeta[]>(_cachedMeta ?? []);
  const [diffs, setDiffs] = useState<Map<string, DiffNumstatResult>>(_cachedDiffs ?? new Map());
  const [allDays, setAllDays] = useState<DayRow[]>(_cachedDays ?? []);
  const [range, setRange] = useState<Range>("last7");
  // 热力图选中某天 → 切到该天单日明细(与 UsageStatsView 同交互)
  const [pickedDate, setPickedDate] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const mountedRef = useRef(true);

  const load = useCallback(async (silent = false) => {
    if (!silent) setLoading(true);
    setError(null);
    try {
      const d = await fetchRawData();
      _cachedSessions = d.sessions; _cachedMeta = d.metas; _cachedDiffs = d.diffs; _cachedDays = d.days; _cachedAt = Date.now();
      if (mountedRef.current) { setSessions(d.sessions); setMetas(d.metas); setDiffs(d.diffs); setAllDays(d.days); }
    } catch (e) {
      if (mountedRef.current) setError(e instanceof Error ? e.message : String(e));
    } finally {
      if (mountedRef.current) setLoading(false);
    }
  }, []);

  useEffect(() => {
    mountedRef.current = true;
    if (_cachedSessions && Date.now() - _cachedAt < CACHE_TTL) {
      setSessions(_cachedSessions); setMetas(_cachedMeta ?? []); setDiffs(_cachedDiffs ?? new Map()); setAllDays(_cachedDays ?? []); setLoading(false);
    } else { void load(); }
    return () => { mountedRef.current = false; };
  }, [load]);

  // 任务聚合:子代理归并父任务 + 按范围过滤 days 求和
  // 热力图选中某天时按该天单日过滤(pickedDate 优先)
  const tasks = useMemo(() => {
    const dates = pickedDate ? new Set([pickedDate]) : rangeDates(range);
    const metaMap = new Map(metas.map((m) => [m.id, m]));
    // workdir → 代表会话 id(代码修改去重:同工作区只计一次)
    const workdirOwner = new Map<string, string>();
    for (const m of metas) {
      if (m.workdir && !workdirOwner.has(m.workdir)) workdirOwner.set(m.workdir, m.id);
    }

    // 顶层任务 id 集合(parent 为 null 且范围内有活动)
    const byId = new Map<string, SessionRow>();
    for (const s of sessions) byId.set(s.session_id, s);

    const rows: TaskRow[] = [];
    for (const s of sessions) {
      if (s.parent) continue; // 子代理:归并进父任务,不单独成行
      // 本会话 + 所有子会话的 days 按 range 过滤求和
      const members = [s, ...sessions.filter((x) => x.parent === s.session_id)];
      const agg = { input: 0, output: 0, calls: 0, activeDates: new Set<string>(), any: false, durationMs: 0 };
      for (const mbr of members) {
        const r = aggInRange(mbr, dates);
        if (!r) continue;
        agg.any = true;
        agg.input += r.input; agg.output += r.output; agg.calls += r.calls;
        agg.durationMs += r.durationMs;
        for (const d of r.activeDates) agg.activeDates.add(d);
      }
      if (!agg.any) continue;

      const meta = metaMap.get(s.session_id);
      // 代码修改:仅当本会话是该 workdir 的代表会话时计入
      const wd = meta?.workdir;
      const diffSid = wd ? workdirOwner.get(wd) : undefined;
      const diff = diffSid && diffSid === s.session_id ? diffs.get(diffSid) : undefined;

      const model = s.models[0]?.model || meta?.model || "";
      rows.push({
        id: s.session_id,
        title: s.title || meta?.title || "未命名",
        model,
        inputTokens: agg.input,
        outputTokens: agg.output,
        calls: agg.calls,
        durationMs: agg.durationMs,
        days: [...agg.activeDates].sort(),
        codeAdded: diff?.total_added ?? 0,
        codeDeleted: diff?.total_deleted ?? 0,
      });
    }
    rows.sort((a, b) => (b.inputTokens + b.outputTokens) - (a.inputTokens + a.outputTokens));
    return rows;
  }, [sessions, metas, diffs, range, pickedDate]);

  const totals = useMemo(() => {
    let input = 0, output = 0, calls = 0, added = 0, deleted = 0, durationMs = 0;
    for (const r of tasks) { input += r.inputTokens; output += r.outputTokens; calls += r.calls; added += r.codeAdded; deleted += r.codeDeleted; durationMs += r.durationMs; }
    return { input, output, calls, added, deleted, durationMs };
  }, [tasks]);

  const rangeLabel = range === "today" ? (t("stats.card.today") ?? "今日") : range === "last7" ? (t("stats.card.last7d") ?? "近 7 天") : (t("stats.card.total") ?? "累计");

  return (
    <div className="flex flex-col gap-4">
      {/* 头部 + 刷新 */}
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <h1 className="text-lg font-semibold">{t("stats.work.title") ?? "工作统计"}</h1>
          <p className="mt-0.5 text-xs text-base-content/50">
            {pickedDate && <span className="badge badge-sm badge-primary badge-outline me-1">{pickedDate}</span>}
            {rangeLabel}
          </p>
        </div>
        <button type="button" className="btn btn-ghost btn-xs shrink-0" onClick={() => void load()} disabled={loading}>
          <IconRefresh size={14} className={loading ? "animate-spin" : ""} />
          {t("stats.refresh") ?? "刷新"}
        </button>
      </div>

      {/* 范围切换 */}
      <div role="radiogroup" aria-label={t("stats.range")} className="join self-start">
        {(["today", "last7", "total"] as const).map((r) => {
          const label = r === "today" ? (t("stats.card.today") ?? "今日") : r === "last7" ? (t("stats.card.last7d") ?? "近 7 天") : (t("stats.card.total") ?? "累计");
          return (
            <button key={r} type="button" role="radio" aria-checked={range === r} className={`btn btn-xs join-item ${range === r ? "btn-primary" : ""}`}
              onClick={() => { setRange(r); setPickedDate(null); void load(true); }}>
              {label}
            </button>
          );
        })}
      </div>

      {error && <div className="alert alert-warning text-sm">{error}</div>}

      {/* 汇总卡 */}
      <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
        <SumCard label={t("stats.card.tokens") ?? "Token 用量"} value={fmtCompact(totals.input + totals.output)} sub={`${fmtCompact(totals.input)} ${t("stats.input") ?? "输入"} · ${fmtCompact(totals.output)} ${t("stats.output") ?? "输出"}`} />
        <SumCard label={t("stats.card.calls") ?? "调用次数"} value={fmtCompact(totals.calls)} sub={`${tasks.length} ${t("stats.work.tasks") ?? "个任务"}`} />
        <SumCard label={t("stats.work.duration") ?? "活跃时长"} value={fmtDuration(totals.durationMs)} sub={rangeLabel} />
        <SumCard label={t("stats.work.codeChanges") ?? "代码修改"} value={`+${fmtCompact(totals.added)} / -${fmtCompact(totals.deleted)}`} sub={`${totals.added + totals.deleted} ${t("stats.work.lines") ?? "行"}`} />
      </div>

      {/* 活跃热力图:一年窗口 + token 分位色阶,与用量统计同款。点某天 =
          该天单日明细(联动上方汇总卡与任务表)。 */}
      <UsageHeatmap
        days={allDays}
        selectedDate={pickedDate}
        onPickDay={setPickedDate}
      />

      {/* 任务明细表 */}
      {loading && tasks.length === 0 ? (
        <div className="flex items-center justify-center py-12 text-base-content/40">{t("stats.loading") ?? "加载中..."}</div>
      ) : tasks.length === 0 ? (
        <div className="flex items-center justify-center py-12 text-base-content/40">{rangeLabel}{t("stats.work.noActivity") ?? "暂无活动"}</div>
      ) : (
        <section className="rounded-box border border-base-300 bg-base-100">
          <h2 className="px-4 pt-3 text-sm font-medium">{t("stats.bySession") ?? "按任务"}</h2>
          <div className="overflow-x-auto p-2">
            <table className="table table-sm">
              <thead>
                <tr className="text-base-content/60">
                  <th>{t("stats.work.task") ?? "任务"}</th>
                  <th>{t("stats.work.model") ?? "模型"}</th>
                  <th className="text-right">{t("stats.card.tokens") ?? "Token"}</th>
                  <th className="text-right">{t("stats.card.calls") ?? "调用"}</th>
                  <th className="text-right">{t("stats.work.duration") ?? "时长"}</th>
                  <th className="text-right">{t("stats.work.added") ?? "新增"}</th>
                  <th className="text-right">{t("stats.work.deleted") ?? "删除"}</th>
                </tr>
              </thead>
              <tbody>
                {tasks.map((r) => (
                  <tr key={r.id} className="hover">
                    <td className="max-w-[240px] truncate font-medium" title={r.title}>{r.title}</td>
                    <td className="max-w-[140px] truncate text-base-content/60" title={r.model}>{r.model}</td>
                    <td className="text-right tabular-nums" title={`${r.inputTokens + r.outputTokens} tokens`}>{fmtCompact(r.inputTokens + r.outputTokens)}</td>
                    <td className="text-right tabular-nums">{fmtCompact(r.calls)}</td>
                    <td className="text-right tabular-nums">{fmtDuration(r.durationMs)}</td>
                    <td className="text-right tabular-nums text-success/80">{r.codeAdded > 0 ? `+${fmtCompact(r.codeAdded)}` : "-"}</td>
                    <td className="text-right tabular-nums text-error/80">{r.codeDeleted > 0 ? `-${fmtCompact(r.codeDeleted)}` : "-"}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </section>
      )}
    </div>
  );
}

/** 活跃热力图:复用 UsageStatsView 的日历格子风格。data: 日期 → 数值。 */
/** GitHub 提交图风格按天热力图(与 UsageStatsView 同款,独立于范围切换):
 * 色阶 = 当天 token 总量的四分位(Q25/Q50/Q75),一年窗口。点某天 =
 * 该天单日明细。 */
function UsageHeatmap({
  days,
  selectedDate,
  onPickDay,
}: {
  days: DayRow[];
  selectedDate: string | null;
  onPickDay: (date: string | null) => void;
}) {
  const { t } = useI18n();
  const byDate = new Map<string, DayRow>();
  for (const d of days) byDate.set(d.date, d);
  const total = (b: DayRow): number => b.input_tokens + b.output_tokens;

  const today = new Date();
  const start = new Date(today);
  start.setDate(start.getDate() - 52 * 7); // 一年窗口(GitHub 提交图风格)
  start.setDate(start.getDate() - start.getDay()); // 对齐到周日

  const weeks: { date: Date; bucket: DayRow | null }[][] = [];
  const cur = new Date(start);
  while (cur <= today) {
    const week: { date: Date; bucket: DayRow | null }[] = [];
    for (let i = 0; i < 7; i++) {
      const d = new Date(cur);
      const key = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
      week.push({ date: d, bucket: byDate.get(key) ?? null });
      cur.setDate(cur.getDate() + 1);
    }
    weeks.push(week);
  }
  const weeksShown = weeks.length;

  // 有数据的天按 token 总量排序取四分位阈值;零调用恒为 0 档(灰)
  const totals = days.map((d) => total(d)).filter((v) => v > 0).sort((a, b) => a - b);
  const quantile = (q: number): number =>
    totals.length === 0 ? 0 : totals[Math.min(totals.length - 1, Math.floor(q * totals.length))]!;
  const q25 = quantile(0.25);
  const q50 = quantile(0.5);
  const q75 = quantile(0.75);
  const level = (b: DayRow | null): number => {
    if (!b || b.calls <= 0) return 0;
    const v = total(b);
    if (v > q75) return 4;
    if (v > q50) return 3;
    if (v > q25) return 2;
    return 1;
  };
  const cellCls = ["bg-base-200", "bg-success/30", "bg-success/50", "bg-success/75", "bg-success"];
  const dowLabel = ["", "Mon", "", "Wed", "", "Fri", ""];

  const monthOf = (d: Date) => d.toLocaleDateString("en-US", { month: "short" });
  const monthCells = weeks.map((w) => monthOf(w[0]!.date));

  return (
    <section className="rounded-box border border-base-300 bg-base-100">
      <h2 className="px-4 pt-3 text-sm font-medium">
        {t("stats.heatmap.title")}
        <span className="ms-2 text-xs font-normal text-base-content/40">{t("stats.heatmap.period", { weeks: weeksShown })}</span>
      </h2>
      <p className="px-4 pt-0.5 text-[11px] text-base-content/40">{t("stats.heatmap.pickHint")}</p>
      <div className="px-4 pb-3 pt-1">
        <div className="flex gap-2">
          <div className="flex shrink-0 flex-col gap-[3px] pt-[17px]">
            {dowLabel.map((l, r) => (
              <span key={r} className={`flex h-[11px] items-center text-[9px] leading-none text-base-content/50 ${l ? "" : "opacity-0"}`}>
                {l}
              </span>
            ))}
          </div>
          <div className="flex min-w-0 flex-1 flex-col gap-[3px]">
            <div className="flex h-[14px] gap-[3px]">
              {monthCells.map((m, i) => {
                const label = m ?? "";
                const prev = monthCells[i - 1] ?? "";
                return (
                  <span
                    key={i}
                    className={`min-w-0 flex-1 overflow-visible whitespace-nowrap text-[9px] leading-[14px] text-base-content/50 ${i > 0 && label === prev ? "invisible" : ""}`}
                  >
                    {label}
                  </span>
                );
              })}
            </div>
            {[0, 1, 2, 3, 4, 5, 6].map((row) => (
              <div key={row} className="flex gap-[3px]">
                {weeks.map((w, wi) => {
                  const cell = w[row]!;
                  const b = cell.bucket;
                  const key = `${cell.date.getFullYear()}-${String(cell.date.getMonth() + 1).padStart(2, "0")}-${String(cell.date.getDate()).padStart(2, "0")}`;
                  const picked = key === selectedDate;
                  const isFuture = cell.date > today;
                  const dateLabel = cell.date.toLocaleDateString("zh-CN", { year: "numeric", month: "short", day: "numeric" });
                  return (
                    <button
                      key={wi}
                      type="button"
                      disabled={isFuture || !b}
                      title={
                        b
                          ? `${dateLabel} · ${t("stats.calls")} ${b.calls.toLocaleString("en-US")} · ↑${fmtCompact(b.input_tokens)} ↓${fmtCompact(b.output_tokens)}`
                          : `${dateLabel} · ${t("stats.calls")} 0`
                      }
                      onClick={() => onPickDay(picked ? null : key)}
                      aria-label={dateLabel}
                      aria-pressed={picked}
                      className={`h-[11px] min-w-0 flex-1 rounded-[2px] ${cellCls[level(b)]} ${
                        picked ? "ring-2 ring-primary ring-offset-1 ring-offset-base-100" : b ? "hover:ring-1 hover:ring-primary/50" : ""
                      } ${isFuture ? "opacity-30" : ""}`}
                    />
                  );
                })}
              </div>
            ))}
            <div className="mt-1 flex items-center justify-end gap-1 text-[10px] text-base-content/40">
              <span>{t("stats.heatmap.less")}</span>
              {cellCls.map((c, i) => (
                <span key={i} className={`h-[11px] w-[11px] rounded-[2px] ${c}`} />
              ))}
              <span>{t("stats.heatmap.more")}</span>
              {selectedDate && (
                <button
                  type="button"
                  className="btn btn-ghost btn-xs ms-2 h-auto min-h-0 py-0 text-[10px]"
                  onClick={() => onPickDay(null)}
                >
                  {t("stats.heatmap.clearPick")}
                </button>
              )}
            </div>
          </div>
        </div>
      </div>
    </section>
  );
}

function SumCard({ label, value, sub }: { label: string; value: string; sub: string }) {
  return (
    <div className="flex min-w-0 flex-1 flex-col gap-1 rounded-box border border-base-300 bg-base-100 px-4 py-3">
      <span className="text-xs text-base-content/50">{label}</span>
      <span className="text-2xl font-semibold tabular-nums leading-none">{value}</span>
      <span className="text-xs text-base-content/40 truncate">{sub}</span>
    </div>
  );
}
