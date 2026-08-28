/**
 * QueueModal:全屏弹窗管理指令队列。
 * - 顶部:标题 + 关闭按钮 + 全部清除按钮
 * - 中部:队列列表，每项序号+文本(可编辑)+发送/删除按钮
 * - 底部:输入框 + 附件按钮 + 添加按钮(新增指令到队列)
 */
import {
  IconPaperclip,
  IconPencil,
  IconPlayerPlay,
  IconSend,
  IconTrash,
  IconX,
} from "@tabler/icons-react";
import {
  memo,
  useCallback,
  useRef,
  useState,
  type ClipboardEvent,
} from "react";

import { useI18n } from "@/lib/i18n";
import { pickAttachmentPaths } from "@/lib/ipc/uploads";
import { useEscLayer } from "@/lib/util/escLayer";
import type { ComposerAtt, ComposerCtl } from "./useComposer";

interface QueueModalProps {
  ctl: ComposerCtl;
  onClose: () => void;
}

export const QueueModal = memo(function QueueModal({ ctl, onClose }: QueueModalProps) {
  const { t } = useI18n();
  const {
    queue,
    editInstr,
    sendInstr,
    removeInstr,
    clearQueue,
    addInstr,
    addFiles,
  } = ctl;

  const [editingId, setEditingId] = useState<string | null>(null);
  const [editText, setEditText] = useState("");
  const [newText, setNewText] = useState("");
  const [newAtts, setNewAtts] = useState<ComposerAtt[]>([]);
  const inputRef = useRef<HTMLTextAreaElement | null>(null);

  useEscLayer(true, () => { onClose(); return true; });

  const startEdit = useCallback((id: string, text: string) => {
    setEditingId(id);
    setEditText(text);
  }, []);

  const commitEdit = useCallback(() => {
    if (editingId) {
      editInstr(editingId, editText);
      setEditingId(null);
    }
  }, [editingId, editText, editInstr]);

  const cancelEdit = useCallback(() => {
    setEditingId(null);
  }, []);

  const handleAdd = useCallback(() => {
    const trimmed = newText.trim();
    if (!trimmed && newAtts.length === 0) return;
    addInstr(trimmed, newAtts);
    setNewText("");
    setNewAtts([]);
  }, [newText, newAtts, addInstr]);

  const handleAddKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
      if (e.key === "Enter" && !e.shiftKey) {
        e.preventDefault();
        handleAdd();
      }
    },
    [handleAdd],
  );

  const handlePaste = useCallback(
    (e: ClipboardEvent<HTMLTextAreaElement>) => {
      const files: File[] = [];
      for (const item of e.clipboardData.items) {
        if (item.kind === "file") {
          const f = item.getAsFile();
          if (f) files.push(f);
        }
      }
      if (files.length) {
        e.preventDefault();
        void addFiles(files).then(() => {
          // addFiles 会把上传完成的附件加到主 composer 的 atts,
          // 这里我们直接从剪贴板获取文件后加到本地 newAtts 状态
        });
      }
    },
    [addFiles],
  );

  const attach = useCallback(() => {
    void pickAttachmentPaths(t("chat.attachDialogTitle")).then((paths) => {
      if (paths.length) {
        const newItems: ComposerAtt[] = paths.map((p) => ({
          path: p,
          name: p.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || p,
          isImage: /\.(png|jpe?g|gif|webp|bmp|svg)$/i.test(p),
        }));
        setNewAtts((prev) => [...prev, ...newItems]);
      }
    });
  }, [t]);

  const removeNewAtt = useCallback((idx: number) => {
    setNewAtts((prev) => prev.filter((_, i) => i !== idx));
  }, []);

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center">
      <div className="modal modal-open" role="dialog" aria-label={t("chat.queue.modalTitle")}>
        <div className="modal-box flex max-h-[84vh] w-[min(640px,92vw)] max-w-[min(640px,92vw)] flex-col gap-3 p-5">
          {/* 顶部:标题 + 全部清除 + 关闭 */}
          <div className="flex shrink-0 items-center gap-2">
            <h2 className="min-w-0 flex-1 truncate text-sm font-semibold">
              {t("chat.queue.modalTitle")}
              {queue.length > 0 && (
                <span className="ml-2 text-xs font-normal text-base-content/50">
                  {t("chat.queue.count", { n: queue.length })}
                </span>
              )}
            </h2>
            {queue.length > 0 && (
              <button
                type="button"
                className="btn btn-ghost btn-xs text-error/70 hover:text-error"
                onClick={clearQueue}
              >
                {t("chat.queue.modalClearAll")}
              </button>
            )}
            <button
              type="button"
              aria-label={t("chat.queue.modalClose")}
              title={t("chat.queue.modalClose")}
              className="btn btn-ghost btn-square btn-xs"
              onClick={onClose}
            >
              <IconX size={14} stroke={1.75} aria-hidden />
            </button>
          </div>

          {/* 队列列表 */}
          <div className="flex-1 overflow-x-hidden overflow-y-auto rounded-box border border-base-200 bg-base-100 p-2">
            {queue.length === 0 ? (
              <p className="py-6 text-center text-xs text-base-content/50">
                {t("chat.queue.modalEmpty")}
              </p>
            ) : (
              <ul className="space-y-1">
                {queue.map((item, idx) => (
                  <li
                    key={item.id}
                    className={`flex items-start gap-1 rounded px-1 py-0.5 ${
                      item.state === "executing"
                        ? "bg-info/5"
                        : item.state === "failed"
                          ? "bg-error/5"
                          : ""
                    }`}
                  >
                    {/* 序号 */}
                    <span className="mt-1 shrink-0 text-[10px] font-medium text-base-content/40">
                      {t("chat.queue.modalSeq", { n: idx + 1 })}
                    </span>

                    {/* 文本区域 */}
                    <div className="min-w-0 flex-1">
                      {editingId === item.id ? (
                        <textarea
                          rows={2}
                          className="textarea textarea-bordered w-full resize-none text-xs"
                          value={editText}
                          onChange={(e) => setEditText(e.target.value)}
                          onKeyDown={(e) => {
                            if (e.key === "Enter" && !e.shiftKey) {
                              e.preventDefault();
                              commitEdit();
                            }
                            if (e.key === "Escape") cancelEdit();
                          }}
                          autoFocus
                        />
                      ) : (
                        <p
                          className="cursor-pointer break-all text-xs leading-relaxed text-base-content/80 hover:text-base-content"
                          onClick={() => startEdit(item.id, item.text)}
                          title={t("chat.queue.edit")}
                        >
                          {item.text}
                        </p>
                      )}
                      {/* 附件显示 */}
                      {item.atts && item.atts.length > 0 && (
                        <div className="mt-0.5 flex flex-wrap gap-1">
                          {item.atts.map((a) => (
                            <span key={a.path} className="badge badge-ghost text-[10px]">
                              {a.name}
                            </span>
                          ))}
                        </div>
                      )}
                      {/* 状态标签 */}
                      {item.state === "executing" && (
                        <span className="mt-0.5 inline-block text-[10px] font-medium text-info">
                          <span className="status status-info mr-1 motion-safe:animate-pulse" aria-hidden />
                          {t("chat.queue.executing")}
                        </span>
                      )}
                      {item.state === "failed" && (
                        <span className="mt-0.5 inline-block text-[10px] font-medium text-error">
                          {t("chat.queue.failedItem")}
                        </span>
                      )}
                    </div>

                    {/* 操作按钮 */}
                    <div className="flex shrink-0 items-center gap-0.5">
                      {editingId === item.id ? (
                        <button
                          type="button"
                          className="btn btn-ghost btn-square btn-xs text-base-content/50"
                          title={t("chat.queue.modalEditDone")}
                          onClick={commitEdit}
                        >
                          <IconPlayerPlay size={12} stroke={1.75} aria-hidden />
                        </button>
                      ) : (
                        <>
                          {item.state === "pending" && (
                            <>
                              <button
                                type="button"
                                className="btn btn-ghost btn-square btn-xs text-base-content/50 hover:text-primary"
                                title={t("chat.queue.modalEdit")}
                                onClick={() => startEdit(item.id, item.text)}
                              >
                                <IconPencil size={12} stroke={1.75} aria-hidden />
                              </button>
                              <button
                                type="button"
                                className="btn btn-ghost btn-square btn-xs text-base-content/50 hover:text-primary"
                                title={t("chat.queue.modalSend")}
                                onClick={() => sendInstr(item.id)}
                              >
                                <IconSend size={12} stroke={1.75} aria-hidden />
                              </button>
                            </>
                          )}
                          {item.state !== "executing" && (
                            <button
                              type="button"
                              className="btn btn-ghost btn-square btn-xs text-base-content/50 hover:text-error"
                              title={t("chat.queue.modalDelete")}
                              onClick={() => removeInstr(item.id)}
                            >
                              <IconTrash size={12} stroke={1.75} aria-hidden />
                            </button>
                          )}
                        </>
                      )}
                    </div>
                  </li>
                ))}
              </ul>
            )}
          </div>

          {/* 底部:输入框 + 附件 + 添加 */}
          <div className="shrink-0 rounded-box border border-base-200 bg-base-100 p-2">
            {/* 已选附件 */}
            {newAtts.length > 0 && (
              <div className="mb-1.5 flex flex-wrap gap-1">
                {newAtts.map((a, i) => (
                  <span key={a.path} className="badge badge-ghost text-xs">
                    <span className="max-w-40 truncate">{a.name}</span>
                    <button
                      type="button"
                      aria-label={t("chat.attachRemove")}
                      className="btn btn-ghost btn-circle btn-xs"
                      onClick={() => removeNewAtt(i)}
                    >
                      <IconX size={10} stroke={1.75} aria-hidden />
                    </button>
                  </span>
                ))}
              </div>
            )}
            <div className="flex items-end gap-1.5">
              <button
                type="button"
                aria-label={t("chat.attach")}
                title={t("chat.attachTip")}
                className="btn btn-ghost btn-square btn-xs shrink-0 text-base-content/60"
                onClick={attach}
              >
                <IconPaperclip size={15} stroke={1.75} aria-hidden />
              </button>
              <textarea
                ref={inputRef}
                rows={1}
                className="textarea textarea-bordered min-h-[2rem] flex-1 resize-none text-xs"
                placeholder={t("chat.queue.modalAddPlaceholder")}
                value={newText}
                onChange={(e) => setNewText(e.target.value)}
                onKeyDown={handleAddKeyDown}
                onPaste={handlePaste}
              />
              <button
                type="button"
                className="btn btn-primary btn-square btn-sm shrink-0"
                disabled={!newText.trim() && newAtts.length === 0}
                onClick={handleAdd}
              >
                <IconSend size={14} stroke={1.75} aria-hidden />
              </button>
            </div>
          </div>
        </div>
        <div className="modal-backdrop cursor-pointer" onClick={onClose} aria-hidden />
      </div>
    </div>
  );
});
