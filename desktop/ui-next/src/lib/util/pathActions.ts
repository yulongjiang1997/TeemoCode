import { openMenu, type MenuItem } from "@/lib/contextMenu";
import { t } from "@/lib/i18n";
import { openPath, revealPath } from "@/lib/ipc/host";
import { copyText } from "@/lib/util/clipboard";
import { isAbsFilePath } from "@/lib/util/filePaths";

export type PathAction = "reveal" | "open";

/** 左键默认「打开文件夹」(reveal);相对路径有 onLocalLink 时走工作区通道。 */
export function runPathAction(path: string, action: PathAction, onLocalLink?: (path: string) => void): void {
  const p = path.trim();
  if (!p) return;
  if (action === "reveal" && !isAbsFilePath(p) && onLocalLink) {
    onLocalLink(p);
    return;
  }
  const run = action === "open" ? openPath : revealPath;
  void run(p).catch((err: unknown) => {
    console.warn(err instanceof Error ? err.message : String(err));
  });
}

export function pathMenuItems(path: string, onLocalLink?: (path: string) => void): MenuItem[] {
  const p = path.trim();
  return [
    { label: t("md.path.copy"), run: () => copyText(p) },
    { label: t("md.path.reveal"), run: () => runPathAction(p, "reveal", onLocalLink) },
    { label: t("md.path.open"), run: () => runPathAction(p, "open", onLocalLink) },
  ];
}

const PATH_MENU_CLASS =
  "menu bg-base-100 rounded-box fixed z-50 w-44 flex-nowrap [&_li]:flex-nowrap shadow-sm";

export function openPathContextMenu(
  e: { clientX: number; clientY: number; preventDefault: () => void; stopPropagation: () => void },
  path: string,
  onLocalLink?: (path: string) => void,
): void {
  const p = path.trim();
  if (!p) return;
  e.preventDefault();
  e.stopPropagation();
  openMenu({ x: e.clientX, y: e.clientY }, pathMenuItems(p, onLocalLink), { menuClass: PATH_MENU_CLASS });
}
