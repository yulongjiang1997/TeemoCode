/** 备用模型链的下一格。
 *  先按名字精确匹配(会话 meta.model 通常是显示名,与备用链同源)。
 *  当前不在链里(主模型或引擎返回的 ID 形态):仅当恰好一个备用与当前同配置
 *  身份(resolve 归一)时才认为"当前就是它"取其下一个;否则视为主模型取
 *  第一个备用——多个模型共用同一 model 字段时,身份匹配无法区分,不能跳过。
 *  返回 undefined = 没有下一格(当前已是最后一个备用,或链空)。 */
export function nextFallbackModel(
  current: string,
  backups: readonly string[],
  resolve?: (v: string) => string | undefined,
): string | undefined {
  const posByName = backups.indexOf(current);
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
