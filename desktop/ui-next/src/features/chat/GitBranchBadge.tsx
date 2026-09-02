/**
 * GitBranchBadge:独立 memo 组件,避免分支状态变化触发 ChatView 整体重渲染。
 * - 挂载时查询当前分支+分支列表
 * - 点击徽标弹出分支列表下拉菜单
 * - 点击其他分支时检查工作区干净状态并切换
 */
import { IconCheck, IconGitBranch } from "@tabler/icons-react";
import { memo, useCallback, useEffect, useRef, useState } from "react";

import { useI18n } from "@/lib/i18n";
import { gitBranch, gitBranchList, gitCheckout, gitIsClean } from "@/lib/ipc/git";
import { useDismiss } from "@/lib/util/useDismiss";

/**
 * Tauri 的 dialog API 在不同运行环境下可能返回 Promise，也可能被实现为
 * 同步的浏览器 alert。统一吞掉拒绝，避免旧权限配置下再产生 unhandledrejection。
 */
function safeAlert(message: string): void {
  try {
    void Promise.resolve(window.alert(message)).catch(() => {});
  } catch {
    // 浏览器原生 alert 被禁用时无需再抛出第二个错误。
  }
}

interface GitBranchBadgeProps {
  workdir: string;
  /** 可选:切换分支后回调(如刷新工作区状态) */
  onSwitched?: () => void;
}

export const GitBranchBadge = memo(function GitBranchBadge({ workdir, onSwitched }: GitBranchBadgeProps) {
  const { t } = useI18n();
  const [branch, setBranch] = useState("");
  const [branches, setBranches] = useState<string[]>([]);
  const [open, setOpen] = useState(false);
  const boxRef = useRef<HTMLDivElement | null>(null);

  const refresh = useCallback(async () => {
    if (!workdir) return;
    try {
      const [cur, list] = await Promise.all([gitBranch(workdir), gitBranchList(workdir)]);
      setBranch(cur);
      setBranches(list);
    } catch {
      setBranch("");
      setBranches([]);
    }
  }, [workdir]);

  useEffect(() => { void refresh(); }, [refresh]);

  const switchTo = useCallback(async (target: string) => {
    if (!workdir || target === branch) { setOpen(false); return; }
    try {
      const clean = await gitIsClean(workdir);
      if (!clean) { safeAlert(t("chat.git.branchHint")); setOpen(false); return; }
      await gitCheckout(workdir, target);
      setBranch(target);
      setOpen(false);
      onSwitched?.();
    } catch (e: unknown) {
      const msg = e instanceof Error ? e.message : String(e);
      safeAlert(t("chat.git.switchFailed", { reason: msg }));
      setOpen(false);
    }
  }, [workdir, branch, t, onSwitched]);

  useDismiss(open, boxRef, useCallback(() => setOpen(false), []));

  if (!branch) return null;

  return (
    <div ref={boxRef} className="relative shrink-0">
      <button
        type="button"
        className="badge badge-ghost badge-sm cursor-pointer text-base-content/60 hover:text-base-content"
        onClick={() => { setOpen((o) => !o); }}
      >
        <IconGitBranch size={12} aria-hidden />
        {branch}
      </button>
      {open && (
        <div className="absolute left-0 top-full z-50 mt-1 min-w-[180px] rounded-box border border-base-300 bg-base-100 shadow-lg">
          <ul className="menu menu-sm">
            {branches.length === 0 ? (
              <li className="text-2xs text-base-content/40">{t("chat.git.noBranches")}</li>
            ) : branches.map((b) => (
              <li key={b}>
                <button
                  type="button"
                  className={b === branch ? "active" : ""}
                  onClick={() => void switchTo(b)}
                >
                  {b === branch && <IconCheck size={12} />}
                  {b}
                </button>
              </li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
});
