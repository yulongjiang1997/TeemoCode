// 更新可用性:**模块级单实例 store** + useSyncExternalStore 订阅。
// 挂载检查一次 + 窗口回焦静默复查 + 4 小时兜底,三个触发点共用全局闸门
// (lib/ipc/update)。安装成功后壳自行重启(promise 不返回,busy 不回收);
// 失败复位忙态并把错误文案交给视图外显——吞掉就是按钮永远转圈。
//
// 为什么必须是模块级而不是各持一份 useState(LAYOUT §3「更新可用 = 侧栏底部条
// + 设置·关于」是**同一条信息的两个法定位置**,两处必须同源):
// 此前侧栏走本 hook、关于页自己另存一份 useState,而两条路又共用 update.ts 的
// 模块级 lastCheckAt 闸门。后果是①侧栏已在提示「有新版本」时切到关于页,那里
// 仍显示普通「检查更新」钮(关于页挂载只拉 hostInfo,从不查更新,found 恒 false);
// ②反过来在关于页查到新版本但没装,退出设置后侧栏底部条依旧不显示,而这次检查
// 还把接下来 30 分钟内的回焦复查一起闸掉了——那笔账记了,结果却只留在已卸载的
// 组件里。
import { useEffect, useSyncExternalStore } from "react";

import { RELEASE_HISTORY } from "@/lib/ipc/releaseHistory";
import { recordUpdateCheck, takeUpdateCheck, updateCheck, updateDownload, updateInstall, type UpdateInfo } from "@/lib/ipc/update";

/** 兜底复查间隔:窗口一直开着、从没失去过焦点(挂着跑长任务正是如此)就
 *  永远等不到前台事件,只靠 focus 触发等于不查。被闸门挡掉只是顺延到下一
 *  次 tick,不会重复请求。 */
const FALLBACK_MS = 4 * 3600_000;

let current: UpdateInfo | null = null;
const listeners = new Set<() => void>();

function publish(info: UpdateInfo | null): void {
  // null = 检查失败/浏览器模式(update.ts 收口),不覆盖已知结果
  if (!info) return;
  // 版本历史来自本地内置文件(releaseHistory.ts):历史记录是静态事实,
  // 内置随应用走、永不断档;云端清单只承担最新一版的 notes。当前版本的
  // 条目也保留——「版本历史」面板 = 当前版 + 历史版,云端 notes 只在
  // 「有可用更新」时展示(那是更新提示,不是已装版本的说明)。
  const curVer = parseVersion(info.current);
  if (curVer) {
    info.history = RELEASE_HISTORY.filter((h) => {
      const v = parseVersion(h.version);
      return v && v <= curVer;
    }).map((h) => ({ version: h.version, notes: h.notes }));
  }
  current = info;
  for (const cb of listeners) cb();
}

/** "0.1.19" → [0,1,19];解析失败返回 null(比较时跳过)。 */
function parseVersion(v: string): number[] | null {
  const m = v.trim().match(/^(\d+)\.(\d+)\.(\d+)$/);
  return m ? [Number(m[1]), Number(m[2]), Number(m[3])] : null;
}

function subscribe(cb: () => void): () => void {
  listeners.add(cb);
  return () => listeners.delete(cb);
}

const getSnapshot = (): UpdateInfo | null => current;

/** 下载/安装态也走同一共享 store:关于页与侧栏底部条两处按钮必须同源联动
 *  (这边点「更新」下载,那边同步变进度条;下载完成两边都出「立即安装」)。 */
interface DownloadState {
  downloading: boolean;
  progress: number;
  downloaded: boolean;
  installing: boolean;
  error: string | null;
}
let dl: DownloadState = { downloading: false, progress: 0, downloaded: false, installing: false, error: null };
function publishDl(patch: Partial<DownloadState>): void {
  dl = { ...dl, ...patch };
  for (const cb of listeners) cb();
}
const getDlSnapshot = (): DownloadState => dl;

/** 仅供测试:复位模块级共享态(更新信息 + 下载/安装态)。 */
export function resetUpdateStoreForTests(): void {
  current = null;
  dl = { downloading: false, progress: 0, downloaded: false, installing: false, error: null };
  for (const cb of listeners) cb();
}

/** 只读订阅(关于页):不起轮询,只跟随共享真值。 */
export function useUpdateInfo(): UpdateInfo | null {
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}

/** 手动检查(关于页按钮):**不过闸门**——用户明确要查就得查——但记一笔账,
 *  紧接着切个窗口回来不该再自动查一遍(旧 UI updateGate.record 同款)。
 *  结果并入共享 store,侧栏底部条随之出现。 */
export async function checkUpdateNow(): Promise<UpdateInfo | null> {
  recordUpdateCheck();
  const info = await updateCheck();
  publish(info);
  return info;
}

/** 静默检查(自动触发点):过闸门,被挡就跳过。 */
function checkGated(): void {
  if (!takeUpdateCheck()) return;
  void updateCheck().then(publish);
}

export function useUpdate(): {
  update: UpdateInfo | null;
  /** 下载中(progress 0-100) */
  downloading: boolean;
  progress: number;
  /** 下载完成,待用户确认安装 */
  downloaded: boolean;
  installing: boolean;
  /** 上次下载/安装失败的原因;null = 没失败过/重试中 */
  error: string | null;
  /** 下载更新(不安装);成功后等 install() */
  download: () => void;
  /** 安装已下载的更新(用户确认后调用) */
  install: () => void;
} {
  const update = useUpdateInfo();
  const dl = useSyncExternalStore(subscribe, getDlSnapshot, getDlSnapshot);

  useEffect(() => {
    checkGated();
    window.addEventListener("focus", checkGated);
    const timer = window.setInterval(checkGated, FALLBACK_MS);
    return () => {
      window.removeEventListener("focus", checkGated);
      window.clearInterval(timer);
    };
  }, []);

  return {
    update,
    downloading: dl.downloading,
    progress: dl.progress,
    downloaded: dl.downloaded,
    installing: dl.installing,
    error: dl.error,
    download: () => {
      publishDl({ error: null, downloading: true, progress: 0, downloaded: false });
      void updateDownload((e) => {
        publishDl({ progress: e.progress });
        if (e.state === "downloaded") publishDl({ downloaded: true });
      })
        .then(() => publishDl({ downloaded: true }))
        .catch((e) => {
          publishDl({ downloading: false, error: e instanceof Error ? e.message : String(e) });
        });
    },
    install: () => {
      publishDl({ installing: true, error: null });
      void updateInstall().catch((e) => {
        // 失败:复位忙态并外显;成功后壳自行重启,不会走到这里
        publishDl({ installing: false, downloading: false, error: e instanceof Error ? e.message : String(e) });
      });
    },
  };
}

/** 仅测试用:清空模块级 store(跨用例会串)。 */
export function resetUpdateForTest(): void {
  current = null;
  dl = { downloading: false, progress: 0, downloaded: false, installing: false, error: null };
  for (const cb of listeners) cb();
}
