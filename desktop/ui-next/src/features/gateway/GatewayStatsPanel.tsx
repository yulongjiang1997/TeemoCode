// 本地大模型网关调用统计面板(2026-09-12 需求)。
//
// 数据来自壳侧 gateway_log_stats 命令(按天 × 模型聚合落盘的 GatewayLogStore),
// 与 UsageStatsView 同理念:挂载时取一次 + 手动刷新,不常驻轮询。
//
// 布局:头部范围切换(今天 / 近7天 / 累计)→ 汇总卡(总 Token / 调用次数 /
// 调用总时长,精确到秒)→ 按模型分类表(跟随范围联动)→ 按天热力图(全量留存)。
// token 数值用 K/M 缩写(与 UsageStatsView 同款 fmtCompact),完整值进 tooltip。
// 调用总时长由后端按 LogEntry.latency_ms 累加(total_duration_ms),前端换算秒展示。
import { IconRefresh } from "@tabler/icons-react";
import { useCallback, useEffect, useState } from "react";

import { useI18n } from "@/lib/i18n";
import { gatewayLogStats, type GatewayLogStats, type GatewayModelStats, type GatewayRangeKind } from "@/lib/ipc/gateway";
import { fmtCompact, fmtFull } from "@/features/stats/UsageStatsView";

const errMsg = (e: unknown): string => (e instanceof Error ? e.message : String(e));

/** 毫秒 → 秒(精确到秒,小数 1 位):1.5s / 42s / 3m 12s / 1h 2m 3s */
export const fmtDuration = (ms: number): string => {
  if (!Number.isFinite(ms) || ms <= 0) return "0s";
  const totalSec = Math.round(ms / 1000);
  if (totalSec < 60) return `${totalSec}s`;
  if (totalSec < 3600) return `${Math.floor(totalSec / 60)}m ${totalSec % 60}s`;
  const h = Math.floor(totalSec / 3600);
  const m = Math.floor((totalSec % 3600) / 60);
  const s = totalSec % 60;
  return `${h}h ${m}m ${s}s`;
};

const RANGES: { kind: GatewayRangeKind; key: "today" | "day7" | "all" }[] = [
  { kind: "today", key: "today" },
  { kind: "day7", key: "day7" },
  { kind: "all", key: "all" },
];

/** 热力图色阶:与 UsageStatsView 同款四分位切档(用量分布极度右偏,
 *  线性映射会让绝大多数天都挤在最浅档)。 */
function GatewayHeatmap({ days }: { days: GatewayLogStats["heatmap"] }) {
  const { t } = useI18n();
  if (days.length === 0) {
    return (
      <section className="rounded-box border border-base-300 bg-base-100">
        <h2 className="px-4 pt-3 text-sm font-medium">{t("gateway.stats.heatmap")}</h2>
        <p className="px-4 pb-4 pt-1 text-xs text-base-content/40">{t("gateway.stats.noData")}</p>
      </section>
    );
  }

  const byDate = new Map<string, { total_tokens: number; calls: number }>();
  for (const d of days) byDate.set(d.date, d);

  const today = new Date();
  const start = new Date(today);
  start.setDate(start.getDate() - 52 * 7); // 一年窗口
  start.setDate(start.getDate() - start.getDay()); // 对齐到周日

  const weeks: { date: Date; data: { total_tokens: number; calls: number } | null }[][] = [];
  const cur = new Date(start);
  while (cur <= today) {
    const week: { date: Date; data: { total_tokens: number; calls: number } | null }[] = [];
    for (let i = 0; i < 7; i++) {
      const d = new Date(cur);
      const key = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
      week.push({ date: d, data: byDate.get(key) ?? null });
      cur.setDate(cur.getDate() + 1);
    }
    weeks.push(week);
  }
  const weeksShown = weeks.length;

  // 有数据的天按 token 总量排序取四分位阈值
  const totals = days.map((d) => d.total_tokens).filter((v) => v > 0).sort((a, b) => a - b);
  const quantile = (q: number): number =>
    totals.length === 0 ? 0 : totals[Math.min(totals.length - 1, Math.floor(q * totals.length))]!;
  const q25 = quantile(0.25);
  const q50 = quantile(0.5);
  const q75 = quantile(0.75);
  const level = (data: { total_tokens: number; calls: number } | null): number => {
    if (!data || data.calls <= 0) return 0;
    const v = data.total_tokens;
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
        {t("gateway.stats.heatmap")}
        <span className="ms-2 text-xs font-normal text-base-content/40">{weeksShown}w</span>
      </h2>
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
                  const data = cell.data;
                  const dateLabel = cell.date.toLocaleDateString("zh-CN", { year: "numeric", month: "short", day: "numeric" });
                  return (
                    <div
                      key={wi}
                      title={
                        data
                          ? `${dateLabel} · ${t("gateway.stats.calls")} ${data.calls.toLocaleString("en-US")} · ${fmtCompact(data.total_tokens)}`
                          : `${dateLabel} · ${t("gateway.stats.calls")} 0`
                      }
                      className={`h-[11px] min-w-0 flex-1 rounded-[2px] ${cellCls[level(data)]}`}
                    />
                  );
                })}
              </div>
            ))}
            <div className="mt-1 flex items-center justify-end gap-1 text-[10px] text-base-content/40">
              <span>{t("gateway.stats.heatmap.less")}</span>
              {cellCls.map((c, i) => (
                <span key={i} className={`h-[11px] w-[11px] rounded-[2px] ${c}`} />
              ))}
              <span>{t("gateway.stats.heatmap.more")}</span>
            </div>
          </div>
        </div>
      </div>
    </section>
  );
}

/** 按模型分类表(K/M 缩写 + tooltip 全量;总时长精确到秒) */
function ModelTable({ models }: { models: GatewayModelStats[] }) {
  const { t } = useI18n();
  if (models.length === 0) return <p className="px-4 py-6 text-center text-sm text-base-content/40">{t("gateway.stats.noData")}</p>;
  return (
    <div className="overflow-x-auto">
      <table className="table table-xs">
        <thead>
          <tr className="text-base-content/50">
            <th>{t("gateway.stats.model")}</th>
            <th className="text-end">{t("gateway.stats.input")}</th>
            <th className="text-end">{t("gateway.stats.output")}</th>
            <th className="text-end">{t("gateway.stats.totalTokens")}</th>
            <th className="text-end">{t("gateway.stats.calls")}</th>
            <th className="text-end">{t("gateway.stats.duration")}</th>
          </tr>
        </thead>
        <tbody>
          {models.map((m) => (
            <tr key={m.model}>
              <td className="font-mono text-xs" title={m.model}>{m.model}</td>
              <td className="text-end tabular-nums" title={fmtFull(m.input_tokens)}>{fmtCompact(m.input_tokens)}</td>
              <td className="text-end tabular-nums" title={fmtFull(m.output_tokens)}>{fmtCompact(m.output_tokens)}</td>
              <td className="text-end tabular-nums" title={fmtFull(m.total_tokens)}>{fmtCompact(m.total_tokens)}</td>
              <td className="text-end tabular-nums">{m.calls.toLocaleString("en-US")}</td>
              <td className="text-end tabular-nums" title={`${fmtFull(m.duration_ms)}ms`}>{fmtDuration(m.duration_ms)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

export function GatewayStatsPanel() {
  const { t } = useI18n();
  const [data, setData] = useState<GatewayLogStats | null>(null);
  const [range, setRange] = useState<GatewayRangeKind>("today");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const fetchStats = useCallback(async (r: GatewayRangeKind) => {
    setLoading(true);
    setError(null);
    try {
      const d = await gatewayLogStats(r);
      setData(d);
    } catch (e) {
      setError(errMsg(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void fetchStats(range);
  }, [range, fetchStats]);

  const totals = data
    ? {
        total_tokens: data.total_tokens,
        input_tokens: data.total_input_tokens,
        output_tokens: data.total_output_tokens,
        calls: data.total_calls,
        duration_ms: data.total_duration_ms,
      }
    : null;

  return (
    <div className="flex flex-col gap-3">
      {/* 头部:范围切换 + 刷新 */}
      <div className="flex items-center gap-2">
        <div className="flex gap-1">
          {RANGES.map((r) => (
            <button
              key={r.kind}
              type="button"
              className={`btn btn-ghost btn-sm ${range === r.kind ? "text-primary" : ""}`}
              onClick={() => setRange(r.kind)}
            >
              {t(`gateway.stats.range.${r.key}` as const)}
            </button>
          ))}
        </div>
        <button
          type="button"
          className="btn btn-ghost btn-sm ms-auto"
          disabled={loading}
          onClick={() => fetchStats(range)}
          title={t("gateway.stats.title")}
        >
          <IconRefresh size={14} stroke={1.75} aria-hidden />
          {loading ? "…" : t("gateway.stats.title")}
        </button>
      </div>

      {error && (
        <div role="alert" className="alert alert-error alert-soft text-xs">{error}</div>
      )}

      {/* 汇总卡 */}
      {totals ? (
        <div className="flex flex-wrap gap-3">
          <div
            className="flex min-w-0 flex-1 flex-col gap-1 rounded-box border border-base-300 bg-base-100 px-4 py-3"
            title={`${fmtFull(totals.total_tokens)} tokens`}
          >
            <span className="text-xs text-base-content/50">{t("gateway.stats.totalTokens")}</span>
            <span className="text-2xl font-semibold tabular-nums leading-none">{fmtCompact(totals.total_tokens)}</span>
            <span
              className="text-xs tabular-nums text-base-content/50"
              title={`↑ ${fmtFull(totals.input_tokens)} · ↓ ${fmtFull(totals.output_tokens)}`}
            >
              ↑{fmtCompact(totals.input_tokens)} · ↓{fmtCompact(totals.output_tokens)}
            </span>
          </div>
          <div className="flex min-w-0 flex-1 flex-col gap-1 rounded-box border border-base-300 bg-base-100 px-4 py-3">
            <span className="text-xs text-base-content/50">{t("gateway.stats.calls")}</span>
            <span className="text-2xl font-semibold tabular-nums leading-none">{totals.calls.toLocaleString("en-US")}</span>
            <span className="text-xs text-base-content/50">{t("gateway.stats.desc")}</span>
          </div>
          <div
            className="flex min-w-0 flex-1 flex-col gap-1 rounded-box border border-base-300 bg-base-100 px-4 py-3"
            title={`${fmtFull(totals.duration_ms)}ms`}
          >
            <span className="text-xs text-base-content/50">{t("gateway.stats.duration")}</span>
            <span className="text-2xl font-semibold tabular-nums leading-none">{fmtDuration(totals.duration_ms)}</span>
            <span className="text-xs text-base-content/50">{t(`gateway.stats.range.${range}` as const)}</span>
          </div>
        </div>
      ) : !error ? (
        <div className="flex items-center gap-2 py-4 text-sm text-base-content/40">
          <span className="loading loading-spinner loading-sm" />
          {t("gateway.stats.title")}
        </div>
      ) : null}

      {/* 按模型分类表 */}
      <section className="rounded-box border border-base-300 bg-base-100">
        <h2 className="px-4 pt-3 text-sm font-medium">{t("gateway.stats.byModel")}</h2>
        <div className="px-2 pb-3 pt-1">
          {data ? <ModelTable models={data.models} /> : null}
        </div>
      </section>

      {/* 热力图 */}
      {data && <GatewayHeatmap days={data.heatmap} />}
    </div>
  );
}
