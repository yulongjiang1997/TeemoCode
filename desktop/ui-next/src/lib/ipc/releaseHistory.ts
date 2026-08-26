// 版本更新记录(编译期嵌入二进制,include_str! 引用)。
//
// 为什么本地维护:历史版本内容是静态事实,放云端清单只会让 latest.json
// 越长越肥,且每次发版都要把上一条从 platforms.notes 挪进 history(容易漏,
// 漏一次 UI 的「版本历史」就断档)。内置文件随应用走,永不丢;云端只承担
// 「最新一版的 notes」这一个动态字段。
//
// 维护规则:发新版时在数组头部追加一条 {version, notes},与 Cargo.toml /
// tauri.conf.json 的 version 同步改(release_v0_1_x.py 已覆盖版本号,notes
// 需手动同步到这里)。UI 只展示不晚于当前安装版本的条目。
export interface ReleaseNote {
  version: string;
  notes: string;
}

export const RELEASE_HISTORY: ReleaseNote[] = [
  {
    version: "0.1.19",
    notes: "修复 0.1.17/0.1.18 点「更新」报 Command update_download not found 导致无法应用内更新的问题",
  },
  {
    version: "0.1.18",
    notes: "模型下拉菜单视觉优化(宽松行距/选中勾选标记) + 用量统计 token 不增长修复",
  },
  {
    version: "0.1.17",
    notes:
      "会员模型/锁定模型(未解锁)也显示删除按钮(hover 提示移除语义) + 修复队列持久化清空后重复投出问题 + 模型连通性测试按钮与折叠行状态徽标 + 大模型配置支持拉取网关模型列表 + Markdown 路径代码块加「打开」按钮 + 恢复上下文用量悬浮窗「压缩上下文」按钮",
  },
  {
    version: "0.1.16",
    notes: "恢复「队列持久化」按钮 + 队列行为修正(运行中追加不再误标 executing、executing 完成后正常移除) + 提问大纲滚动定位与 per-turn token 用量 + session-usage 事件发送 + 自动压缩阈值保存修复",
  },
  {
    version: "0.1.15",
    notes: "修托盘右键菜单中文乱码(显示窗口/设置/重启引擎/检查更新/退出 TeemoCode) + 补齐账号 UI 编译错误",
  },
  {
    version: "0.1.14",
    notes: "本地任务 Git 上传/导入(git_push/git_import) + 任务数据三目录迁移 + 上传自动打包任务会话数据一起提交推送 + 创建本地任务支持选空目录/Git导入项目 + Git菜单向上弹出",
  },
];
