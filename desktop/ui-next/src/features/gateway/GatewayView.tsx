// 本地大模型网关空间主视图(2026-09-12:从设置页独立到左侧 rail)。
//
// 网关原先挂在设置页第 N 个分区(settings.nav.gateway),用户要求独立成
// 左侧菜单栏空间并加 token 统计面板。本视图把两件事组合成两个 tab:
//   - 模型组:Gw配置本体(GatewaySection,网关命令面 gateway_*,改了即生效)
//   - 调用统计:GatewayStatsPanel(按模型/范围/热力图/调用总时长)
//
// tab 状态不持久化:与 stats 空间的 usage/work 切换同口径(每次进来默认
// 落在第一个 tab)。垂直滚动与 stats 空间一致(scrollbar-gutter:stable
// 防滚动条出现时布局跳动)。
import { useState } from "react";

import { useI18n } from "@/lib/i18n";
import { GatewaySection } from "./GatewaySection";
import { GatewayStatsPanel } from "./GatewayStatsPanel";

type GatewayTab = "groups" | "stats";

export function GatewayView() {
  const { t } = useI18n();
  const [tab, setTab] = useState<GatewayTab>("groups");

  return (
    <div className="min-h-0 flex-1 overflow-x-hidden overflow-y-auto [scrollbar-gutter:stable] bg-mask-100">
      <div className="flex flex-col">
        <div className="flex gap-1 border-b border-base-300 px-6 pt-4">
          <button
            type="button"
            className={`btn btn-ghost btn-sm ${tab === "groups" ? "text-primary" : ""}`}
            onClick={() => setTab("groups")}
          >
            {t("settings.gateway.groups.title")}
          </button>
          <button
            type="button"
            className={`btn btn-ghost btn-sm ${tab === "stats" ? "text-primary" : ""}`}
            onClick={() => setTab("stats")}
          >
            {t("gateway.stats.title")}
          </button>
        </div>
        <div className="px-6 py-5">
          <h1 className="sr-only">{t("gateway.view.title")}</h1>
          {tab === "groups" ? <GatewaySection /> : <GatewayStatsPanel />}
        </div>
      </div>
    </div>
  );
}
