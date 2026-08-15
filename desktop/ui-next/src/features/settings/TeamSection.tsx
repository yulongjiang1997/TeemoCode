import { IconEdit, IconPlus, IconTrash } from "@tabler/icons-react";
import { useEffect, useState } from "react";

import { useI18n } from "@/lib/i18n";
import { skillsList, type SkillInfo } from "@/lib/ipc/skills";
import { readTeamRoles, writeTeamRoles, type TeamRole } from "@/lib/util/prefs";

/** 子代理团队:统一模型,按技能(角色)分派。团队成员提前配置(角色名 +
 *  职责文本 + 技能库多选下拉),任务下发时主模型(协调者)拆解并分派给
 *  合适成员,执行时职责与指定技能都生效。 */
export function TeamSection() {
  const { t } = useI18n();
  const [roles, setRoles] = useState<TeamRole[]>(readTeamRoles);
  const [skills, setSkills] = useState<SkillInfo[]>([]);
  // 编辑中角色 id(展开该卡为编辑表单)
  const [editingId, setEditingId] = useState<string | null>(null);
  // 编辑/新增的草稿
  const [name, setName] = useState("");
  const [skill, setSkill] = useState("");
  const [picked, setPicked] = useState<string[]>([]);

  useEffect(() => {
    void skillsList().then(setSkills).catch(() => {});
  }, []);

  const commit = (next: TeamRole[]) => {
    setRoles(next);
    writeTeamRoles(next);
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
      </div>
    </section>
  );
}
