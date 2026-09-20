/** 从模型正文里识别「值得当成文件系统路径」的片段。
 * 误判代价:多一个打不开的链接/菜单项(壳侧报错外显);
 * 漏判代价:用户对着路径没法点。所以判定从宽、但排除 URL 和命令行。 */

const TRAIL_PUNCT = /[),.;:!?。，、；：]+$/;

export function trimPathCandidate(raw: string): string {
  return raw.replace(TRAIL_PUNCT, "").replace(/["']$/, "");
}

export function isAbsFilePath(path: string): boolean {
  const s = path.trim();
  if (!s) return false;
  // 盘符后必须是单分隔符,排除 https:// 里的 `s:/`
  if (/^[A-Za-z]:[\\/](?![\\/])/.test(s)) return true;
  if (s.startsWith("\\\\")) return true;
  return s.startsWith("/") && !s.startsWith("//");
}

/** 相对路径:至少一段目录 + 末段带扩展名(避免把 `src/lib` 这种模块路径当文件)。 */
export function looksLikeRelFilePath(path: string): boolean {
  const s = path.trim();
  if (!s || s.length > 260 || /\s/.test(s)) return false;
  if (/^(https?:|mailto:|data:|blob:)/i.test(s)) return false;
  if (!/[\\/]/.test(s)) return false;
  return /(?:^|[\\/])[^\\/]+\.[A-Za-z0-9]{1,12}$/.test(s);
}

/** 整段文本是否像单条路径(围栏代码块 / 行内 `code` 用)。 */
export function looksLikeFilePath(text: string): boolean {
  const s = text.trim();
  if (!s || s.length > 260 || /[\n\r]/.test(s)) return false;
  if (!/[\\/]/.test(s)) return false;
  if (/^(\$|>|npm |npx |node |git |cargo |python|pip |cd |ls |dir )/.test(s)) return false;
  if (/\s/.test(s)) return false;
  if (/^(https?:|mailto:|data:|blob:)/i.test(s)) return false;
  return isAbsFilePath(s) || looksLikeRelFilePath(s) || /[\\/]/.test(s);
}

/** 正文里扫路径:Windows 盘符 / UNC / Unix 绝对 / 带扩展名的相对路径。 */
const PLAIN_PATH_RE =
  /(?:[A-Za-z]:[\\/](?![\\/])[^\s`'"<>|*?]+|\\\\[\w.-]+\\[^\s`'"<>|*?]+|\/(?:[\w.-]+\/)+[\w.-]+|(?:\.\.?\/)?(?:[\w.-]+\/)+[\w.-]+\.[A-Za-z0-9]{1,12})/g;

function isInsideUrl(text: string, index: number): boolean {
  const from = text.lastIndexOf("://", index);
  if (from < 0) return false;
  const scheme = text.slice(Math.max(0, from - 12), from);
  if (!/[a-z][a-z0-9+.-]*$/i.test(scheme)) return false;
  return !/\s/.test(text.slice(from, index));
}

export function findPlainPaths(text: string): Array<{ start: number; end: number; path: string }> {
  const out: Array<{ start: number; end: number; path: string }> = [];
  const re = new RegExp(PLAIN_PATH_RE.source, "g");
  let m: RegExpExecArray | null;
  while ((m = re.exec(text))) {
    if (isInsideUrl(text, m.index)) continue;
    const path = trimPathCandidate(m[0]);
    if (!path) continue;
    if (!isAbsFilePath(path) && !looksLikeRelFilePath(path)) continue;
    out.push({ start: m.index, end: m.index + path.length, path });
  }
  return out;
}
