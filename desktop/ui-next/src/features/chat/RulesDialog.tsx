// 工作区 AGENTS.md 规则弹窗(与 MEMORY.md 记忆分开)。
import { IconX } from "@tabler/icons-react";
import { useEffect, useState } from "react";

import { useI18n } from "@/lib/i18n";
import { projectRulesRead, projectRulesWrite } from "@/lib/ipc/projectRules";

interface RulesDialogProps {
  workdir: string;
  onClose: () => void;
}

export function RulesDialog({ workdir, onClose }: RulesDialogProps) {
  const { t } = useI18n();
  const [text, setText] = useState<string | null>(null);
  const [dirty, setDirty] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    projectRulesRead(workdir)
      .then((content) => {
        if (!alive) return;
        setText(content ?? "");
        setDirty(false);
      })
      .catch((e: unknown) => {
        if (!alive) return;
        setText("");
        setError(e instanceof Error ? e.message : String(e));
      });
    return () => {
      alive = false;
    };
  }, [workdir]);

  const save = () => {
    if (text === null) return;
    setBusy(true);
    setError(null);
    projectRulesWrite(workdir, text)
      .then(() => {
        setDirty(false);
        setNotice(t("chat.rules.saved"));
      })
      .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)))
      .finally(() => setBusy(false));
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40" onClick={dirty ? undefined : onClose}>
      <div
        className="flex max-h-[80vh] w-[680px] max-w-[95vw] flex-col rounded-box border border-base-300 bg-base-100 shadow-xl"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between border-b border-base-300 px-4 py-3">
          <div className="min-w-0">
            <h2 className="text-sm font-semibold">{t("chat.rules.title")}</h2>
            <p className="truncate text-2xs text-base-content/50" title={workdir}>
              {workdir}/AGENTS.md
            </p>
          </div>
          <button type="button" className="btn btn-ghost btn-square btn-xs" onClick={onClose} aria-label={t("chat.rules.close")}>
            <IconX size={14} aria-hidden />
          </button>
        </div>
        <div className="flex min-h-0 flex-1 flex-col gap-2 p-4">
          <p className="text-xs leading-relaxed text-base-content/60">{t("chat.rules.hint")}</p>
          {error && (
            <div role="alert" className="alert alert-error alert-soft text-xs">
              {error}
            </div>
          )}
          {notice && <div className="alert alert-success alert-soft text-xs">{notice}</div>}
          {text === null ? (
            <div className="flex flex-1 items-center justify-center text-xs text-base-content/40">{t("chat.rules.loading")}</div>
          ) : (
            <textarea
              className="textarea min-h-64 flex-1 w-full resize-none font-mono text-xs leading-relaxed"
              aria-label={t("chat.rules.title")}
              placeholder={t("chat.rules.empty")}
              value={text}
              onChange={(e) => {
                setText(e.target.value);
                setDirty(true);
                setNotice(null);
              }}
            />
          )}
        </div>
        <div className="flex items-center gap-2 border-t border-base-300 px-4 py-3">
          <span className="flex-1 text-2xs text-base-content/40">{dirty ? t("chat.rules.dirty") : ""}</span>
          <button type="button" className="btn btn-ghost btn-sm" onClick={onClose}>
            {t("settings.team.cancel")}
          </button>
          <button type="button" className="btn btn-primary btn-sm" disabled={busy || text === null || !dirty} onClick={save}>
            {t("chat.rules.save")}
          </button>
        </div>
      </div>
    </div>
  );
}
