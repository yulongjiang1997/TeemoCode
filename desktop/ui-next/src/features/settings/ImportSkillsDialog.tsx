// Git 技能库导入弹窗:输入 git 地址 → 克隆扫描 SKILL.md → 可选大模型解析
// 摘要 → 列表展示(勾选) → 导入为本地自定义技能。
// 大模型解析是增强项:没配模型也能导入(用 frontmatter 的原始信息)。
import { IconCheck, IconX } from "@tabler/icons-react";
import { useEffect, useState } from "react";

import { useI18n } from "@/lib/i18n";
import { getConfig, type HostModel } from "@/lib/ipc/config";
import { skillAnalyze, skillsImportGit, skillsSave, type GitSkillRaw } from "@/lib/ipc/skills";

/** 解析后的技能条目(原始信息 + 可选的模型摘要) */
interface AnalyzedSkill {
  raw: GitSkillRaw;
  /** 模型解析出的结构化摘要;null = 未解析或解析失败 */
  summary: {
    summary: string;
    details: string;
    keywords: string[];
    scenarios: string[];
  } | null;
  /** 解析失败原因(仅展示用,不阻断导入) */
  analyzeError?: string;
}

/** 从模型输出文本中提取 JSON(容错:剥 markdown 代码块 / 截取首尾花括号) */
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
      summary: obj.summary,
      details: typeof obj.details === "string" ? obj.details : "",
      keywords: Array.isArray(obj.keywords) ? obj.keywords.map(String) : [],
      scenarios: Array.isArray(obj.scenarios) ? obj.scenarios.map(String) : [],
    };
  } catch {
    return null;
  }
}

interface ImportSkillsDialogProps {
  onClose: () => void;
  /** 导入完成(已落盘)回调:父组件刷新技能列表 */
  onImported: (names: string[]) => void;
  /** 已存在的技能名(导入同名会覆盖,列表里提示) */
  existingNames: string[];
}

export function ImportSkillsDialog({ onClose, onImported, existingNames }: ImportSkillsDialogProps) {
  const { t } = useI18n();
  // 步骤:input(填地址) → fetching(克隆) → analyzing(解析) → list(展示/选择)
  const [phase, setPhase] = useState<"input" | "fetching" | "analyzing" | "list">("input");
  const [gitUrl, setGitUrl] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [items, setItems] = useState<AnalyzedSkill[]>([]);
  const [checked, setChecked] = useState<Set<string>>(new Set());
  const [detail, setDetail] = useState<AnalyzedSkill | null>(null);
  const [importing, setImporting] = useState(false);
  const [importDone, setImportDone] = useState<string[] | null>(null);

  // 模型选择(取配置里的模型列表)
  const [models, setModels] = useState<HostModel[]>([]);
  const [modelIdx, setModelIdx] = useState(0);
  const [useModel, setUseModel] = useState(true);

  useEffect(() => {
    getConfig()
      .then((cfg) => {
        const ms = cfg?.models ?? [];
        setModels(ms);
        const defIdx = ms.findIndex((m) => m.default);
        setModelIdx(defIdx >= 0 ? defIdx : 0);
      })
      .catch(() => {});
  }, []);

  const curModel: HostModel | undefined = models[modelIdx];

  const startImport = async () => {
    const url = gitUrl.trim();
    if (!url) return;
    setError(null);
    setPhase("fetching");
    try {
      const r = await skillsImportGit(url);
      const analyzed: AnalyzedSkill[] = r.skills.map((raw) => ({ raw, summary: null }));
      setItems(analyzed);
      if (useModel && curModel) {
        setPhase("analyzing");
        // 逐个解析(串行避免网关限流);失败不阻断,回退原始 description
        for (const item of analyzed) {
          try {
            const out = await skillAnalyze({
              provider: curModel.provider,
              baseUrl: curModel.base_url,
              apiKey: curModel.api_key,
              model: curModel.model,
              content: item.raw.content.slice(0, 8000), // 截断防爆 token
            });
            item.summary = extractJson(out);
            if (!item.summary) item.analyzeError = "模型输出无法解析为 JSON";
          } catch (e) {
            item.analyzeError = e instanceof Error ? e.message : String(e);
          }
          setItems([...analyzed]);
        }
      }
      setPhase("list");
      // 默认全选未冲突的
      setChecked(new Set(r.skills.filter((s) => !existingNames.includes(s.dir_name)).map((s) => s.dir_name)));
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setPhase("input");
    }
  };

  const doImport = async () => {
    const chosen = items.filter((i) => checked.has(i.raw.dir_name));
    if (chosen.length === 0) return;
    setImporting(true);
    setError(null);
    const okNames: string[] = [];
    for (const item of chosen) {
      try {
        await skillsSave(item.raw.dir_name, item.raw.content);
        okNames.push(item.raw.dir_name);
      } catch (e) {
        setError(`${item.raw.dir_name}: ${e instanceof Error ? e.message : String(e)}`);
      }
    }
    setImporting(false);
    if (okNames.length > 0) {
      setImportDone(okNames);
      onImported(okNames);
    }
  };

  const toggle = (name: string) =>
    setChecked((prev) => {
      const next = new Set(prev);
      if (next.has(name)) next.delete(name);
      else next.add(name);
      return next;
    });

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40" onClick={phase === "fetching" || phase === "analyzing" ? undefined : onClose}>
      <div
        className="flex max-h-[80vh] w-[640px] max-w-[95vw] flex-col rounded-box border border-base-300 bg-base-100 shadow-xl"
        onClick={(e) => e.stopPropagation()}
      >
        {/* 头部 */}
        <div className="flex items-center justify-between border-b border-base-300 px-4 py-3">
          <h2 className="text-sm font-semibold">{t("settings.skills.import.title")}</h2>
          <button type="button" className="btn btn-ghost btn-square btn-xs" onClick={onClose} disabled={phase === "fetching" || phase === "analyzing"}>
            <IconX size={14} aria-hidden />
          </button>
        </div>

        <div className="min-h-0 flex-1 overflow-y-auto p-4">
          {phase === "input" && (
            <div className="flex flex-col gap-3">
              <fieldset className="fieldset gap-1.5">
                <legend className="fieldset-legend">{t("settings.skills.import.gitUrl")}</legend>
                <input
                  autoFocus
                  className="input input-sm w-full font-mono text-xs"
                  placeholder="https://github.com/user/skills-repo.git"
                  value={gitUrl}
                  onChange={(e) => setGitUrl(e.target.value)}
                  onKeyDown={(e) => { if (e.key === "Enter") void startImport(); }}
                />
                <p className="text-2xs text-base-content/50">{t("settings.skills.import.gitHint")}</p>
              </fieldset>
              <fieldset className="fieldset gap-1.5">
                <legend className="fieldset-legend">{t("settings.skills.import.model")}</legend>
                <div className="flex items-center gap-2">
                  <input
                    type="checkbox"
                    className="toggle toggle-sm"
                    checked={useModel}
                    onChange={(e) => setUseModel(e.target.checked)}
                    disabled={models.length === 0}
                  />
                  <select
                    className="select select-sm w-full text-xs"
                    value={modelIdx}
                    onChange={(e) => setModelIdx(Number(e.target.value))}
                    disabled={!useModel || models.length === 0}
                  >
                    {models.length === 0 && <option value={0}>{t("settings.skills.import.noModel")}</option>}
                    {models.map((m, i) => (
                      <option key={i} value={i}>{m.name} ({m.model})</option>
                    ))}
                  </select>
                </div>
                <p className="text-2xs text-base-content/50">{t("settings.skills.import.modelHint")}</p>
              </fieldset>
              {error && (
                <div role="alert" className="alert alert-error alert-soft text-xs">{error}</div>
              )}
              <button type="button" className="btn btn-primary btn-sm w-fit" disabled={!gitUrl.trim()} onClick={() => void startImport()}>
                {t("settings.skills.import.start")}
              </button>
            </div>
          )}

          {(phase === "fetching" || phase === "analyzing") && (
            <div className="flex flex-col items-center gap-3 py-10">
              <span className="loading loading-spinner loading-md text-primary" />
              <p className="text-xs text-base-content/60">
                {phase === "fetching" ? t("settings.skills.import.fetching") : t("settings.skills.import.analyzing")}
              </p>
            </div>
          )}

          {phase === "list" && importDone === null && (
            <div className="flex flex-col gap-2">
              <p className="text-2xs text-base-content/50">
                {t("settings.skills.import.found", { n: items.length, sel: checked.size })}
              </p>
              {error && <div role="alert" className="alert alert-error alert-soft text-xs">{error}</div>}
              <ul className="flex flex-col gap-1.5">
                {items.map((item) => {
                  const isDup = existingNames.includes(item.raw.dir_name);
                  const desc = item.summary?.summary || item.raw.description;
                  return (
                    <li key={item.raw.dir_name} className="rounded-box border border-base-300 bg-base-200/30 px-3 py-2">
                      <div className="flex items-start gap-2">
                        <input
                          type="checkbox"
                          className="checkbox checkbox-sm checkbox-primary mt-0.5 shrink-0"
                          checked={checked.has(item.raw.dir_name)}
                          onChange={() => toggle(item.raw.dir_name)}
                        />
                        <div className="min-w-0 flex-1">
                          <div className="flex items-center gap-1.5">
                            <span className="truncate font-mono text-xs font-semibold">{item.raw.dir_name}</span>
                            {isDup && <span className="badge badge-warning badge-soft badge-xs shrink-0">{t("settings.skills.import.dupBadge")}</span>}
                          </div>
                          <p className="mt-0.5 line-clamp-2 text-xs text-base-content/60">{desc}</p>
                          {item.summary?.keywords && item.summary.keywords.length > 0 && (
                            <div className="mt-1 flex flex-wrap gap-1">
                              {item.summary.keywords.slice(0, 5).map((k) => (
                                <span key={k} className="badge badge-ghost badge-xs">{k}</span>
                              ))}
                            </div>
                          )}
                          {item.analyzeError && (
                            <p className="mt-0.5 text-2xs text-warning">{t("settings.skills.import.analyzeFailed")}: {item.analyzeError}</p>
                          )}
                        </div>
                        <button type="button" className="btn btn-ghost btn-xs shrink-0" onClick={() => setDetail(item)}>
                          {t("settings.skills.import.detail")}
                        </button>
                      </div>
                    </li>
                  );
                })}
              </ul>
            </div>
          )}

          {importDone !== null && (
            <div className="flex flex-col items-center gap-3 py-10">
              <IconCheck size={40} className="text-success" stroke={1.5} aria-hidden />
              <p className="text-xs text-base-content/70">{t("settings.skills.import.done", { n: importDone.length })}</p>
            </div>
          )}
        </div>

        {/* 底部操作 */}
        {phase === "list" && importDone === null && (
          <div className="flex items-center justify-end gap-2 border-t border-base-300 px-4 py-3">
            <button type="button" className="btn btn-ghost btn-sm" onClick={onClose}>
              {t("settings.skills.import.close")}
            </button>
            <button
              type="button"
              className="btn btn-primary btn-sm"
              disabled={checked.size === 0 || importing}
              onClick={() => void doImport()}
            >
              {importing ? <span className="loading loading-spinner loading-xs" /> : null}
              {t("settings.skills.import.importN", { n: checked.size })}
            </button>
          </div>
        )}
        {importDone !== null && (
          <div className="flex items-center justify-end border-t border-base-300 px-4 py-3">
            <button type="button" className="btn btn-primary btn-sm" onClick={onClose}>
              {t("settings.skills.import.close")}
            </button>
          </div>
        )}
      </div>

      {/* 详情浮层 */}
      {detail && (
        <div className="fixed inset-0 z-[60] flex items-center justify-center bg-black/50" onClick={() => setDetail(null)}>
          <div
            className="flex max-h-[70vh] w-[560px] max-w-[95vw] flex-col rounded-box border border-base-300 bg-base-100 shadow-xl"
            onClick={(e) => e.stopPropagation()}
          >
            <div className="flex items-center justify-between border-b border-base-300 px-4 py-3">
              <h3 className="font-mono text-sm font-semibold">{detail.raw.dir_name}</h3>
              <button type="button" className="btn btn-ghost btn-square btn-xs" onClick={() => setDetail(null)}>
                <IconX size={14} aria-hidden />
              </button>
            </div>
            <div className="min-h-0 flex-1 overflow-y-auto p-4 text-xs">
              <p className="mb-1 text-base-content/50">{detail.raw.rel_path}</p>
              {detail.summary && (
                <div className="mb-3 flex flex-col gap-2 rounded-box bg-base-200/50 p-3">
                  <p><span className="font-semibold">{t("settings.skills.import.fSummary")}</span> {detail.summary.summary}</p>
                  {detail.summary.details && <p><span className="font-semibold">{t("settings.skills.import.fDetails")}</span> {detail.summary.details}</p>}
                  {detail.summary.scenarios.length > 0 && (
                    <p><span className="font-semibold">{t("settings.skills.import.fScenarios")}</span> {detail.summary.scenarios.join("、")}</p>
                  )}
                </div>
              )}
              <pre className="overflow-auto rounded-box bg-base-200/60 p-3 font-mono text-xs whitespace-pre-wrap">{detail.raw.content}</pre>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
