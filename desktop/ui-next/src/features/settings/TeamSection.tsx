import { IconPlus, IconTrash } from "@tabler/icons-react";
import { useEffect, useState } from "react";

import { useI18n } from "@/lib/i18n";
import { skillsList, type SkillInfo } from "@/lib/ipc/skills";
import { readTeamRoles, writeTeamRoles, type TeamRole } from "@/lib/util/prefs";

/** 子代理团队:统一模型,按技能(角色)分派。团队成员提前配置(技能可多选),
 *  任务下发时主模型(协调者)拆解并分派给合适成员,执行优先用成员指定技能。 */
export function TeamSection() {
  const { t } = useI18n();
  const [roles, setRoles] = useState<TeamRole[]>(readTeamRoles);
  const [name, setName] = useState("");
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
    commit([...roles, { id: crypto.randomUUID(), name: n, skills: picked }]);
    setName("");
    setPicked([]);
  };

  const togglePick = (skillName: string) => {
    setPicked((p) => (p.includes(skillName) ? p.filter((x) => x !== skillName) : [...p, skillName]));
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
              {/* 成员技能多选:执行时优先使用 */}
              {skills.length > 0 && (
                <div className="flex flex-wrap gap-1">
                  {skills.map((s) => {
                    const on = r.skills.includes(s.name);
                    return (
                      <button
                        key={s.name}
                        type="button"
                        className={`badge badge-sm cursor-pointer gap-1 transition-colors ${
                          on ? "border-primary/60 bg-primary/10 text-primary" : "badge-outline text-base-content/40 hover:text-base-content/60"
                        }`}
                        title={s.description}
                        onClick={() => toggleRoleSkill(r.id, s.name)}
                      >
                        {s.name}
                      </button>
                    );
                  })}
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
          {skills.length > 0 && (
            <div className="flex flex-wrap gap-1">
              {skills.map((s) => {
                const on = picked.includes(s.name);
                return (
                  <button
                    key={s.name}
                    type="button"
                    className={`badge badge-sm cursor-pointer gap-1 transition-colors ${
                      on ? "border-primary/60 bg-primary/10 text-primary" : "badge-outline text-base-content/40 hover:text-base-content/60"
                    }`}
                    title={s.description}
                    onClick={() => togglePick(s.name)}
                  >
                    {s.name}
                  </button>
                );
              })}
            </div>
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
