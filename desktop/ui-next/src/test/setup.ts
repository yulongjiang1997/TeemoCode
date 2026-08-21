// dom project 全局测试设置:视图测试固定中文 locale(断言以中文文案为锚,
// happy-dom 的 navigator.language 是 en-US,不钉住会整体漂到英文)。
// composer stash 是模块级每会话留档:RTL 卸载清理会把用例留下的草稿写档,
// 同文件下一个用例挂载同 id 会话即串档——每例后清空隔离。
// 指令队列也持久化到 localStorage(跨重启);卸载清理同样会写盘,下一个用例
// 挂载同 id 会话若只读盘会串档——每例后一并清空 localStorage 隔离。
import { afterEach, beforeEach } from "vitest";

import { resetStashForTests } from "@/features/chat/composer/stash";
import { setLocale } from "@/lib/i18n";

beforeEach(() => {
  setLocale("zh-CN");
});

afterEach(() => {
  resetStashForTests();
  try {
    localStorage.clear();
  } catch {
    // 无 localStorage 环境(jest 等)忽略
  }
});
