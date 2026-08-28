/**
 * CommandWarehouse:指令仓库弹窗。
 * 保存预设指令，手动逐条发送到对话。与运行中的自动队列完全分离。
 *
 * - 顶部：标题 + 关闭按钮 + 全部清除按钮
 * - 中部：仓库列表，每项序号 + 文本（可编辑）+ 发送按钮 + 删除按钮
 * - 底部：输入框 + 附件按钮 + 添加按钮（新增指令到仓库）
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
import type { ComposerAtt } from "./useComposer";

interface WarehouseItem {
  id: string;
  text: string;
  atts: ComposerAtt[];
}

interface CommandWarehouseProps {
  /** 当前仓库中的指令 */
  items: WarehouseItem[];
  /** 新增一条指令到仓库 */
  onAdd: (text: string, atts: ComposerAtt[]) => void;
  /** 编辑仓库中的一条指令 */
  onEdit: (id: string, text: string) => void;
  /** 删除仓库中的一条指令 */
  onRemove: (id: string) => void;
  /** 清空仓库 */
  onClear: () => void;
  /** 发送一条指令到对话（实际执行） */
  onSend: (id: string, text: string, atts: ComposerAtt[]) => void;
  /** 关闭弹窗 */
  onClose: () => void;
}

export const CommandWarehouse = memo(function CommandWarehouse({
  items,
  onAdd,
  onEdit,
  onRemove,
  onClear,
  onSend,
  onClose,
}: CommandWarehouseProps) {
  const { t } = useI18n();
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
      onEdit(editingId, editText);
      setEditingId(null);
      setEditText("");
    }
  }, [editingId, editText, onEdit]);

  const handleAdd = useCallback(() => {
    const trimmed = newText.trim();
    if (!trimmed) return;
    onAdd(trimmed, [...newAtts]);
    setNewText("");
    setNewAtts([]);
    inputRef.current?.focus();
  }, [newText, newAtts, onAdd]);

  const handlePaste = useCallback((e: ClipboardEvent<HTMLTextAreaElement>) => {
    const files = e.clipboardData.files;
    if (files.length > 0) {
      e.preventDefault();
      const atts: ComposerAtt[] = Array.from(files).map((f) => ({
        path: f.name,
        name: f.name,
        isImage: f.type.startsWith("image/"),
      }));
      setNewAtts((prev) => [...prev, ...atts]);
    }
  }, []);

  const handleAttach = useCallback(async () => {
    const paths = await pickAttachmentPaths();
    if (paths.length > 0) {
      setNewAtts((prev) => [
        ...prev,
        ...paths.map((p) => ({ path: p, name: p.split(/[\\/]/).pop() || p, isImage: /\.(png|jpe?g|gif|bmp|webp)$/i.test(p) })),
      ]);
    }
  }, []);

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40" onClick={onClose}>
      <div
        className="flex h-[70vh] w-[480px] max-w-[95vw] flex-col rounded-box border border-base-300 bg-base-100 shadow-xl"
        onClick={(e) => e.stopPropagation()}
      >
        {/* 头部 */}
        <div className="flex items-center justify-between border-b border-base-300 px-4 py-3">
          <h2 className="text-sm font-semibold">{t("chat.warehouse.title")}</h2>
          <div className="flex items-center gap-2">
            <span className="text-xs text-base-content/50">{t("chat.warehouse.count", { n: items.length })}</span>
            {items.length > 0 && (
              <button
                type="button"
                className="btn btn-ghost btn-xs text-error"
                onClick={onClear}
              >
                {t("chat.warehouse.clearAll")}
              </button>
            )}
            <button type="button" className="btn btn-ghost btn-square btn-xs" onClick={onClose}>
              <IconX size={14} aria-hidden />
            </button>
          </div>
        </div>

        {/* 指令列表 */}
        <div className="min-h-0 flex-1 overflow-y-auto p-3">
          {items.length === 0 ? (
            <p className="py-8 text-center text-xs text-base-content/40">{t("chat.warehouse.empty")}</p>
          ) : (
            <ul className="flex flex-col gap-2">
              {items.map((item, idx) => (
                <li
                  key={item.id}
                  className="rounded-box border border-base-300 bg-base-200/30 px-3 py-2"
                >
                  {editingId === item.id ? (
                    <div className="flex flex-col gap-1.5">
                      <textarea
                        autoFocus
                        className="textarea textarea-bordered textarea-xs w-full min-h-[4rem] resize-none text-xs"
                        value={editText}
                        onChange={(e) => setEditText(e.target.value)}
                        onKeyDown={(e) => { if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); commitEdit(); } if (e.key === "Escape") { setEditingId(null); setEditText(""); } }}
                      />
                      <div className="flex justify-end gap-1">
                        <button type="button" className="btn btn-ghost btn-xs" onClick={() => { setEditingId(null); setEditText(""); }}>{t("settings.skills.cancel")}</button>
                        <button type="button" className="btn btn-primary btn-xs" onClick={commitEdit}>{t("chat.warehouse.saveEdit")}</button>
                      </div>
                    </div>
                  ) : (
                    <>
                      <div className="flex items-start gap-2">
                        <span className="shrink-0 text-xs font-medium text-base-content/40">{idx + 1}</span>
                        <p className="min-w-0 flex-1 whitespace-pre-wrap text-xs text-base-content/80">{item.text}</p>
                        {item.atts.length > 0 && (
                          <span className="shrink-0 text-[10px] text-base-content/40">📎{item.atts.length}</span>
                        )}
                      </div>
                      <div className="mt-1.5 flex items-center gap-1">
                        <button
                          type="button"
                          className="btn btn-primary btn-xs gap-1"
                          onClick={() => onSend(item.id, item.text, item.atts)}
                          title={t("chat.warehouse.sendTip")}
                        >
                          <IconSend size={12} aria-hidden />
                          {t("chat.warehouse.send")}
                        </button>
                        <button
                          type="button"
                          className="btn btn-ghost btn-xs"
                          onClick={() => startEdit(item.id, item.text)}
                          title={t("chat.warehouse.edit")}
                        >
                          <IconPencil size={12} aria-hidden />
                        </button>
                        <button
                          type="button"
                          className="btn btn-ghost btn-xs text-error"
                          onClick={() => onRemove(item.id)}
                          title={t("chat.warehouse.delete")}
                        >
                          <IconTrash size={12} aria-hidden />
                        </button>
                      </div>
                    </>
                  )}
                </li>
              ))}
            </ul>
          )}
        </div>

        {/* 底部新增区 */}
        <div className="border-t border-base-300 p-3">
          <div className="flex gap-2">
            <textarea
              ref={inputRef}
              className="textarea textarea-bordered textarea-xs min-h-[3rem] flex-1 resize-none text-xs"
              placeholder={t("chat.warehouse.addPlaceholder")}
              value={newText}
              onChange={(e) => setNewText(e.target.value)}
              onPaste={handlePaste}
              onKeyDown={(e) => { if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); handleAdd(); } }}
            />
            <div className="flex shrink-0 flex-col gap-1">
              <button type="button" className="btn btn-ghost btn-square btn-xs" onClick={handleAttach} title={t("chat.attachTip")}>
                <IconPaperclip size={14} aria-hidden />
              </button>
              <button type="button" className="btn btn-primary btn-square btn-xs" onClick={handleAdd} disabled={!newText.trim()} title={t("chat.warehouse.addTip")}>
                <IconPlayerPlay size={14} aria-hidden />
              </button>
            </div>
          </div>
          {newAtts.length > 0 && (
            <div className="mt-1.5 flex flex-wrap gap-1">
              {newAtts.map((a, i) => (
                <span key={i} className="badge badge-ghost badge-xs">
                  {a.name}
                  <button type="button" className="ml-0.5 text-error" onClick={() => setNewAtts((p) => p.filter((_, j) => j !== i))}>×</button>
                </span>
              ))}
            </div>
          )}
        </div>
      </div>
    </div>
  );
});
