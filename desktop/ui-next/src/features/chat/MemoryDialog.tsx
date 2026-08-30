// 工作区记忆弹窗:查看/编辑引擎约定的 <workdir>/.monkeycode/MEMORY.md。
// 引擎在该工作区的每个会话自动加载这份记忆(用户指令/项目知识),这里
// 给它一个可见可编辑的面板(对标 ZCode 的持久记忆管理)。保存走原子写,
// 不存在时保存即创建。
import { IconX } from "@tabler/icons-react";
import { useEffect, useState } from "react";

import { useI18n } from "@/lib/i18n";
import { memoryRead, memoryWrite } from "@/lib/ipc/memory";

interface MemoryDialogProps {
  /** 当前会话的工作目录(会话元数据) */
  workdir: string;
  onClose: () => void;
}

export function MemoryDialog({ workdir, onClose }: MemoryDialogProps) {
  const { t } = useI18n();
  const [text, setText] = useState<string | null>(null); // null = 加载中
  const [dirty, setDirty] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    memoryRead(workdir)
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
    memoryWrite(workdir, text)
      .then(() => {
        setDirty(false);
        setNotice(t("chat.memory.saved"));
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
        {/* 头部 */}
        <div className="flex items-center justify-between border-b border-base-300 px-4 py-3">
          <div className="min-w-0">
            <h2 className="text-sm font-semibold">{t("chat.memory.title")}</h2>
            <p className="truncate text-2xs text-base-content/50" title={workdir}>
              {workdir}/.monkeycode/MEMORY.md
            </p>
          </div>
          <button type="button" className="btn btn-ghost btn-square btn-xs" onClick={onClose} aria-label={t("chat.memory.close")}>
            <IconX size={14} aria-hidden />
          </button>
        </div>

        <div className="flex min-h-0 flex-1 flex-col gap-2 p-4">
          <p className="text-xs leading-relaxed text-base-content/60">{t("chat.memory.hint")}</p>
          {error && (
            <div role="alert" className="alert alert-error alert-soft text-xs">
              {error}
            </div>
          )}
          {notice && (
            <div className="alert alert-success alert-soft text-xs">{notice}</div>
          )}
          {text === null ? (
            <div className="flex flex-1 items-center justify-center text-xs text-base-content/40">{t("chat.memory.loading")}</div>
          ) : (
            <textarea
              className="textarea min-h-64 flex-1 w-full resize-none font-mono text-xs leading-relaxed"
              aria-label={t("chat.memory.title")}
              placeholder={t("chat.memory.empty")}
              value={text}
              onChange={(e) => {
                setText(e.target.value);
                setDirty(true);
                setNotice(null);
              }}
            />
          )}
        </div>

        {/* 底部动作条 */}
        <div className="flex items-center gap-2 border-t border-base-300 px-4 py-3">
          <span className="flex-1 text-2xs text-base-content/40">{dirty ? t("chat.memory.dirty") : ""}</span>
          <button type="button" className="btn btn-ghost btn-sm" onClick={onClose}>
            {t("settings.team.cancel")}
          </button>
          <button type="button" className="btn btn-primary btn-sm" disabled={busy || text === null || !dirty} onClick={save}>
            {t("chat.memory.save")}
          </button>
        </div>
      </div>
    </div>
  );
}
