// cron 下次触发预览:与 Rust 匹配器同语义(分钟粒度),用于 UI 实时展示。
// only cron: once 在编辑器里直接显示人类可读绝对时间。
// 这里只实现 cron 的最接近未来匹配时间的简易推算(向前最多遍历 366 天,
// 但 minute/hour 的字段通常很小,实际分支很少)。

/**
 * 预览 cron 的下一次触发时间（本地时钟）。
 * 返回 `{ date, label }`; expr 非法返回 null。
 */
export function cronNextPreview(expr: string): { date: Date; label: string } | null {
  const parsed = parseCron(expr);
  if (!parsed) return null;
  const now = new Date();
  // 从当前分钟+1 开始搜索,最多到 366 天后
  const msNow = now.getTime();
  const horizon = msNow + 366 * 24 * 60 * 60 * 1000;
  // 候选年份:当前年,若年末都找不到再试下一年
  for (let y = now.getFullYear(); y <= now.getFullYear() + 1; y++) {
    for (let m = 1; m <= 12; m++) {
      if (!parsed.months.has(m)) continue;
      const days = daysInMonth(y, m - 1);
      for (let d = 1; d <= days; d++) {
        if (!parsed.dom.has(d)) continue;
        // weekday(0=Sun):JS Date.getDay()
        const dow = new Date(y, m - 1, d).getDay();
        if (!parsed.weekdays.has(dow)) continue;
        for (let h of sortedAsc(parsed.hours)) {
          // 对每个命中的分钟
          for (let mi of sortedAsc(parsed.minutes)) {
            const date = new Date(y, m - 1, d, h, mi);
            if (date.getTime() > msNow && date.getTime() <= horizon) {
              return { date, label: formatNext(date, msNow) };
            }
          }
        }
      }
    }
  }
  return null;
}

function formatNext(date: Date, nowMs: number): string {
  const diff = date.getTime() - nowMs;
  const mins = Math.round(diff / 60_000);
  if (mins < 60) return `${mins} 分钟后`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours} 小时后`;
  const days = Math.floor(hours / 24);
  if (days < 30) return `${days} 天后`;
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")} ${String(date.getHours()).padStart(2, "0")}:${String(date.getMinutes()).padStart(2, "0")}`;
}

function daysInMonth(y: number, m: number) {
  return new Date(y, m + 1, 0).getDate();
}

function sortedAsc(s: Set<number>): number[] {
  return [...s].sort((a, b) => a - b);
}

interface ParsedCron {
  minutes: Set<number>;
  hours: Set<number>;
  months: Set<number>;
  dom: Set<number>;
  weekdays: Set<number>;
}

function parseCron(expr: string): ParsedCron | null {
  const fields = expr.trim().split(/\s+/);
  if (fields.length !== 5) return null;
  const mm = parseField(fields[0]!, 0, 59);
  const hh = parseField(fields[1]!, 0, 23);
  const dom = parseField(fields[2]!, 1, 31);
  const mon = parseField(fields[3]!, 1, 12);
  const dow = parseField(fields[4]!, 0, 7); // 7 不映射为 0
  if (!mm || !hh || !dom || !mon || !dow) return null;
  // dow:JS Date.getDay() 是 0=Sun..6=Sat;输入 7 视作 0
  const weekdaySet = new Set<number>();
  for (const d of dow) weekdaySet.add(d === 7 ? 0 : d);
  return { minutes: new Set(mm), hours: new Set(hh), months: new Set(mon), dom: new Set(dom), weekdays: weekdaySet };
}

function parseField(field: string, min: number, max: number): number[] | null {
  const out: number[] = [];
  for (const part of field.split(",")) {
    const [rangePart, stepStr] = part.trim().split("/");
    const step = stepStr ? Number(stepStr) : 1;
    if (!Number.isFinite(step) || step < 1) return null;
    let lo: number, hi: number;
    if (rangePart === "*") {
      lo = min;
      hi = max;
    } else if (rangePart!.includes("-")) {
      const [a, b] = rangePart!.split("-").map(Number) as [number, number];
      if (!Number.isFinite(a) || !Number.isFinite(b)) return null;
      lo = a;
      hi = b;
    } else {
      const v = Number(rangePart);
      if (!Number.isFinite(v)) return null;
      lo = v;
      hi = v;
    }
    if (lo < min || hi > max || lo > hi) return null;
    let v = lo;
    while (v <= hi) {
      if (!out.includes(v)) out.push(v);
      v += step;
    }
  }
  out.sort((a, b) => a - b);
  return out.length > 0 ? out : null;
}
