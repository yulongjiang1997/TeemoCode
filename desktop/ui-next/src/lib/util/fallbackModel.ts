/** 备用模型链的下一格:当前模型在链中的位置 +1;不在链里(主模型)则取第一个备用。
 *  注意不能用"当前打头 + 全链"——切换后当前在链首,chain[1] 恒等于自己,永远只切一次。 */
export function nextFallbackModel(current: string, backups: readonly string[]): string | undefined {
  const pos = backups.indexOf(current);
  return pos >= 0 ? backups[pos + 1] : backups[0];
}
