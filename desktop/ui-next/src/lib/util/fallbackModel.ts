/** 备用模型链的下一格。
 *  resolve:把模型值(ID 或显示名)归一成配置项身份(模型 ID)——会话里 meta.model
 *  可能是引擎返回的模型 ID,而备用链存显示名,直接 indexOf 匹配不上会永远取第一个
 *  备用(无限重试)。按配置项身份匹配:当前在链中的位置 + 1;不在链里则取第一个。
 *  返回 undefined = 没有下一格(主模型/当前不在链里且链空,或已是最后一个备用)。 */
export function nextFallbackModel(
  current: string,
  backups: readonly string[],
  resolve: (v: string) => string | undefined,
): string | undefined {
  const curId = resolve(current);
  const pos = backups.findIndex((b) => {
    const bid = resolve(b);
    return curId && bid ? bid === curId : b === current;
  });
  return pos >= 0 ? backups[pos + 1] : backups[0];
}
