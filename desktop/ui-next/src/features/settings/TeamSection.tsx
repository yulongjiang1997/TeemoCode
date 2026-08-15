import { IconPlus, IconTrash } from "@tabler/icons-react";
import { useEffect, useState } from "react";

import { useI18n } from "@/lib/i18n";
import { skillsList, type SkillInfo } from "@/lib/ipc/skills";
import { readTeamRoles, writeTeamRoles, type TeamRole } from "@/lib/util/prefs";

/** 子代理团队:统一模型,按技能(角色)分派。团队成员提前配置(职责文本 +
 *  技能库多选),任务下发时主模型(协调者)拆解并分派给合适成员,执行时
 *  职责与指定技能都生效。 */
export function TeamSection() {
  const { t } = useI18n();
  const [roles, setRoles] = useState<TeamRole[]>(readTeamRoles);
  const [name, setName] = useState("");
  const [skill, setSkill] = useState("");
  const [picked, setPicked] = useState<string[]>([]);
  const [skills, setSkills] = useState<SkillInfo[]>([]);

  useEffect(() => {
    void skillsList().then(setSkills).catch(() => {});
  }, []);

  const commit = (next: TeamRole[]) => {
    setRoles(next);
    writeTeamRoles(next);
  };

  const add = () => {
    const n = name.trim();
    if (!n) return;
    commit([...roles, { id: crypto.randomUUID(), name: n, skill: skill.trim(), skills: picked }]);
    setName("");
    setSkill("");
    setPicked([]);
  };

  const toggleRoleSkill = (roleId: string, skillName: string) => {
    commit(
      roles.map((r) =>
        r.id === roleId
          ? {
              ...r,
              skills: r.skills.includes(skillName)
                ? r.skills.filter((x) => x !== skillName)
                : [...r.skills, skillName],
            }
          : r,
      ),
    );
  };

  const patchRoleSkill = (roleId: string, text: string) => {
    commit(roles.map((r) => (r.id === roleId ? { ...r, skill: text } : r)));
  };

  return (
    <section aria-label={t("settings.nav.team")} className="flex flex-col gap-2">
      <div className="rounded-box border border-base-300">
        <div className="flex flex-col gap-1 border-b border-base-300/70 p-3">
          <div className="text-sm font-semibold">{t("settings.team.title")}</div>
          <p className="text-xs text-base-content/60">{t("settings.team.hint")}</p>
        </div>
        <ul className="flex flex-col gap-2 p-3">
          {roles.length === 0 && <li className="text-xs text-base-content/40">{t("settings.team.empty")}</li>}
          {roles.map((r) => (
            <li key={r.id} className="flex flex-col gap-1.5 rounded-box border border-base-300/70 bg-base-200/40 px-3 py-2">
              <div className="flex items-start gap-2">
                <div className="min-w-0 flex-1">
                  <div className="text-xs font-semibold">{r.name}</div>
                  {/* 职责(手动填写,可改) */}
                  <textarea
                    className="textarea textarea-xs mt-1 w-full resize-y text-[11px]"
                    placeholder={t("settings.team.skillPlaceholder")}
                    value={r.skill}
                    onChange={(e) => patchRoleSkill(r.id, e.target.value)}
                    aria-label={`${r.name} ${t("settings.team.skillLabel")}`}
                  />
                </div>
                <button
                  type="button"
                  className="btn btn-ghost btn-square btn-xs shrink-0 text-base-content/50"
                  aria-label={t("settings.team.remove", { name: r.name })}
                  title={t("settings.team.remove", { name: r.name })}
                  onClick={() => commit(roles.filter((x) => x.id !== r.id))}
                >
                  <IconTrash size={13} stroke={1.75} aria-hidden />
                </button>
              </div>
              {/* 成员技能(技能库多选,执行时优先使用) */}
              {skills.length > 0 && (
                <div className="flex flex-wrap items-center gap-1">
                  <span className="text-[10px] text-base-content/40">{t("settings.team.skillsLabel")}:</span>
                  {r.skills.length === 0 && <span className="text-[10px] text-base-content/30">{t("settings.team.skillsNone")}</span>}
                  {r.skills.map((sn) => (
                    <button
                      key={sn}
                      type="button"
                      className="badge badge-sm cursor-pointer gap-1 border-primary/60 bg-primary/10 text-primary"
                      title={t("settings.team.skillsRemoveTip")}
                      onClick={() => toggleRoleSkill(r.id, sn)}
                    >
                      {sn}×
                    </button>
                  ))}
                  {/* 技能下拉:多选追加 */}
                  <select
                    className="select select-xs h-6 min-h-0 w-auto max-w-36 text-[10px]"
                    value=""
                    aria-label={`${r.name} ${t("settings.team.skillsAdd")}`}
                    onChange={(e) => {
                      const v = e.target.value;
                      if (v) toggleRoleSkill(r.id, v);
                    }}
                  >
                    <option value="">{t("settings.team.skillsAdd")}…</option>
                    {skills
                      .filter((s) => !r.skills.includes(s.name))
                      .map((s) => (
                        <option key={s.name} value={s.name}>
                          {s.name}
                        </option>
                      ))}
                  </select>
                </div>
              )}
              {skills.length === 0 && (
                <p className="text-[10px] text-base-content/40">{t("settings.team.noSkills")}</p>
              )}
            </li>
          ))}
        </ul>
        <div className="flex flex-col gap-1.5 border-t border-base-300/70 p-3">
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
          {skills.length > 0 && (
            <select
              className="select select-sm w-full"
              multiple
              aria-label={t("settings.team.skillsAdd")}
              value={picked}
              onChange={(e) => setPicked(Array.from(e.target.selectedOptions).map((o) => o.value))}
            >
              {skills.map((s) => (
                <option key={s.name} value={s.name}>
                  {s.name} — {s.description}
                </option>
              ))}
            </select>
          )}
          <button type="button" className="btn btn-outline btn-sm w-fit" onClick={add}>
            <IconPlus size={13} stroke={2} aria-hidden />
            {t("settings.team.add")}
          </button>
        </div>
      </div>
    </section>
  );
}
