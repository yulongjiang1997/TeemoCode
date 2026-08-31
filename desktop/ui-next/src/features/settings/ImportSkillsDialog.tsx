// Git 技能库导入弹窗:支持三种来源(官方市场/历史/自定义),克隆扫描
// SKILL.md → 勾选导入;若仓库根含 mcp.json 则同步展示 MCP 服务器一键安装。
import { IconCheck, IconX } from "@tabler/icons-react";
import { useEffect, useState } from "react";

import { useI18n } from "@/lib/i18n";
import { getConfig, type HostModel } from "@/lib/ipc/config";
import { mcpServersInstall } from "@/lib/ipc/mcpinstall";
import { skillAnalyze, skillsImportGit, skillsSave, type GitSkillRaw } from "@/lib/ipc/skills";

const OFFICIAL_URL = "https://github.com/chaitin/MonkeyCodeOfficialPlugins.git";
const HISTORY_KEY = "mc.market.sources";
const MAX_HISTORY = 10;

function loadHistory(): string[] {
  try { return JSON.parse(localStorage.getItem(HISTORY_KEY) ?? "[]"); } catch { return []; }
}
function saveHistory(urls: string[]) {
  try { localStorage.setItem(HISTORY_KEY, JSON.stringify(urls.slice(0, MAX_HISTORY))); } catch {}
}
function errText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

interface AnalyzedSkill {
  raw: GitSkillRaw;
  summary: { summary: string; details: string; keywords: string[]; scenarios: string[] } | null;
  analyzeError?: string;
}
function extractJson(text: string): AnalyzedSkill["summary"] | null {
  let t = text.trim();
  const fence = t.match(/```(?:json)?\s*([\s\S]*?)```/);
  if (fence?.[1]) t = fence[1].trim();
  const start = t.indexOf("{");
  const end = t.lastIndexOf("}");
  if (start < 0 || end <= start) return null;
  try {
    const obj = JSON.parse(t.slice(start, end + 1));
    if (typeof obj.summary !== "string") return null;
    return {
      summary: obj.summary, details: typeof obj.details === "string" ? obj.details : "",
      keywords: Array.isArray(obj.keywords) ? obj.keywords.map(String) : [],
      scenarios: Array.isArray(obj.scenarios) ? obj.scenarios.map(String) : [],
    };
  } catch { return null; }
}

interface Props {
  onClose: () => void;
  onImported: (names: string[]) => void;
  existingNames: string[];
}

export function ImportSkillsDialog({ onClose, onImported, existingNames }: Props) {
  const { t } = useI18n();
  const [phase, setPhase] = useState<"input" | "fetching" | "analyzing" | "list">("input");
  const [sourceTab, setSourceTab] = useState<"official" | "history" | "custom">("official");
  const [gitUrl, setGitUrl] = useState(OFFICIAL_URL);
  const [error, setError] = useState<string | null>(null);
  const [mcp, setMcp] = useState<Record<string, Record<string, unknown>> | null>(null);
  const [skills, setSkills] = useState<AnalyzedSkill[]>([]);
  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [models, setModels] = useState<HostModel[]>([]);

  useEffect(() => {
    if (typeof window === "undefined" || !(window as { __TAURI__?: unknown }).__TAURI__) return;
    getConfig().then((cfg) => setModels(cfg?.models ?? [])).catch(() => {});
  }, []);

  const isOfficialUrl = gitUrl.trim() === OFFICIAL_URL;

  const startImport = () => {
    const url = gitUrl.trim();
    if (!url) return;
    setError(null);
    setPhase("fetching");
    skillsImportGit(url)
      .then((result) => {
        const analyzed: AnalyzedSkill[] = result.skills.map((raw) => ({ raw, summary: null }));
        setSkills(analyzed);
        setSelected(new Set(analyzed.map((_, i) => i)));
        setMcp(result.mcp ?? null);
        const hist = loadHistory().filter((h) => h !== url);
        saveHistory([url, ...hist]);
        if (analyzed.length > 0) { setPhase("analyzing"); void runAnalysis(analyzed); }
        else setPhase("list");
      })
      .catch((e: unknown) => { setError(errText(e)); setPhase("input"); });
  };

  const runAnalysis = async (list: AnalyzedSkill[]) => {
    let updated = [...list];
    for (let i = 0; i < updated.length; i++) {
      const s = updated[i]!;
      try {
        const txt = await skillAnalyze({
          provider: models[0]?.provider ?? "openai",
          baseUrl: models[0]?.base_url ?? "",
          apiKey: models[0]?.api_key ?? "",
          model: models[0]?.model ?? "",
          content: s.raw.content,
        });
        updated[i] = { ...s, summary: extractJson(txt) ?? null };
      } catch (e: unknown) { updated[i] = { ...s, analyzeError: errText(e) }; }
      setSkills([...updated]);
    }
    setPhase("list");
  };

  const toggle = (i: number) => setSelected((prev) => { const n = new Set(prev); n.has(i) ? n.delete(i) : n.add(i); return n; });

  const doImport = () => {
    setPhase("fetching");
    const importedNames: string[] = [];
    const errors: string[] = [];
    void [...selected].reduce<Promise<void>>(
      (p, i) => p.then(() => {
        const s = skills[i];
        if (!s) return;
        return skillsSave(s.raw.dir_name, s.raw.content)
          .then((info) => { importedNames.push(info.name); })
          .catch((e: unknown) => { errors.push(`${s.raw.name}: ${errText(e)}`); });
      }),
      Promise.resolve(),
    ).then(() => { if (errors.length > 0) setError(errors.join("; ")); onImported(importedNames); onClose(); });
  };

  const installMcp = (name: string, cfg: Record<string, unknown>) => {
    setError(null);
    void mcpServersInstall({ [name]: cfg })
      .then((msg) => alert(msg))
      .catch((e: unknown) => setError(errText(e)));
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40" onClick={phase === "fetching" || phase === "analyzing" ? undefined : onClose}>
      <div className="flex max-h-[80vh] w-[720px] max-w-[95vw] flex-col rounded-box border border-base-300 bg-base-100 shadow-xl" onClick={(e) => e.stopPropagation()}>
        <div className="flex items-center justify-between border-b border-base-300 px-4 py-3">
          <h2 className="text-sm font-semibold">{t("settings.skills.import.title")}</h2>
          <button type="button" className="btn btn-ghost btn-square btn-xs" onClick={onClose} disabled={phase === "fetching" || phase === "analyzing"}><IconX size={14} aria-hidden /></button>
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto p-4">
          {phase === "input" && (
            <div className="flex flex-col gap-2">
              <div className="flex gap-1">
                {(["official", "history", "custom"] as const).map((tab) => (
                  <button key={tab} type="button"
                    className={`badge badge-sm cursor-pointer ${sourceTab === tab ? "badge-primary" : "badge-outline"}`}
                    onClick={() => { setSourceTab(tab); if (tab === "official") setGitUrl(OFFICIAL_URL); }}
                  >{t(`settings.skills.import.tab.${tab}`)}</button>
                ))}
              </div>
              {sourceTab === "history" ? (
                <div className="flex flex-col gap-1">
                  {loadHistory().length === 0 && <span className="text-xs text-base-content/40">{t("settings.skills.import.historyEmpty")}</span>}
                  {loadHistory().map((u) => (
                    <button key={u} type="button" className="rounded-box border border-base-300 px-3 py-1.5 text-left text-[11px] hover:bg-base-200" onClick={() => { setGitUrl(u); setSourceTab("custom"); }}><span className="break-all font-mono">{u}</span></button>
                  ))}
                </div>
              ) : (
                <input className={`input input-sm w-full ${isOfficialUrl && sourceTab === "official" ? "" : "font-mono text-xs"}`}
                  placeholder={t("settings.skills.import.placeholder")}
                  value={gitUrl}
                  readOnly={isOfficialUrl && sourceTab === "official"}
                  onChange={(e) => setGitUrl(e.target.value)}
                />
              )}
              {error && <p className="text-xs text-error">{error}</p>}
              <button type="button" className="btn btn-primary btn-sm w-fit" disabled={!gitUrl.trim()} onClick={() => void startImport()}>{t("settings.skills.import.fetch")}</button>
            </div>
          )}
          {phase === "fetching" && <p className="text-xs text-base-content/60">{t("settings.skills.import.fetching")}</p>}
          {phase === "analyzing" && (
            <div className="flex flex-col gap-1">
              <p className="text-xs text-base-content/60">{t("settings.skills.import.analyzing")}</p>
              <ul className="flex flex-col gap-1 mt-1">
                {skills.map((s) => (
                  <li key={s.raw.dir_name} className="flex items-center gap-2 text-xs">
                    <span className="truncate">{s.raw.name}</span>
                    {s.summary ? <IconCheck size={12} className="text-success" /> : s.analyzeError ? <span className="text-warning text-[10px]">{s.analyzeError}</span> : null}
                  </li>
                ))}
              </ul>
            </div>
          )}
          {phase === "list" && (
            <div className="flex flex-col gap-3">
              <div className="flex items-center gap-2">
                <span className="text-xs font-semibold">{t("settings.skills.import.found", { count: skills.length })}</span>
                <span className="text-[10px] text-base-content/40">{selected.size}/{skills.length} selected</span>
                <span className="flex-1" />
                <button type="button" className="text-xs text-base-content/50" onClick={() => setSelected(new Set(skills.map((_, i) => i)))}>{t("settings.skills.import.selectAll")}</button>
                <button type="button" className="text-xs text-base-content/50" onClick={() => setSelected(new Set())}>{t("settings.skills.import.selectNone")}</button>
              </div>
              <ul className="flex flex-col gap-1.5">
                {skills.map((s, i) => (
                  <li key={s.raw.dir_name} className="flex flex-col gap-1 rounded-box border border-base-300 bg-base-200/40 p-3">
                    <div className="flex items-center gap-2">
                      <input type="checkbox" className="checkbox checkbox-sm" checked={selected.has(i)} onChange={() => toggle(i)} />
                      <span className="min-w-0 truncate text-xs font-semibold">{s.raw.name}</span>
                      {existingNames.includes(s.raw.name) && <span className="badge badge-warning badge-xs shrink-0">{t("settings.skills.import.overrides")}</span>}
                      <span className="truncate text-[10px] text-base-content/50">{s.raw.rel_path}</span>
                    </div>
                    {s.summary && <div className="mt-1 flex flex-wrap gap-1 text-[10px] text-base-content/50"><span>{s.summary.summary}</span>{s.summary.keywords.slice(0, 4).map((k) => <span key={k} className="badge badge-ghost badge-xs">{k}</span>)}</div>}
                  </li>
                ))}
              </ul>
              {mcp && Object.keys(mcp).length > 0 && (
                <div className="rounded-box border border-info/30 bg-info/5 p-3">
                  <p className="text-xs font-semibold text-info">{t("settings.skills.import.mcp.title")}</p>
                  <p className="text-[10px] text-base-content/50">{t("settings.skills.import.mcp.hint")}</p>
                  <ul className="flex flex-col gap-1 mt-1">
                    {Object.entries(mcp).map(([name, cfg]) => (
                      <li key={name} className="flex items-center gap-2 text-xs">
                        <span className="font-mono">{name}</span>
                        <button type="button" className="btn btn-info btn-xs" onClick={() => installMcp(name, cfg as Record<string, unknown>)}>{t("settings.skills.import.mcp.install")}</button>
                      </li>
                    ))}
                  </ul>
                </div>
              )}
              <div className="flex gap-2">
                <button type="button" className="btn btn-primary btn-sm" disabled={selected.size === 0} onClick={doImport}>{t("settings.skills.import.import")}</button>
                <button type="button" className="btn btn-ghost btn-sm" onClick={() => { setPhase("input"); setMcp(null); }}>{t("automation.cancel")}</button>
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
