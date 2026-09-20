import { describe, expect, it } from "vitest";

import { findPlainPaths, isAbsFilePath, looksLikeFilePath, looksLikeRelFilePath, trimPathCandidate } from "./filePaths";

describe("filePaths", () => {
  it("识别盘符/UNC/Unix 绝对路径", () => {
    expect(isAbsFilePath(String.raw`D:\works\ziji\MonkeyCode\desktop\src\git.rs`)).toBe(true);
    expect(isAbsFilePath("/home/u/proj/src/main.rs")).toBe(true);
    expect(isAbsFilePath(String.raw`\\wsl$\Ubuntu\home\u\a.ts`)).toBe(true);
    expect(isAbsFilePath("src/git.rs")).toBe(false);
  });

  it("相对路径要有扩展名", () => {
    expect(looksLikeRelFilePath("desktop/src/git.rs")).toBe(true);
    expect(looksLikeRelFilePath("src/lib")).toBe(false);
    expect(looksLikeRelFilePath("https://example.com/a.ts")).toBe(false);
  });

  it("整段围栏:排除命令行", () => {
    expect(looksLikeFilePath("git status")).toBe(false);
    expect(looksLikeFilePath("desktop/src/git.rs")).toBe(true);
    expect(looksLikeFilePath("npm install foo/bar")).toBe(false);
  });

  it("正文里抽出路径并剥掉句末标点", () => {
    const hits = findPlainPaths("改了 desktop/src/git.rs。以及 D:\\works\\a.ts,");
    expect(hits.map((h) => h.path)).toEqual(["desktop/src/git.rs", String.raw`D:\works\a.ts`]);
    expect(trimPathCandidate("foo/bar.ts。")).toBe("foo/bar.ts");
  });

  it("不把 http(s) URL 当成本地路径", () => {
    expect(findPlainPaths("见 https://example.com/foo/bar.ts 文档")).toEqual([]);
  });
});
