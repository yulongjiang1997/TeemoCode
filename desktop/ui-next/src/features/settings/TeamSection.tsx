import { IconPlus, IconTrash } from "@tabler/icons-react";
import { useState } from "react";

import { useI18n } from "@/lib/i18n";
import { readTeamRoles, writeTeamRoles, type TeamRole } from "@/lib/util/prefs";

/** 子代理团队:统一模型,按技能(角色)分派。团队成员提前配置,任务下发时
 *  主模型(协调者)拆解并分派给合适成员。 */
export function TeamSection() {
  const { t } = useI18n();
  const [roles, setRoles] = useState<TeamRole[]>(readTeamRoles);
  const [name, setName] = useState("");
  const [skill, setSkill] = useState("");

  const commit = (next: TeamRole[]) => {
    setRoles(next);
    writeTeamRoles(next);
  };

  const add = () => {
    const n = name.trim();
    const s = skill.trim();
    if (!n) return;
    commit([...roles, { id: crypto.randomUUID(), name: n, skill: s }]);
    setName("");
    setSkill("");
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
            <li key={r.id} className="flex items-start gap-2 rounded-box border border-base-300/70 bg-base-200/40 px-3 py-2">
              <div className="min-w-0 flex-1">
                <div className="text-xs font-semibold">{r.name}</div>
                {r.skill && <p className="mt-0.5 whitespace-pre-wrap text-[11px] text-base-content/60">{r.skill}</p>}
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
          <button type="button" className="btn btn-outline btn-sm w-fit" onClick={add}>
            <IconPlus size={13} stroke={2} aria-hidden />
            {t("settings.team.add")}
          </button>
        </div>
      </div>
    </section>
  );
}
