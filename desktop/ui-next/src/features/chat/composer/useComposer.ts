// composer 状态机:草稿/指令队列/附件上传/发送与停止。
// 发送面契约(对表壳侧 driver/session.rs::session_send):
// - user-input 载荷只有 {content: b64};本地附件不进独立字段,按
//   「[图片]/[文件] <工作区相对路径>」附件行并入正文(旧 UI ATT_LINE 同
//   口径,壳只解 content)。
// - Err ⟺ 消息未入会话(未物化任何帧)——失败回队/回草稿是安全的;
//   引擎接活后本轮失败会回 Ok(错误走 task-error 帧),不得重投。
// - 停止 = user-cancel {}(取消斡旋与看门狗都在壳侧)。
// 排队语义:运行中/上一条未回执时发送进**队列**(按序追加,而非覆盖单槽),
// 轮结束(running 变 false)自动补投队头;成功后出队继续投下一条;失败
// (task-error 结束或壳 reject)标该条 failed + 自动暂停,等用户重试/移除/
// 解除暂停,或退避重试到点(避免 chip 永久钉住)。
// 补投的三道闸(每道都对应过一次真实故障,见各自注释):
//   ①feed.historyLoaded —— 首份历史归约前 running 不可信,不许抢投;
//   ②stateSid === sessionId —— 切会话那一帧里 queue 还属于上一个会话;
//   ③sendingRef —— 上行在途(壳已收、回显帧未到)期间不许第二条直发。
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { t } from "@/lib/i18n";
import { sessionCompact } from "@/lib/ipc/controls";
import { sessionSend } from "@/lib/ipc/sessions";
import { attLineOf } from "@/lib/protocol/attLine";
import {
  isImagePath,
  nativePathOf,
  uploadFilePath,
  uploadFileStream,
} from "@/lib/ipc/uploads";
import { b64encode } from "@/lib/protocol/codec";
import { bindActiveComposer, stashGet, stashSet } from "./stash";
import { readComposerQueue, writeComposerQueue, type QueueItem } from "@/lib/util/prefs";

// QueueItem 的权威定义在 lib/util/prefs(叶子模块,无依赖);此处再导出,方便
// stash/test 统一从 useComposer 取类型。
export type { QueueItem } from "@/lib/util/prefs";
// 向后别名:既有 stash/test 以 QueuedInstr 称呼队列项(同构)。
export type QueuedInstr = QueueItem;

export interface ComposerAtt {
  /** 工作区相对路径(壳返回;附件行与模型可读路径都用它)。 */
  path: string;
  name: string;
  isImage: boolean;
}

export interface ComposerUpload {
  id: number;
  name: string;
  /** 0-100;-1 = 不确定进度(路径直拷/空文件,无分块回调)。 */
  pct: number;
  /** 分块通道可取消;路径直拷不可(无句柄)。 */
  cancel?: () => void;
}

/** 本地附件行(约定唯一出处在 lib/protocol/attLine,进消息正文)。 */
export const attLine = (a: ComposerAtt) => attLineOf(a.path, a.isImage);

const newInstrId = (): string => `qi-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;

export interface ComposerCtl {
  draft: string;
  setDraft(v: string): void;
  /** 指令队列(按序发送;队首 executing,失败项 failed 待重试/移除)。 */
  queue: QueueItem[];
  /** 队列区展开态(折叠时只显示首条)。 */
  queueOpen: boolean;
  toggleQueueOpen(): void;
  /** 队列暂停:暂停后轮结束不再自动取下一条(不影响当前轮)。 */
  paused: boolean;
  togglePaused(): void;
  retryInstr(id: string): void;
  removeInstr(id: string): void;
  /** 拖动排序(仅 pending 项可拖;执行中/失败项不参与换序)。 */
  reorderInstr(from: number, to: number): void;
  editInstr(id: string, text: string): void;
  clearQueue(): void;
  atts: ComposerAtt[];
  removeAtt(index: number): void;
  uploads: ComposerUpload[];
  /** 短暂错误提示(上传/切换失败;自动消退)。 */
  error: string | null;
  dismissError(): void;
  notifyError(message: string): void;
  /** 发送草稿+附件;运行中自动追加进指令队列。返回是否已接受(发送或入队)。 */
  send(): boolean;
  stop(): void;
  /** 粘贴/拖拽的 File 上传为附件(path-backed 占位走路径直拷)。 */
  addFiles(files: File[]): Promise<void>;
  /** 系统对话框选出的本地路径直拷为附件。 */
  addPaths(paths: string[]): Promise<void>;
}

const ERROR_TTL_MS = 8000;

/** 补投失败后的退避重试节奏(ms)。旧 UI 在「每批帧到达 / 断线重连 / 首份
 *  历史落地」三处反复重投,ui-next 只在 running 变化时解除失败抑制——可
 *  失败若恰好发生在空闲期(壳还没接活),那条 running 边沿可能永远不来,
 *  「已排队」chip 就永久钉住、谁也不再投。帧水位变化已经补回"每批帧重投",
 *  这串退避再补上"一帧都不再来"的死角;耗尽即停,不无限空转。 */
const FLUSH_RETRY_MS = [600, 1800, 5000, 12000];

/** 数据面喂给 composer 的三个信号(全部来自 useSessionFeed 的 ChatState)。 */
export interface ComposerFeed {
  /** 轮次执行中(壳的忙碌守卫按它拒直发)。 */
  running: boolean;
  /** 首份历史(尾部回放窗口)已落地——落地前 running 恒 false 但不可信。 */
  historyLoaded: boolean;
  /** 帧 seq 水位:任一批帧到达即抬升。等价于旧 UI 的 onFrames 时机——
   *  "壳已把上一条上行物化成帧",是解除在途标记与失败抑制的唯一可信信号。 */
  lastSeq: number;
  /** 轮次正常结束(task-ended 置 true;task-error 不置)——队列靠它区分
   *  "本轮成功" 与 "本轮失败",失败时当前指令标 failed 并自动暂停。 */
  turnEnded: boolean;
}

export function useComposer(sessionId: string, feed: ComposerFeed): ComposerCtl {
  const { running, historyLoaded, lastSeq, turnEnded } = feed;
  const [draft, setDraft] = useState("");
  const [queue, setQueue] = useState<QueueItem[]>([]);
  const [paused, setPaused] = useState(false);
  const [queueOpen, setQueueOpen] = useState(false);
  const [atts, setAtts] = useState<ComposerAtt[]>([]);
  const [uploads, setUploads] = useState<ComposerUpload[]>([]);
  const [error, setError] = useState<string | null>(null);
  // 上行在途:user-input 发出到回执/开轮之间再发必须入队,否则第二条直发
  // 会被壳的忙碌守卫拒掉
  const sendingRef = useRef(false);
  // 排队补投失败后的抑制闸:防「失败→回队→effect 立即重投」空转,
  // 新帧到达/running 变化/退避到点/用户再次发送时解除
  const flushBlockedRef = useRef(false);
  const retryTimer = useRef(0);
  const retryStep = useRef(0);
  // 补投 effect 的显式重跑信号(退避定时器只能改 ref,得有个 state 推一把)
  const [flushTick, setFlushTick] = useState(0);
  const uploadSeqRef = useRef(0);
  const errorTimer = useRef(0);

  // 「当前这份 composer 状态归属哪个会话」。必须是 state 而不是 ref:切会话
  // 那一帧里 sessionId 已经换新,而 draft/queue/atts 还是上一个会话的值——
  // 留档-恢复 effect 的 setState 要下一次渲染才回流,可补投 effect 就排在它
  // 后面、同一次提交里跑,拿到的正是「旧 queue + 新 sessionId」,于是
  // sessionSend(新会话, 旧文本)(send() 早有 forSid/activeRef 纪元守卫,
  // 唯独这条自动路径漏了)。恢复 effect 落位时与 draft/queue 同批提交,
  // 补投 effect 因它变化重新起跑,不靠"下一次恰好有别的依赖变"。
  const [stateSid, setStateSid] = useState(sessionId);
  const composerReady = stateSid === sessionId;

  const clearRetry = useCallback(() => {
    window.clearTimeout(retryTimer.current);
    retryTimer.current = 0;
    retryStep.current = 0;
  }, []);
  const scheduleRetry = useCallback(() => {
    const delay = FLUSH_RETRY_MS[retryStep.current];
    if (delay === undefined) return; // 退避耗尽:停手,等帧/轮次/用户再发
    retryStep.current += 1;
    window.clearTimeout(retryTimer.current);
    retryTimer.current = window.setTimeout(() => {
      flushBlockedRef.current = false;
      setFlushTick((n) => n + 1);
    }, delay);
  }, []);

  // 编辑面快照(留档用):cleanup 时拿到的是最后一次已提交状态
  const snapRef = useRef<{ draft: string; queue: QueueItem[]; atts: ComposerAtt[]; paused: boolean }>({
    draft: "",
    queue: [],
    atts: [],
    paused: false,
  });
  snapRef.current = { draft, queue, atts, paused };
  // 当前活跃会话(迟到的发送回执按它守卫,不污染切换后的会话)
  const activeRef = useRef(sessionId);
  activeRef.current = sessionId;

  // 切会话 = 先留档再恢复(草稿/队列/附件按 sid 暂存,切回不丢;上传中列表
  // 是瞬态不入档,在途收尾回调按 id 过滤,清空后的 filter/map 无害)。
  // 留档挂在 cleanup:切走与卸载(关视图/进设置)统一走同一条路径。
  // 持久化(跨重启)优先于内存 stash:stash 是切会话瞬态留档,可能比磁盘旧。
  useEffect(() => {
    const persisted = readComposerQueue(sessionId);
    const entry = persisted ?? stashGet(sessionId);
    setDraft(entry?.draft ?? "");
    setQueue(entry?.queue ? [...entry.queue] : []);
    setPaused(entry?.paused ?? false);
    setAtts(entry?.atts ? [...entry.atts] : []);
    setUploads([]);
    setError(null);
    setStateSid(sessionId); // 与上面几个 setState 同批提交:补投 effect 据此放行
    sendingRef.current = false;
    flushBlockedRef.current = false;
    clearRetry();
    // 登记活动队列槽:后台补投失败且人恰好切进来时,消息回到这里
    const unbind = bindActiveComposer(sessionId, (text) => {
      if (snapRef.current.queue.some((x) => x.text === text)) return false; // 已有同文则让位
      setQueue((cur) => [...cur, { id: newInstrId(), text, atts: [], state: "pending" }]);
      return true;
    });
    return () => {
      unbind();
      stashSet(sessionId, snapRef.current);
      // 队列持久化到 localStorage(跨重启):仅当队列非空/暂停才算"有东西要留",
      // 否则移除键。注意:空队列(含仅草稿)不写盘——既贴合"清空即移除"语义,
      // 也避免普通卸载把空档写入盘、污染同 id 会话的下一次挂载(测试/复用隔离)。
      const snap = snapRef.current;
      writeComposerQueue(
        sessionId,
        snap.queue.length > 0 || snap.paused
          ? { queue: snap.queue, paused: snap.paused, draft: snap.draft, atts: snap.atts }
          : null,
      );
      clearRetry();
    };
  }, [sessionId, clearRetry]);

  useEffect(() => () => window.clearTimeout(errorTimer.current), []);

  // 帧水位抬升 = 壳已经把上一条上行物化成帧(user-input 回显 + task-started),
  // 这才是"上行落地"的可信信号。此前是 session_send 的 Promise resolve 就摘
  // 在途标记,可壳在**引擎 ack** 时就返回、回显帧还要 ~30ms 才批量推回:
  // 这段真空里 running 仍是 false、sendingRef 也已归零,紧跟着的第二条会
  // **直发**,撞上壳的忙碌守卫(driver/session.rs 「当前会话已有任务在执行」),
  // catch 静默把草稿放回输入框——用户看到的是"消息自己跳回来了"。
  // 顺带解除失败抑制:新帧到达说明这条通道还活着(旧 UI 每批帧都重投一次)。
  useEffect(() => {
    sendingRef.current = false;
    flushBlockedRef.current = false;
    clearRetry();
  }, [lastSeq, clearRetry]);
  // 空转兜底:测试/某些壳路径中 sessionSend 的 Promise 会真正 resolve(上行
  // 已被壳接收),但**不产任何帧**(无回显帧抬升 lastSeq),于是上面的帧水位
  // effect 永不触发,sendingRef 永久锁死 → 队列里的指令再也不投、chip 钉死。
  // 官方新版靠真实回显帧解在途;这里补一道"在途标记终会自己松绑"的闸:
  // 投出后短暂延时若在途标记仍在(真空窗口本应短促),主动松绑并触发补投,
  // 绝不空转。退避机制(flushBlockedRef)仍在失败路径生效,不会无限重投。
  useEffect(() => {
    if (!sendingRef.current) return;
    const t = window.setTimeout(() => {
      if (sendingRef.current) {
        sendingRef.current = false;
        setFlushTick((n) => n + 1);
      }
    }, 80);
    return () => window.clearTimeout(t);
  }, [flushTick, running, queue, lastSeq, paused, composerReady, historyLoaded]);

  const notifyError = useCallback((message: string) => {
    setError(message);
    window.clearTimeout(errorTimer.current);
    errorTimer.current = window.setTimeout(() => setError(null), ERROR_TTL_MS);
  }, []);

  const dismissError = useCallback(() => {
    window.clearTimeout(errorTimer.current);
    setError(null);
  }, []);

  const send = useCallback((): boolean => {
    const text = [draft.trim(), ...atts.map(attLine)].filter(Boolean).join("\n");
    if (!text) return false;
    // /compact 是控制指令不是消息:直达壳的 session_call,不得进排队槽
    // (排队会在轮后把「/compact」当普通文本发给模型)。忙时外显错误并留
    // 住草稿;接受后不乐观落帧——压缩生命周期由壳外显(task_started +
    // 实时 compact_status(started) → task_ended)。reject ⟺ 压缩没起来
    // (忙碌/旧引擎无能力/会话未打开),走 ErrorBar;开轮后的失败壳按
    // user-input 同契约经 task-error 帧收进对话流,不再 reject。
    if (draft.trim() === "/compact") {
      // 不论是否带附件都走直达壳:附件对压缩无意义,且若带附件 fall-through
      // 会把字面量 "/compact" 当普通消息进队列投给模型(测试已覆盖此回归)
      if (running || sendingRef.current || queue.length > 0) {
        notifyError(t("chat.compact.busy"));
        return false;
      }
      setDraft("");
      setAtts([]);
      void sessionCompact(sessionId).catch((e: unknown) => {
        notifyError(t("chat.compact.failed", { reason: e instanceof Error ? e.message : String(e) }));
      });
      return true;
    }
    // 忙/在途/还有队列 → 追加进指令队列(连同附件以附件行并入正文),
    // 轮结束按序自动补投;用户主动再发也解除失败抑制,flush effect 在空闲时立即补投。
    // 附件不在队首(执行中项)保留,只作为当下这条新指令的附件随它一起投出;
    // 因此这里统一并入指令文本,不再单存 atts 列表(官方新版附件亦只走正文)。
    if (running || sendingRef.current || queue.length > 0) {
      flushBlockedRef.current = false;
      clearRetry();
      // 指令连同附件一起入队(附件行随指令拼装正文,见 flush effect);
      // 仅当本轮是"手动直发回合"时附件才走下面的直发路径。
      const item: QueueItem = { id: newInstrId(), text, atts: [...atts], state: "pending" };
      setQueue((cur) => [...cur, item]);
      setDraft("");
      setAtts([]);
      return true;
    }
    sendingRef.current = true;
    const forSid = sessionId;
    const prevDraft = draft;
    const prevAtts = atts;
    setDraft("");
    setAtts([]);
    // 成功路径**不摘** sendingRef:壳在引擎 ack 时就返回,回显帧还在路上,
    // 这段真空里摘掉标记会让下一条直发撞忙碌守卫(见上面的帧水位 effect)
    void sessionSend(sessionId, "user-input", { content: b64encode(text) })
      .catch((e: unknown) => {
        sendingRef.current = false;
        // 失败不丢草稿:文本回输入框、附件回 chips(壳契约 Err ⟺ 未入会话)。
        // 回执迟到且人已切走 → 回原会话留档,不污染当前会话(纪元守卫)
        if (activeRef.current !== forSid) {
          const prev = stashGet(forSid);
          stashSet(forSid, {
            draft: prev?.draft || prevDraft,
            queue: prev?.queue ?? [],
            atts: prev?.atts.length ? prev.atts : prevAtts,
          });
          return;
        }
        // 期间用户已敲了新内容/新附件则让位,不覆盖
        setDraft((cur) => (cur ? cur : prevDraft));
        setAtts((cur) => (cur.length ? cur : prevAtts));
        // **必须外显**:只回滚草稿的话,用户看到的是"输入框先清空、片刻后原文
        // 又跳回来",界面一句解释都没有——分不清是自己手滑还是引擎出了问题,
        // 只能反复重试,而重试同样静默失败。壳侧 Err 分支是实打实的:
        // driver/session.rs 的 ensure_engine_ready / 「会话未打开」/
        // 「当前会话已有任务在执行」,引擎重启后与会话恢复失败后最容易撞上。
        // ErrorBar 正是本文件其它失败路径(上传、切模型/档位/权限模式)统一
        // 使用的外显通道;云端侧 CloudTaskView 也早就渲染了 sendFailed
        notifyError(t("chat.sendFailed", { reason: e instanceof Error ? e.message : String(e) }));
      });
    return true;
  }, [draft, atts, queue, running, sessionId, clearRetry]);

  // 当前正在执行(已投出)的队列指令:回合结束(running→false)时据它判成败
  const inFlightRef = useRef<QueueItem | null>(null);
  // 本轮是否由队列投出(区分「手动直发回合」与「队列投出回合」)
  const deliveredTurnRef = useRef(false);
  // 本轮是否已真正开轮(task-started 把 running 置真)。仅当开轮后才认
  // "running 回落 = 本轮结束":避免上行回显帧(先令 running 假)早于真正开轮
  // 到达,把刚投出的指令误判成"失败"。壳 rejected 由 .catch 单独处理。
  const turnStartedRef = useRef(false);

  // 指令队列补投:轮结束(running 变 false)且无在途上行时投出队头下一条。
  // 重跑时机 = running 变化 / 队列内容变化 / 新一批帧(lastSeq)/ 退避到点
  // (flushTick)/ 会话状态归位(composerReady)/ 暂停切换 / 轮次结束边沿(turnEnded)。
  //
  // 帧水位(lastSeq)变化会重跑本 effect——这是关键:壳在引擎 ack 时就让
  // sessionSend 的 Promise resolve,但 user-input 回显帧还要 ~30ms 才批量推回。
  // 这段真空里 running 仍是 false、可 sendingRef 还被置着(见下方投出处),
  // 紧跟着的第二条会"直发"撞壳的忙碌守卫。旧 UI 在「每批帧到达」都重投一次,
  // 这里借 lastSeq 抬升重跑 effect,把在途标记摘掉,让队列补投得以继续——
  // 成功路径不乐观摘 sendingRef(见投出处注释),全靠这一道帧水位闸兜底。
  useEffect(() => {
    if (running) {
      // 开轮 = 上一条上行已被壳接收;失败抑制也随轮次推进解除
      turnStartedRef.current = true;
      sendingRef.current = false;
      flushBlockedRef.current = false;
      clearRetry();
      return;
    }
    if (!historyLoaded || !composerReady || sendingRef.current || flushBlockedRef.current) return;

    // 本轮是队列投出的回合:轮已真正开过、且已结束(running 回落),判定成功/失败。
    // 未开轮就回落(上行回显帧先令 running 假)→ 不是本轮结束,等真正开轮再判。
    if (deliveredTurnRef.current && inFlightRef.current && turnStartedRef.current) {
      deliveredTurnRef.current = false;
      turnStartedRef.current = false;
      const done = inFlightRef.current;
      inFlightRef.current = null;
      if (!turnEnded) {
        // task-error 结束(running false 但 turnEnded 未置):本轮失败 →
        // 该指令标 failed + 自动暂停(防失败后疯狂自动补投),停手等用户重试/移除
        setQueue((cur) => cur.map((x) => (x.id === done.id ? { ...x, state: "failed" } : x)));
        setPaused(true);
        flushBlockedRef.current = true;
        return;
      }
      // task-ended 正常结束:该项执行成功,出队(return 让 effect 用新队列
      // 重跑,自动投下一条;否则这里会拿旧闭包把已成功的队首再投一遍)
      setQueue((cur) => cur.filter((x) => x.id !== done.id));
      return;
    }

    // 队列非空 + 未暂停 + 队头未失败 → 投出队头下一条
    const next = queue[0];
    if (!next || paused || next.state === "failed") return;
    const forSid = sessionId;
    inFlightRef.current = next;
    deliveredTurnRef.current = true;
    sendingRef.current = true;
    // 标记执行中(队首):UI 据此锁住拖动/删除/编辑,其余待发送项可排序
    setQueue((cur) => cur.map((x) => (x.id === next.id ? { ...x, state: "executing" } : x)));
    // 正文 = 指令文本 + 附件行(与 send() 同一拼法)
    const payload = [next.text, ...next.atts.map(attLine)].filter(Boolean).join("\n");
    void sessionSend(sessionId, "user-input", { content: b64encode(payload) }).catch(() => {
      sendingRef.current = false;
      flushBlockedRef.current = true;
      inFlightRef.current = null;
      deliveredTurnRef.current = false;
      // 发送被壳拒(消息未入会话):该指令标 failed + 自动暂停,等用户重试/移除
      setQueue((cur) => cur.map((x) => (x.id === next.id ? { ...x, state: "failed" } : x)));
      setPaused(true);
      // 回执迟到且人已切走 → 回原会话暂存(deliverQueued 接手后台补投)
      if (activeRef.current !== forSid) {
        const prev = stashGet(forSid);
        stashSet(forSid, { draft: prev?.draft ?? "", queue: prev?.queue ?? [], atts: prev?.atts ?? [] });
        return;
      }
      scheduleRetry();
    });
  }, [running, queue, paused, sessionId, historyLoaded, composerReady, lastSeq, flushTick, clearRetry, scheduleRetry, turnEnded]);

  const retryInstr = useCallback((id: string) => {
    flushBlockedRef.current = false;
    setQueue((cur) => cur.map((x) => (x.id === id ? { ...x, state: "pending" } : x)));
    setPaused(false);
    setFlushTick((n) => n + 1); // 推一把补投 effect
  }, []);
  const removeInstr = useCallback((id: string) => {
    flushBlockedRef.current = false;
    setQueue((cur) => cur.filter((x) => x.id !== id));
  }, []);
  const reorderInstr = useCallback((from: number, to: number) => {
    setQueue((cur) => {
      if (from === to || from < 0 || to < 0 || from >= cur.length || to >= cur.length) return cur;
      const next = [...cur];
      const moved = next.splice(from, 1)[0];
      if (!moved) return cur;
      next.splice(to, 0, moved);
      return next;
    });
  }, []);
  const editInstr = useCallback((id: string, text: string) => {
    setQueue((cur) => cur.map((x) => (x.id === id ? { ...x, text } : x)));
  }, []);
  const clearQueue = useCallback(() => {
    flushBlockedRef.current = false;
    setQueue((cur) => cur.filter((x) => x.state === "failed" || x.state === "executing")); // 只清待发送,执行中/失败项留给用户处置
  }, []);
  // 暂停态 ref:启动(解除暂停)时据此判断是否要「失败项回队」
  const pausedRef = useRef(paused);
  pausedRef.current = paused;
  const togglePaused = useCallback(() => {
    const next = !pausedRef.current;
    setPaused(next);
    // 启动:失败项自动回队标 pending,并推一把补投——
    // 任务停止/失败后点「启动」应直接重新排队继续,而不是停在重试态
    if (!next) {
      flushBlockedRef.current = false;
      setQueue((cur) => cur.map((x) => (x.state === "failed" ? { ...x, state: "pending" } : x)));
      setFlushTick((n) => n + 1);
    }
  }, []);
  const toggleQueueOpen = useCallback(() => setQueueOpen((o) => !o), []);

  const stop = useCallback(() => {
    void sessionSend(sessionId, "user-cancel", {}).catch(() => {});
  }, [sessionId]);

  /** 上传一个来源并入列附件;失败外显、不阻断后续文件。 */
  const uploadOne = useCallback(
    async (run: (onProgress: (sent: number, total: number) => void, signal: AbortSignal) => Promise<{ path: string }>, name: string, indeterminate: boolean, fallbackIsImage: boolean) => {
      const id = ++uploadSeqRef.current;
      const forSid = sessionId;
      const ctl = new AbortController();
      setUploads((list) => [
        ...list,
        {
          id,
          // 空名兜底(旧 UI useSession.ts `f.name || "文件"`):剪贴板截图可为
          // 空名(uploads.ts 头注),不兜底的话上传中的 chip 就是一枚只有
          // spinner + 百分比、没有任何文字的 badge——大图分块要传数秒,这
          // 几秒里用户看不出这是什么。**只兜显示名**:下面成品附件仍优先
          // 用真实路径末段(比"未命名文件"信息量大),两者不共用一个值。
          name: name || t("common.unnamedFile"),
          pct: indeterminate ? -1 : 0,
          ...(indeterminate ? {} : { cancel: () => ctl.abort() }),
        },
      ]);
      try {
        const { path } = await run((sent, total) => {
          // 封顶 99:最后一块落地后还有 finish(改名)在途,100% 由出列表达
          const pct = total > 0 ? Math.min(99, Math.floor((sent / total) * 100)) : 99;
          setUploads((list) => list.map((u) => (u.id === id ? { ...u, pct } : u)));
        }, ctl.signal);
        const att: ComposerAtt = {
          path,
          name: name || path.split("/").pop() || "file",
          isImage: fallbackIsImage || isImagePath(path),
        };
        // 大文件上传耗时可观(数秒),期间完全可能已切会话:附件只归原会话。
        // 不守卫的话它会落进**当前**会话的 composer,而 path 是按旧工作区
        // 算的相对路径——附件行发出去模型根本读不到那个文件(旧 UI
        // useSession.ts:555-571 同款纪元守卫)
        if (activeRef.current === forSid) {
          setAtts((list) => [...list, att]);
        } else {
          const prev = stashGet(forSid);
          stashSet(forSid, {
            draft: prev?.draft ?? "",
            queue: prev?.queue ?? [],
            atts: [...(prev?.atts ?? []), att],
          });
        }
      } catch (e) {
        if (!ctl.signal.aborted) {
          notifyError(t("chat.uploadFailed", { reason: e instanceof Error ? e.message : String(e) }));
        }
      } finally {
        setUploads((list) => list.filter((u) => u.id !== id));
      }
    },
    [notifyError, sessionId],
  );

  const addFiles = useCallback(
    async (files: File[]) => {
      for (const f of files) {
        const native = nativePathOf(f);
        await uploadOne(
          (onProgress, signal) =>
            native
              ? uploadFilePath(sessionId, native)
              : uploadFileStream(sessionId, f, { onProgress, signal }),
          f.name,
          !!native || f.size === 0,
          f.type.startsWith("image/"),
        );
      }
    },
    [sessionId, uploadOne],
  );

  const addPaths = useCallback(
    async (paths: string[]) => {
      for (const p of paths) {
        const name = p.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || p;
        await uploadOne(() => uploadFilePath(sessionId, p), name, true, false);
      }
    },
    [sessionId, uploadOne],
  );

  const removeAtt = useCallback((index: number) => {
    setAtts((list) => list.filter((_, i) => i !== index));
  }, []);

  return useMemo(
    () => ({
      draft,
      setDraft,
      queue,
      queueOpen,
      toggleQueueOpen,
      paused,
      togglePaused,
      retryInstr,
      removeInstr,
      reorderInstr,
      editInstr,
      clearQueue,
      atts,
      removeAtt,
      uploads,
      error,
      dismissError,
      notifyError,
      send,
      stop,
      addFiles,
      addPaths,
    }),
    [
      draft,
      queue,
      queueOpen,
      paused,
      toggleQueueOpen,
      togglePaused,
      retryInstr,
      removeInstr,
      reorderInstr,
      editInstr,
      clearQueue,
      atts,
      removeAtt,
      uploads,
      error,
      dismissError,
      notifyError,
      send,
      stop,
      addFiles,
      addPaths,
    ],
  );
}
