import { sameModelName } from "@/lib/models/modelMenu";

/** 备用模型链的下一格。
 *  先按名字宽松匹配(剥 @source#后缀 + 小写):会话 meta.model 可能是剥后缀
 *  后的短名("minimaxai/minimax-m3"),而备用链存全名("...@monkeycode#uuid")——
 *  精确 indexOf 永远匹配不上 → 永远取第一个。匹配不上再按配置身份兜底;
 *  仅当恰好一个备用与当前同配置身份时才从它后面取,否则(多个模型共用
 *  同一 model 字段)视为主模型取第一个备用。
 *  返回 undefined = 没有下一格(当前已是最后一个备用,或链空)。 */
export function nextFallbackModel(
  current: string,
  backups: readonly string[],
  resolve?: (v: string) => string | undefined,
): string | undefined {
  const posByName = backups.findIndex((b) => sameModelName(b, current));
  if (posByName >= 0) return backups[posByName + 1];
  if (resolve) {
    const curId = resolve(current);
    if (curId) {
      const sameId = backups.filter((b) => resolve(b) === curId);
      if (sameId.length === 1) {
        const target = sameId[0];
        const pos = target !== undefined ? backups.indexOf(target) : -1;
        return pos >= 0 ? backups[pos + 1] : undefined;
      }
    }
  }
  return backups[0];
}
