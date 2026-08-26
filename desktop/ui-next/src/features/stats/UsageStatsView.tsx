// 本地会话 token 用量统计面板(侧栏「用量统计」空间主视图)。
//
// 数据来自壳侧 usage 事件记账(按天/会话/模型聚合),挂载时取一次,之后
// 手动刷新——统计面板不常驻轮询,与设置页的账号权益面板同理念。
//
// 布局:头部范围切换(今日/7日/累计)→ 汇总卡 → 每日趋势(最近 7 天,
// 可折叠)→ 按天活跃热力图(点某天 = 该天明细)→ 明细(按模型/按任务表,
// 跟随所选范围联动)。token 数值用 K/M 缩写(1.2K / 3.4M),完整值进
// tooltip——大数全量展开既难读又把表格撑爆(2026-08-26 用户反馈)。
import { IconChevronDown, IconRefresh } from "@tabler/icons-react";
import { useCallback, useEffect, useState } from "react";

import { useI18n } from "@/lib/i18n";
import { usageStats, type Bucket, type DayRow, type SessionRow, type UsageStats } from "@/lib/ipc/usageStats";

const errMsg = (e: unknown): string => (e instanceof Error ? e.message : String(e));

const total = (b: Bucket): number => b.input_tokens + b.output_tokens;

/** token 数缩写:<1000 原样;≥1000 用 K;≥100 万用 M。一位小数,整数不带
 *  小数点。表格里大数全量展开(11,749,178,44)既难读又撑爆列宽。 */
export const fmtCompact = (n: number): string => {
  if (!Number.isFinite(n)) return "0";
  const sign = n < 0 ? "-" : "";
  n = Math.abs(n);
  if (n >= 1_000_000) {
    const v = n / 1_000_000;
    return `${sign}${v >= 10 ? Math.round(v) : Math.round(v * 10) / 10}M`;
  }
  if (n >= 1_000) {
    const v = n / 1_000;
    return `${sign}${v >= 10 ? Math.round(v) : Math.round(v * 10) / 10}K`;
  }
  return `${sign}${Math.round(n)}`;
};

/** 浏览器本地时区的今天 `YYYY-MM-DD`,与壳侧 `stats::today()` 同口径 */
const todayKey = (): string => {
  const d = new Date();
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
};

const zeroBucket: Bucket = { input_tokens: 0, output_tokens: 0, calls: 0 };

/** 头部范围:今日 / 近7日 / 累计。热力图点击某天 = 单日范围(kind=day)。 */
type Range = { kind: "today" } | { kind: "day"; date: string } | { kind: "last7" } | { kind: "total" };
const RANGE_KEYS = ["today", "last7d", "total"] as const;

const rangeLabelKey = (r: Range): `stats.card.${"today" | "last7d" | "total"}` =>
  r.kind === "day" ? "stats.card.today" : r.kind === "last7" ? "stats.card.last7d" : "stats.card.total";

/** 汇总卡:大号总计(K/M 缩写)+ 「输入 X · 输出 Y · N 次」副行,完整数值 tooltip */
function SumCard({ label, bucket }: { label: string; bucket: Bucket }) {
  return (
    <div
      className="flex min-w-0 flex-1 flex-col gap-1 rounded-box border border-base-300 bg-base-100 px-4 py-3"
      title={`${fmtFull(total(bucket))} tokens`}
    >
      <span className="text-xs text-base-content/50">{label}</span>
      <span className="text-2xl font-semibold tabular-nums leading-none">{fmtCompact(total(bucket))}</span>
      <span
        className="text-xs tabular-nums text-base-content/50"
        title={`↑ ${fmtFull(bucket.input_tokens)} · ↓ ${fmtFull(bucket.output_tokens)}`}
      >
        ↑{fmtCompact(bucket.input_tokens)} · ↓{fmtCompact(bucket.output_tokens)} · {bucket.calls.toLocaleString("en-US")} 次
      </span>
    </div>
  );
}

/** 全量数字(tooltip 用):缩写会丢精度,悬停给回真实值 */
const fmtFull = (n: number): string => n.toLocaleString("en-US");

/** 每日趋势:最近 7 天(可折叠,默认收起只留标题行),每天一行堆叠条。
 *  数值同样走 K/M 缩写,tooltip 给全量。 */
function DailyChart({ days }: { days: UsageStats["days"] }) {
  const { t } = useI18n();
  const [open, setOpen] = useState(false);
  const recent = days.slice(0, 7).reverse();
  const max = Math.max(...recent.map((d) => total(d)), 1);
  return (
    <section className="rounded-box border border-base-300 bg-base-100">
      <button
        type="button"
        className="flex w-full items-center gap-2 px-4 py-3 text-start"
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
      >
        <span className="text-sm font-medium">{t("stats.daily")}</span>
        <span className="text-xs font-normal text-base-content/40">{t("stats.daily.last7")}</span>
        <IconChevronDown
          size={14}
          className={`ms-auto shrink-0 text-base-content/40 transition-transform ${open ? "" : "-rotate-90"}`}
          aria-hidden
        />
      </button>
      {open && (
        <div className="flex flex-col gap-1 px-4 pb-3">
          {recent.length === 0 && (
            <p className="py-6 text-center text-sm text-base-content/40">{t("stats.noData")}</p>
          )}
          {recent.map((d) => {
            const inW = Math.round((d.input_tokens / max) * 100);
            const outW = Math.round((d.output_tokens / max) * 100);
            return (
              <div key={d.date} className="flex items-center gap-3 text-xs tabular-nums">
                <span className="w-11 shrink-0 text-base-content/50">{d.date.slice(5)}</span>
                <div
                  className="flex h-3 min-w-0 flex-1 overflow-hidden rounded-sm bg-base-200"
                  title={`in ${fmtFull(d.input_tokens)} · out ${fmtFull(d.output_tokens)}`}
                >
                  <div className="h-full shrink-0 bg-primary" style={{ width: `${inW}%` }} />
                  <div className="h-full shrink-0 bg-primary/40" style={{ width: `${outW}%` }} />
                </div>
                <span
                  className="w-16 shrink-0 text-end"
                  title={fmtFull(total(d))}
                >
                  {fmtCompact(total(d))}
                </span>
              </div>
            );
          })}
          {recent.length > 0 && (
            <div className="mt-1 flex items-center gap-3 text-[11px] text-base-content/40">
              <span className="w-11 shrink-0" />
              <span className="flex items-center gap-3">
                <span className="inline-block size-2 rounded-sm bg-primary" /> {t("stats.input")}
                <span className="inline-block size-2 rounded-sm bg-primary/40" /> {t("stats.output")}
              </span>
            </div>
          )}
        </div>
      )}
    </section>
  );
}

/** GitHub 提交图风格按天热力图:色阶=当天 token 总量的分位数(不是线性
 *  相对最大值)。用量分布极度右偏(一天爆量、其余天寥寥),线性映射会让
 *  绝大多数天都挤在最浅档、只有孤零零的深格——观感就是"颜色规则不对"。
 *  改用四分位切档:Q25/Q50/Q75 分位把有数据的天均分四档,梯度自然铺开。
 *  点击某天 = 切到该天的单日明细(onPickDay)。 */
function UsageHeatmap({
  days,
  selectedDate,
  onPickDay,
}: {
  days: UsageStats["days"];
  selectedDate: string | null;
  onPickDay: (date: string | null) => void;
}) {
  const { t } = useI18n();
  const byDate = new Map<string, DayRow>();
  for (const d of days) byDate.set(d.date, d);

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
    // 阈值相等(数据集中)时低档被跳过没关系,高档必须可达
    if (v > q75) return 4;
    if (v > q50) return 3;
    if (v > q25) return 2;
    return 1;
  };
  const cellCls = ["bg-base-200", "bg-success/30", "bg-success/50", "bg-success/75", "bg-success"];
  const dowLabel = ["", "Mon", "", "Wed", "", "Fri", ""]; // 0=周日

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

/** 按模型表(跟随当前范围的过滤结果;K/M 缩写 + tooltip 全量) */
function ModelTable({ models }: { models: ModelRow[] }) {
  const { t } = useI18n();
  if (models.length === 0) return <p className="px-4 py-6 text-center text-sm text-base-content/40">{t("stats.noData")}</p>;
  return (
    <div className="overflow-x-auto">
      <table className="table table-xs">
        <thead>
          <tr className="text-base-content/50">
            <th>{t("stats.model")}</th>
            <th className="text-end">{t("stats.input")}</th>
            <th className="text-end">{t("stats.output")}</th>
            <th className="text-end">{t("stats.card.total")}</th>
            <th className="text-end">{t("stats.calls")}</th>
          </tr>
        </thead>
        <tbody>
          {models.map((m) => (
            <tr key={m.model}>
              <td className="font-mono text-xs" title={m.model}>{m.model}</td>
              <td className="text-end tabular-nums" title={fmtFull(m.input_tokens)}>{fmtCompact(m.input_tokens)}</td>
              <td className="text-end tabular-nums" title={fmtFull(m.output_tokens)}>{fmtCompact(m.output_tokens)}</td>
              <td className="text-end font-medium tabular-nums" title={fmtFull(total(m))}>{fmtCompact(total(m))}</td>
              <td className="text-end tabular-nums">{m.calls.toLocaleString("en-US")}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

interface ModelRow extends Bucket {
  model: string;
}

/** 单个会话的展开明细:按模型 + 按天 */
function SessionDetail({ session }: { session: SessionRow }) {
  const { t } = useI18n();
  return (
    <div className="grid gap-4 px-8 py-2">
      <div>
        <div className="mb-1 text-[11px] text-base-content/50">{t("stats.byModel")}</div>
        <table className="table table-xs">
          <tbody>
            {session.models.map((m) => (
              <tr key={m.model}>
                <td className="font-mono text-xs" title={m.model}>{m.model}</td>
                <td className="text-end tabular-nums" title={fmtFull(m.input_tokens)}>{fmtCompact(m.input_tokens)}</td>
                <td className="text-end tabular-nums" title={fmtFull(m.output_tokens)}>{fmtCompact(m.output_tokens)}</td>
                <td className="text-end font-medium tabular-nums" title={fmtFull(total(m))}>{fmtCompact(total(m))}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <div>
        <div className="mb-1 text-[11px] text-base-content/50">{t("stats.daily")}</div>
        <table className="table table-xs">
          <tbody>
            {session.days.map((d) => (
              <tr key={d.date}>
                <td className="tabular-nums">{d.date}</td>
                <td className="text-end tabular-nums" title={fmtFull(d.input_tokens)}>{fmtCompact(d.input_tokens)}</td>
                <td className="text-end tabular-nums" title={fmtFull(d.output_tokens)}>{fmtCompact(d.output_tokens)}</td>
                <td className="text-end font-medium tabular-nums" title={fmtFull(total(d))}>{fmtCompact(total(d))}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}

/** 按任务/会话表:顶层任务行 + 挂在父行下的子代理行,均可展开看明细。
 *  sessions 已由调用方按范围过滤。 */
function SessionTable({ sessions }: { sessions: SessionRow[] }) {
  const { t } = useI18n();
  const [open, setOpen] = useState<Record<string, boolean>>({});
  if (sessions.length === 0) return <p className="px-4 py-6 text-center text-sm text-base-content/40">{t("stats.noData")}</p>;

  const childrenByParent = new Map<string, SessionRow[]>();
  for (const s of sessions) {
    if (!s.parent) continue;
    const list = childrenByParent.get(s.parent) ?? [];
    list.push(s);
    childrenByParent.set(s.parent, list);
  }
  const topLevel = sessions.filter((s) => !s.parent);
  const orphaned = sessions.filter((s) => s.parent && !childrenByParent.has(s.parent) && !topLevel.some((t2) => t2.session_id === s.parent));

  const Row = ({ s, child }: { s: SessionRow; child?: boolean }) => {
    const expanded = !!open[s.session_id];
    const kids = child ? [] : (childrenByParent.get(s.session_id) ?? []);
    return (
      <>
        <tr
          className={`cursor-pointer hover:bg-base-200/60 ${child ? "text-base-content/70" : ""}`}
          onClick={() => setOpen((m) => ({ ...m, [s.session_id]: !expanded }))}
        >
          <td className="flex min-w-0 items-center gap-1.5">
            <IconChevronDown
              size={12}
              className={`shrink-0 text-base-content/40 transition-transform ${expanded ? "rotate-0" : "-rotate-90"}`}
              aria-hidden
            />
            <span className="truncate">{s.title || s.session_id}</span>
            {child && (
              <span className="badge badge-ghost badge-xs shrink-0 text-base-content/50">{t("stats.parentTask")}</span>
            )}
          </td>
          <td className="text-end tabular-nums" title={fmtFull(s.input_tokens)}>{fmtCompact(s.input_tokens)}</td>
          <td className="text-end tabular-nums" title={fmtFull(s.output_tokens)}>{fmtCompact(s.output_tokens)}</td>
          <td className="text-end font-medium tabular-nums" title={fmtFull(total(s))}>{fmtCompact(total(s))}</td>
          <td className="text-end tabular-nums">{s.calls.toLocaleString("en-US")}</td>
        </tr>
        {expanded && (
          <tr className="bg-base-200/40">
            <td colSpan={5}>
              <SessionDetail session={s} />
            </td>
          </tr>
        )}
        {!child &&
          kids.map((k) => (
            <Row key={k.session_id} s={k} child />
          ))}
      </>
    );
  };

  return (
    <div className="overflow-x-auto">
      <table className="table table-xs">
        <thead>
          <tr className="text-base-content/50">
            <th>{t("stats.session")}</th>
            <th className="text-end">{t("stats.input")}</th>
            <th className="text-end">{t("stats.output")}</th>
            <th className="text-end">{t("stats.card.total")}</th>
            <th className="text-end">{t("stats.calls")}</th>
          </tr>
        </thead>
        <tbody>
          {topLevel.map((s) => (
            <Row key={s.session_id} s={s} />
          ))}
          {orphaned.map((s) => (
            <Row key={s.session_id} s={s} />
          ))}
        </tbody>
      </table>
    </div>
  );
}

export function UsageStatsView() {
  const { t } = useI18n();
  const [stats, setStats] = useState<UsageStats | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  // 明细范围:头部三键切换;热力图点某天也落在这里(kind=day)
  const [range, setRange] = useState<Range>({ kind: "today" });

  const load = useCallback(async (silent = false) => {
    if (!silent) setBusy(true);
    setError("");
    try {
      setStats(await usageStats());
    } catch (e) {
      setError(errMsg(e));
    } finally {
      if (!silent) setBusy(false);
    }
  }, []);

  // 挂载时取一次;此后每 60s 静默轮询 + 窗口回到前台时刷新,保证跨天/后台
  // 任务消耗能被面板看到,不必手动点刷新
  useEffect(() => {
    void load();
    const timer = setInterval(() => void load(true), 60_000);
    const onVisible = () => {
      if (document.visibilityState === "visible") void load(true);
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      clearInterval(timer);
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, [load]);

  const today = todayKey();
  const byDate = new Map<string, UsageStats["days"][number]>();
  for (const d of stats?.days ?? []) byDate.set(d.date, d);

  /** 范围内的日期键集合(单日=那一天;近7日=含今天的 7 个自然日;累计=null 不过滤) */
  let rangeDates: Set<string> | null;
  if (range.kind === "day") rangeDates = new Set([range.date]);
  else if (range.kind === "today") {
    // today 与 day 同口径,只是日期键固定;必须给出集合而非 null,
    // 否则下方按天过滤的明细路径会拿空集/崩(null 是「不过滤」语义)
    rangeDates = new Set([todayKey()]);
  } else if (range.kind === "last7") {
    rangeDates = new Set();
    for (let i = 0; i < 7; i++) {
      const d = new Date();
      d.setDate(d.getDate() - i);
      rangeDates.add(`${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`);
    }
  } else rangeDates = null;

  // 当前范围的合计卡。⚠️ today 必须显式分支:它和 day 的区别只是日期键
  // 固定为今天,绝不能落到 total 的 else 里(否则今日卡显示累计——2026-08-26
  // 用户报障「今日与累计一样,点热力图反而正常」的根因)。
  let sumBucket: Bucket = { ...zeroBucket };
  if (range.kind === "today") {
    sumBucket = byDate.get(todayKey()) ?? zeroBucket;
  } else if (range.kind === "day") {
    sumBucket = byDate.get(range.date) ?? zeroBucket;
  } else if (range.kind === "last7") {
    for (const key of rangeDates!) {
      const b = byDate.get(key);
      if (b) sumBucket = { input_tokens: sumBucket.input_tokens + b.input_tokens, output_tokens: sumBucket.output_tokens + b.output_tokens, calls: sumBucket.calls + b.calls };
    }
  } else {
    sumBucket = stats?.totals ?? zeroBucket;
  }

  /** 范围内某会话行的聚合:day/today/last7 都按 rangeDates 过滤该会话的
   *  days 再求和(行合计与汇总卡同口径——否则单日视图里会话行还是全会话
   *  总量,和顶部对不上);total 不过滤。 */
  const aggSessionInRange = (s: SessionRow): SessionRow | null => {
    if (range.kind === "total") return s;
    const dayRows = s.days.filter((d) => rangeDates!.has(d.date));
    if (dayRows.length === 0) return null;
    const out = { session_id: s.session_id, title: s.title, parent: s.parent, days: dayRows, models: s.models, input_tokens: 0, output_tokens: 0, calls: 0 };
    for (const d of dayRows) {
      out.input_tokens += d.input_tokens;
      out.output_tokens += d.output_tokens;
      out.calls += d.calls;
    }
    // 按模型拆分壳侧只存了整会话口径(无按天×模型粒度),无法精确按范围
    // 拆;保留整会话 models 供展开明细参考,行级合计以 days 过滤结果为准。
    return out;
  };

  const rangeModels: ModelRow[] = (() => {
    if (range.kind === "total") return stats?.models ?? [];
    if (range.kind === "last7" || range.kind === "day") {
      // 会话行的 days 有按天模型吗?壳只存了整会话 models + 按天 tokens。
      // 精确按范围拆模型做不到,退而求其次:聚合「范围内有数据」的会话的
      // 整会话 models(口径:参与过范围内的调用),并在标题注明。
      const acc = new Map<string, Bucket & { model: string }>();
      for (const s of stats?.sessions ?? []) {
        const hit = s.days.some((d) => rangeDates!.has(d.date));
        if (!hit) continue;
        for (const m of s.models) {
          const cur = acc.get(m.model) ?? { model: m.model, input_tokens: 0, output_tokens: 0, calls: 0 };
          cur.input_tokens += m.input_tokens;
          cur.output_tokens += m.output_tokens;
          cur.calls += m.calls;
          acc.set(m.model, cur);
        }
      }
      return [...acc.values()].sort((a, b) => b.input_tokens + b.output_tokens - (a.input_tokens + a.output_tokens));
    }
    return [];
  })();

  const rangeSessions = range.kind === "total"
    ? stats?.sessions ?? []
    : (stats?.sessions ?? []).map(aggSessionInRange).filter((s): s is SessionRow => s !== null)
        .filter((s) => s.input_tokens > 0 || s.output_tokens > 0 || s.calls > 0)
        .sort((a, b) => b.input_tokens + b.output_tokens - (a.input_tokens + a.output_tokens));

  const rangeTabs: { r: Range; labelKey: (typeof RANGE_KEYS)[number] }[] = [
    { r: { kind: "today" }, labelKey: "today" },
    { r: { kind: "last7" }, labelKey: "last7d" },
    { r: { kind: "total" }, labelKey: "total" },
  ];

  return (
    <div className="min-h-0 flex-1 overflow-x-hidden overflow-y-auto [scrollbar-gutter:stable] bg-mask-100 px-6 py-5">
      <div className="mx-auto flex max-w-3xl flex-col gap-4">
        <div className="flex items-start justify-between gap-3">
          <div className="min-w-0">
            <h1 className="text-lg font-semibold">{t("stats.title")}</h1>
            <p className="mt-0.5 text-xs text-base-content/50">{t("stats.subtitle")}</p>
          </div>
          <button
            type="button"
            className="btn btn-ghost btn-xs shrink-0"
            onClick={() => void load()}
            disabled={busy}
            title={t("stats.refresh")}
            aria-label={t("stats.refresh")}
          >
            <IconRefresh size={14} className={busy ? "animate-spin" : ""} aria-hidden />
            {t("stats.refresh")}
          </button>
        </div>

        {/* 范围切换:今日 / 近7日 / 累计;热力图选中某天后追加一个「单日」态。
            单日=今天时高亮「今日」键并省略日期徽标;非今天则显示日期徽标。 */}
        <div role="radiogroup" aria-label={t("stats.range")} className="join self-start">
          {rangeTabs.map(({ r, labelKey }) => {
            const active =
              r.kind === "today"
                ? range.kind === "today" || (range.kind === "day" && range.date === today)
                : range.kind === r.kind;
            return (
              <button
                key={labelKey}
                type="button"
                role="radio"
                aria-checked={active}
                className={`btn btn-xs join-item ${active ? "btn-primary" : ""}`}
                onClick={() => setRange(r)}
              >
                {t(`stats.card.${labelKey}`)}
              </button>
            );
          })}
          {range.kind === "day" && range.date !== today && (
            <span className="join-item btn btn-xs pointer-events-none">{range.date}</span>
          )}
        </div>

        {error ? (
          <div className="rounded-box border border-error/30 bg-error/5 px-4 py-3 text-sm text-error">
            {t("stats.loadFailed", { reason: error })}
          </div>
        ) : stats === null ? (
          <div className="flex items-center justify-center gap-2 rounded-box border border-base-300 bg-base-100 px-4 py-8 text-sm text-base-content/50">
            <span className="loading loading-spinner loading-sm" aria-hidden />
            {t("stats.loading")}
          </div>
        ) : stats.totals.calls === 0 ? (
          <div className="rounded-box border border-base-300 bg-base-100 px-4 py-10 text-center">
            <p className="text-sm font-medium">{t("stats.empty.title")}</p>
            <p className="mt-1 text-xs text-base-content/50">{t("stats.empty.detail")}</p>
          </div>
        ) : (
          <>
            {/* 汇总卡跟随所选范围:单日显示那一天的合计(标签带日期) */}
            <div className="flex gap-3">
              <SumCard
                label={
                  range.kind === "day"
                    ? range.date === today
                      ? t("stats.card.today")
                      : range.date
                    : t(rangeLabelKey(range))
                }
                bucket={sumBucket}
              />
              {range.kind !== "total" && <SumCard label={t("stats.card.total")} bucket={stats.totals} />}
            </div>

            <DailyChart days={stats.days} />

            <UsageHeatmap
              days={stats.days}
              selectedDate={range.kind === "day" ? range.date : null}
              onPickDay={(date) =>
                setRange(date ? { kind: "day", date } : { kind: "today" })
              }
            />

            <section className="rounded-box border border-base-300 bg-base-100">
              <h2 className="px-4 pt-3 text-sm font-medium">
                {t("stats.byModel")}
                {range.kind !== "total" && (
                  <span className="ms-2 text-xs font-normal text-base-content/40">{t("stats.rangeScopeHint")}</span>
                )}
              </h2>
              <div className="pt-1">
                <ModelTable models={rangeModels} />
              </div>
            </section>

            <section className="rounded-box border border-base-300 bg-base-100">
              <h2 className="px-4 pt-3 text-sm font-medium">{t("stats.bySession")}</h2>
              <div className="pt-1">
                <SessionTable sessions={rangeSessions} />
              </div>
            </section>
          </>
        )}
      </div>
    </div>
  );
}
