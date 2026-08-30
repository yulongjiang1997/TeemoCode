// 子代理团队:统一模型,按技能(角色)分派。团队成员提前配置(角色名 +
// 职责文本 + 技能库多选下拉),任务下发时主模型(协调者)拆解并分派给
// 合适成员,执行时职责与指定技能都生效。
// v2 增强(参考 CrewAI/MetaGPT/AutoGen 的开源编排模式,见
// docs/superpowers/specs/2026-08-30-team-v2-design.md):
//   - 工作流(SOP)步骤编辑:阶段 + 参与角色,按序推进;
//   - 内置团队预设一键应用(覆盖确认);
//   - 导出(剪贴板)/导入(粘贴),零新 IPC;
//   - 成员数上限提示(>8 的部分不进编排指令,见 teamPreamble TEAM_MAX_ROLES)。
import { IconArrowDown, IconArrowUp, IconCopy, IconDownload, IconEdit, IconPlus, IconTrash, IconUpload } from "@tabler/icons-react";
import { useEffect, useState } from "react";

import { useI18n } from "@/lib/i18n";
import { skillsList, type SkillInfo } from "@/lib/ipc/skills";
import { TEAM_MAX_ROLES } from "@/lib/ipc/teamPreamble";
import {
  readTeamRoles,
  readTeamWorkflow,
  writeTeamRoles,
  writeTeamWorkflow,
  type TeamRole,
  type TeamWorkflowStep,
} from "@/lib/util/prefs";
import { TEAM_PRESETS, instantiatePreset } from "@/lib/util/teamPresets";
import { copyText } from "@/lib/util/clipboard";

export function TeamSection() {
  const { t } = useI18n();
  const [roles, setRoles] = useState<TeamRole[]>(readTeamRoles);
  const [workflow, setWorkflow] = useState<TeamWorkflowStep[]>(readTeamWorkflow);
  const [skills, setSkills] = useState<SkillInfo[]>([]);
  // 编辑中角色 id(展开该卡为编辑表单)
  const [editingId, setEditingId] = useState<string | null>(null);
  // 编辑/新增的草稿
  const [name, setName] = useState("");
  const [skill, setSkill] = useState("");
  const [picked, setPicked] = useState<string[]>([]);
  // 导入面板
  const [importOpen, setImportOpen] = useState(false);
  const [importText, setImportText] = useState("");
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void skillsList().then(setSkills).catch(() => {});
  }, []);

  const commit = (nextRoles: TeamRole[], nextWorkflow?: TeamWorkflowStep[]) => {
    setRoles(nextRoles);
    writeTeamRoles(nextRoles);
    if (nextWorkflow) {
      setWorkflow(nextWorkflow);
      writeTeamWorkflow(nextWorkflow);
    }
  };

  const resetDraft = () => {
    setName("");
    setSkill("");
    setPicked([]);
    setEditingId(null);
  };

  const save = () => {
    const n = name.trim();
    if (!n) return;
    if (editingId) {
      commit(roles.map((r) => (r.id === editingId ? { ...r, name: n, skill: skill.trim(), skills: picked } : r)));
    } else {
      commit([...roles, { id: crypto.randomUUID(), name: n, skill: skill.trim(), skills: picked }]);
    }
    resetDraft();
  };

  const startEdit = (r: TeamRole) => {
    setEditingId(r.id);
    setName(r.name);
    setSkill(r.skill);
    setPicked(r.skills);
  };

  const togglePick = (sn: string) => {
    setPicked((p) => (p.includes(sn) ? p.filter((x) => x !== sn) : [...p, sn]));
  };

  // ---- 工作流编辑 ----
  const updateStep = (id: string, patch: Partial<TeamWorkflowStep>) => {
    setWorkflow((cur) => {
      const next = cur.map((s) => (s.id === id ? { ...s, ...patch } : s));
      writeTeamWorkflow(next);
      return next;
    });
  };
  const moveStep = (idx: number, delta: -1 | 1) => {
    setWorkflow((cur) => {
      const j = idx + delta;
      const a = cur[idx];
      const b = cur[j];
      if (!a || !b) return cur;
      const next = [...cur];
      next[idx] = b;
      next[j] = a;
      writeTeamWorkflow(next);
      return next;
    });
  };
  const removeStep = (id: string) => {
    setWorkflow((cur) => {
      const next = cur.filter((s) => s.id !== id);
      writeTeamWorkflow(next);
      return next;
    });
  };
  const addStep = () => {
    setWorkflow((cur) => {
      const next = [...cur, { id: crypto.randomUUID(), title: "", roleIds: [] }];
      writeTeamWorkflow(next);
      return next;
    });
  };
  const toggleStepRole = (stepId: string, roleId: string) => {
    setWorkflow((cur) => {
      const next = cur.map((s) =>
        s.id === stepId
          ? { ...s, roleIds: s.roleIds.includes(roleId) ? s.roleIds.filter((x) => x !== roleId) : [...s.roleIds, roleId] }
          : s,
      );
      writeTeamWorkflow(next);
      return next;
    });
  };

  // ---- 预设 / 导入导出 ----
  const applyPreset = (presetId: string) => {
    const preset = TEAM_PRESETS.find((p) => p.id === presetId);
    if (!preset) return;
    if ((roles.length > 0 || workflow.length > 0) && !window.confirm(t("settings.team.presets.confirm"))) return;
    const { roles: nextRoles, workflow: nextWorkflow } = instantiatePreset(preset, () => crypto.randomUUID());
    commit(nextRoles, nextWorkflow);
    resetDraft();
    setEditingId(null);
    setNotice(t("settings.team.presets.applied", { name: t(`settings.team.preset.${preset.id}`) }));
  };

  const exportTeam = () => {
    const payload = JSON.stringify({ version: 1, roles, workflow }, null, 2);
    copyText(payload);
    setNotice(t("settings.team.export.ok"));
  };

  const applyImport = () => {
    try {
      const parsed: unknown = JSON.parse(importText);
      const obj = parsed as { roles?: unknown; workflow?: unknown };
      if (!Array.isArray(obj?.roles)) throw new Error("roles 不是数组");
      // 一次遍历产出(旧 id, 新角色)对:角色 id 全部重生成,工作流 roleIds
      // 经旧→新映射重建,悬空引用与无成员步骤丢弃
      const pairs = (obj.roles as Array<Record<string, unknown>>)
        .filter((r): r is Record<string, unknown> => Boolean(r && typeof r === "object" && typeof r.name === "string" && String(r.name).trim()))
        .map((r) => ({
          oldId: typeof r.id === "string" ? r.id : "",
          role: {
            id: crypto.randomUUID(),
            name: String(r.name).slice(0, 64).trim(),
            skill: typeof r.skill === "string" ? r.skill : "",
            skills: Array.isArray(r.skills) ? (r.skills as unknown[]).filter((x): x is string => typeof x === "string") : [],
          } satisfies TeamRole,
        }));
      if (pairs.length === 0) throw new Error("没有有效成员");
      const idMap = new Map(pairs.filter((p) => p.oldId).map((p) => [p.oldId, p.role.id] as const));
      const importedRoles = pairs.map((p) => p.role);
      const importedWorkflow: TeamWorkflowStep[] = Array.isArray(obj.workflow)
        ? (obj.workflow as Array<Record<string, unknown>>)
            .filter((s) => s && typeof s === "object" && typeof s.title === "string" && Array.isArray(s.roleIds))
            .map((s) => ({
              id: crypto.randomUUID(),
              title: String(s.title),
              roleIds: (s.roleIds as string[]).map((old) => idMap.get(old) ?? "").filter(Boolean),
            }))
            .filter((s) => s.roleIds.length > 0)
        : [];
      commit(importedRoles, importedWorkflow);
      setImportText("");
      setImportOpen(false);
      setNotice(t("settings.team.import.ok", { count: importedRoles.length }));
    } catch (e) {
      setError(t("settings.team.import.failed", { reason: e instanceof Error ? e.message : String(e) }));
    }
  };

  const overLimit = roles.length > TEAM_MAX_ROLES;

  const SkillPicker = () => (
    <div className="flex flex-wrap items-center gap-1">
      <span className="text-[10px] text-base-content/40">{t("settings.team.skillsLabel")}:</span>
      {picked.length === 0 && <span className="text-[10px] text-base-content/30">{t("settings.team.skillsNone")}</span>}
      {picked.map((sn) => (
        <button
          key={sn}
          type="button"
          className="badge badge-sm cursor-pointer gap-1 border-primary/60 bg-primary/10 text-primary"
          title={t("settings.team.skillsRemoveTip")}
          onClick={() => togglePick(sn)}
        >
          {sn}×
        </button>
      ))}
      {skills.length > 0 && (
        <select
          className="select select-xs h-6 min-h-0 w-auto max-w-36 text-[10px]"
          value=""
          aria-label={t("settings.team.skillsAdd")}
          onChange={(e) => {
            const v = e.target.value;
            if (v && !picked.includes(v)) togglePick(v);
          }}
        >
          <option value="">{t("settings.team.skillsAdd")}…</option>
          {skills
            .filter((s) => !picked.includes(s.name))
            .map((s) => (
              <option key={s.name} value={s.name}>
                {s.name}
              </option>
            ))}
        </select>
      )}
    </div>
  );

  return (
    <section aria-label={t("settings.nav.team")} className="flex flex-col gap-2">
      <div className="rounded-box border border-base-300">
        <div className="flex flex-col gap-1 border-b border-base-300/70 p-3">
          <div className="text-sm font-semibold">{t("settings.team.title")}</div>
          <p className="text-xs text-base-content/60">{t("settings.team.hint")}</p>
        </div>

        {/* 预设 / 导入导出工具行 */}
        <div className="flex flex-wrap items-center gap-2 border-b border-base-300/70 p-3">
          <select
            className="select select-xs h-7 min-h-0 w-auto max-w-44 text-xs"
            aria-label={t("settings.team.presets.label")}
            value=""
            onChange={(e) => {
              if (e.target.value) applyPreset(e.target.value);
              e.currentTarget.value = "";
            }}
          >
            <option value="">{t("settings.team.presets.label")}…</option>
            {TEAM_PRESETS.map((p) => (
              <option key={p.id} value={p.id}>
                {t(`settings.team.preset.${p.id}`)}
              </option>
            ))}
          </select>
          <button type="button" className="btn btn-ghost btn-xs text-base-content/60" onClick={exportTeam}>
            <IconDownload size={13} stroke={1.75} aria-hidden />
            {t("settings.team.export")}
          </button>
          <button
            type="button"
            className="btn btn-ghost btn-xs text-base-content/60"
            onClick={() => setImportOpen((v) => !v)}
          >
            <IconUpload size={13} stroke={1.75} aria-hidden />
            {t("settings.team.import")}
          </button>
          <span className="flex-1" />
          <span className="text-[10px] text-base-content/35">
            {roles.length}/{TEAM_MAX_ROLES}
          </span>
        </div>
        {overLimit && (
          <p className="border-b border-base-300/70 px-3 py-1.5 text-[11px] text-warning">
            {t("settings.team.rolesOverLimit", { max: TEAM_MAX_ROLES })}
          </p>
        )}
        {notice && (
          <p className="flex items-center gap-1.5 border-b border-base-300/70 px-3 py-1.5 text-[11px] text-success">
            <IconCopy size={12} stroke={1.75} aria-hidden />
            {notice}
          </p>
        )}
        {error && (
          <p role="alert" className="border-b border-base-300/70 px-3 py-1.5 text-[11px] text-error">
            {error}
          </p>
        )}
        {importOpen && (
          <div className="flex flex-col gap-1.5 border-b border-base-300/70 p-3">
            <textarea
              className="textarea textarea-xs min-h-24 w-full font-mono text-[11px]"
              placeholder={t("settings.team.import.placeholder")}
              value={importText}
              onChange={(e) => setImportText(e.target.value)}
            />
            <div className="flex gap-1">
              <button type="button" className="btn btn-primary btn-xs" disabled={!importText.trim()} onClick={applyImport}>
                {t("settings.team.import.apply")}
              </button>
              <button
                type="button"
                className="btn btn-ghost btn-xs"
                onClick={() => {
                  setImportOpen(false);
                  setImportText("");
                }}
              >
                {t("settings.team.cancel")}
              </button>
            </div>
          </div>
        )}

        <ul className="flex flex-col gap-2 p-3">
          {roles.length === 0 && <li className="text-xs text-base-content/40">{t("settings.team.empty")}</li>}
          {roles.map((r) =>
            editingId === r.id ? (
              // 编辑态:名称 + 职责 + 技能下拉
              <li key={r.id} className="flex flex-col gap-1.5 rounded-box border border-base-300/70 bg-base-200/40 px-3 py-2">
                <input
                  className="input input-xs w-full"
                  type="text"
                  placeholder={t("settings.team.namePlaceholder")}
                  value={name}
                  onChange={(e) => setName(e.target.value)}
                  aria-label={`${r.name} ${t("settings.team.namePlaceholder")}`}
                />
                <textarea
                  className="textarea textarea-xs w-full resize-y text-[11px]"
                  placeholder={t("settings.team.skillPlaceholder")}
                  value={skill}
                  onChange={(e) => setSkill(e.target.value)}
                />
                <SkillPicker />
                <div className="flex gap-1">
                  <button type="button" className="btn btn-primary btn-xs" onClick={save}>
                    {t("settings.team.save")}
                  </button>
                  <button type="button" className="btn btn-ghost btn-xs" onClick={resetDraft}>
                    {t("settings.team.cancel")}
                  </button>
                </div>
              </li>
            ) : (
              <li key={r.id} className="flex flex-col gap-1 rounded-box border border-base-300/70 bg-base-200/40 px-3 py-2">
                <div className="flex items-start gap-2">
                  <div className="min-w-0 flex-1">
                    <div className="text-xs font-semibold">{r.name}</div>
                    {r.skill && <p className="mt-0.5 whitespace-pre-wrap text-[11px] text-base-content/60">{r.skill}</p>}
                    {r.skills.length > 0 && (
                      <div className="mt-1 flex flex-wrap gap-1">
                        {r.skills.map((sn) => (
                          <span key={sn} className="badge badge-sm border-primary/40 bg-primary/5 text-primary/90">
                            {sn}
                          </span>
                        ))}
                      </div>
                    )}
                  </div>
                  <div className="flex shrink-0 gap-0.5">
                    <button
                      type="button"
                      className="btn btn-ghost btn-square btn-xs text-base-content/50"
                      aria-label={t("settings.team.edit", { name: r.name })}
                      title={t("settings.team.edit", { name: r.name })}
                      onClick={() => startEdit(r)}
                    >
                      <IconEdit size={13} stroke={1.75} aria-hidden />
                    </button>
                    <button
                      type="button"
                      className="btn btn-ghost btn-square btn-xs text-base-content/50"
                      aria-label={t("settings.team.remove", { name: r.name })}
                      title={t("settings.team.remove", { name: r.name })}
                      onClick={() => commit(roles.filter((x) => x.id !== r.id))}
                    >
                      <IconTrash size={13} stroke={1.75} aria-hidden />
                    </button>
                  </div>
                </div>
              </li>
            ),
          )}
        </ul>
        {/* 新增表单:角色名 + 职责 + 技能下拉 */}
        {!editingId && (
          <div className="flex flex-col gap-1.5 border-t border-base-300/70 p-3">
            <div className="text-xs font-semibold text-base-content/60">{t("settings.team.addNew")}</div>
            <input
              className="input input-sm w-full"
              type="text"
              placeholder={t("settings.team.namePlaceholder")}
              value={name}
              onChange={(e) => setName(e.target.value)}
            />
            <textarea
              className="textarea textarea-sm w-full resize-y"
              placeholder={t("settings.team.skillPlaceholder")}
              value={skill}
              onChange={(e) => setSkill(e.target.value)}
            />
            <SkillPicker />
            <button type="button" className="btn btn-outline btn-sm w-fit" onClick={save}>
              <IconPlus size={13} stroke={2} aria-hidden />
              {t("settings.team.add")}
            </button>
          </div>
        )}

        {/* 工作流(SOP)编辑 */}
        <div className="flex flex-col gap-2 border-t border-base-300/70 p-3">
          <div className="text-xs font-semibold text-base-content/60">{t("settings.team.workflow.title")}</div>
          <p className="text-[11px] text-base-content/50">{t("settings.team.workflow.hint")}</p>
          {workflow.length === 0 && roles.length === 0 && (
            <p className="text-[11px] text-base-content/35">{t("settings.team.workflow.noRoles")}</p>
          )}
          {workflow.map((s, idx) => (
            <div key={s.id} className="flex flex-col gap-1.5 rounded-box border border-base-300/70 bg-base-200/40 px-3 py-2">
              <div className="flex items-center gap-1.5">
                <span className="text-[10px] text-base-content/35">{idx + 1}.</span>
                <input
                  className="input input-xs h-6 min-h-0 flex-1 text-[11px]"
                  type="text"
                  placeholder={t("settings.team.workflow.stepTitle")}
                  value={s.title}
                  aria-label={`${t("settings.team.workflow.stepTitle")} ${idx + 1}`}
                  onChange={(e) => updateStep(s.id, { title: e.target.value })}
                />
                <button
                  type="button"
                  className="btn btn-ghost btn-square btn-xs text-base-content/40"
                  aria-label={t("settings.team.workflow.up")}
                  disabled={idx === 0}
                  onClick={() => moveStep(idx, -1)}
                >
                  <IconArrowUp size={12} stroke={1.75} aria-hidden />
                </button>
                <button
                  type="button"
                  className="btn btn-ghost btn-square btn-xs text-base-content/40"
                  aria-label={t("settings.team.workflow.down")}
                  disabled={idx === workflow.length - 1}
                  onClick={() => moveStep(idx, 1)}
                >
                  <IconArrowDown size={12} stroke={1.75} aria-hidden />
                </button>
                <button
                  type="button"
                  className="btn btn-ghost btn-square btn-xs text-base-content/40 hover:text-error"
                  aria-label={t("settings.team.workflow.remove")}
                  onClick={() => removeStep(s.id)}
                >
                  <IconTrash size={12} stroke={1.75} aria-hidden />
                </button>
              </div>
              <div className="flex flex-wrap items-center gap-1">
                {roles.length === 0 && <span className="text-[10px] text-base-content/35">{t("settings.team.workflow.noRoles")}</span>}
                {roles.map((r) => {
                  const on = s.roleIds.includes(r.id);
                  return (
                    <button
                      key={r.id}
                      type="button"
                      className={`badge badge-sm cursor-pointer ${
                        on ? "border-primary/60 bg-primary/10 text-primary" : "border-base-300 bg-base-100 text-base-content/50"
                      }`}
                      title={t("settings.team.workflow.toggleRole", { name: r.name })}
                      onClick={() => toggleStepRole(s.id, r.id)}
                    >
                      {r.name}
                    </button>
                  );
                })}
              </div>
            </div>
          ))}
          <button type="button" className="btn btn-ghost btn-xs w-fit text-base-content/60" onClick={addStep}>
            <IconPlus size={12} stroke={2} aria-hidden />
            {t("settings.team.workflow.addStep")}
          </button>
        </div>
      </div>
    </section>
  );
}
