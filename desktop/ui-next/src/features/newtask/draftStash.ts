export interface NewTaskDraftAtt {
  file: File;
  name: string;
  preview?: string;
}

export interface NewTaskGitImportResult {
  migrated: number;
  ok: boolean;
  err?: string;
}

/** 新建弹窗切走后的内存草稿。创建成功才清；取消/Esc/卸载都留档。 */
export interface NewTaskDraft {
  kind: "local" | "chat" | "cloud";
  dir: string;
  text: string;
  think: string;
  model: string;
  engine: "ohmy" | "codex";
  isolate: boolean;
  enabledSkills: string[] | null;
  atts: NewTaskDraftAtt[];
  gitImportResult: NewTaskGitImportResult | null;
}

let draft: NewTaskDraft | null = null;

export function readNewTaskDraft(): NewTaskDraft | null {
  return draft;
}

export function saveNewTaskDraft(next: NewTaskDraft): void {
  draft = next;
}

export function clearNewTaskDraft(): void {
  if (draft) {
    for (const att of draft.atts) if (att.preview) URL.revokeObjectURL(att.preview);
  }
  draft = null;
}

export function resetNewTaskDraftForTests(): void {
  draft = null;
}
