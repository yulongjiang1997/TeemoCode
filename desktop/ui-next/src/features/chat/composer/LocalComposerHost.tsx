// 本地 composer 的状态边界。草稿、附件、上传和错误都只在这个子树更新；
// ChatView/Timeline 不再因为 textarea 每个按键重渲。
import { forwardRef, useEffect, useImperativeHandle, useMemo, useRef, useState } from "react";

import type { SessionMeta } from "@/lib/ipc/sessions";
import type { ChatState } from "@/lib/protocol/types";
import { readTeamMode, writeTeamMode } from "@/lib/util/prefs";
import { Composer, composerPresentationOf, type ComposerInputHandle } from "./Composer";
import { useComposer } from "./useComposer";
import type { QueueItem } from "./useComposer";

export interface LocalComposerHandle {
  addFiles(files: File[]): Promise<void>;
  notifyError(message: string): void;
  focus(): void;
  restorePersisted(items: QueueItem[]): void;
}

export const LocalComposerHost = forwardRef<
  LocalComposerHandle,
  {
    sessionId: string;
    state: ChatState;
    historyLoaded: boolean;
    meta: SessionMeta;
    onAfterSend?: () => void;
    focusRequest?: number;
    onFocusRequestHandled?: (request: number) => void;
  }
>(function LocalComposerHost(
  { sessionId, state, historyLoaded, meta, onAfterSend, focusRequest, onFocusRequestHandled },
  ref,
) {
  // 团队模式(按会话):开启后发送任务注入团队编排指令
  const [teamOn, setTeamOn] = useState(() => readTeamMode(sessionId));
  useEffect(() => setTeamOn(readTeamMode(sessionId)), [sessionId]);
  const enabledSkills = meta.skills ?? [];
  const ctl = useComposer(sessionId, { running: state.running, historyLoaded, lastSeq: state.lastSeq, turnEnded: state.turnEnded, teamOn, enabledSkills });
  const inputRef = useRef<ComposerInputHandle>(null);
  const presentation = useMemo(() => composerPresentationOf(state), [state]);

  // addFiles/notifyError 都是 useCallback，逐键草稿更新不会改变句柄；切会话
  // 时 React 会原子替换为新 ctl，上传迟到回调仍由 useComposer 纪元守卫。
  useImperativeHandle(
    ref,
    () => ({
      addFiles: ctl.addFiles,
      notifyError: ctl.notifyError,
      focus: () => inputRef.current?.focus(),
      restorePersisted: ctl.restorePersisted,
    }),
    [ctl.addFiles, ctl.notifyError, ctl.restorePersisted],
  );

  const toggleTeamMode = (next: boolean) => {
    setTeamOn(next);
    writeTeamMode(sessionId, next);
  };

  return (
    <Composer
      ref={inputRef}
      sessionId={sessionId}
      presentation={presentation}
      meta={meta}
      ctl={ctl}
      teamOn={teamOn}
      toggleTeamMode={toggleTeamMode}
      onAfterSend={onAfterSend}
      focusRequest={focusRequest}
      onFocusRequestHandled={onFocusRequestHandled}
    />
  );
});
