import { describe, expect, it } from "vitest";
import { isApplyPatchText, parseApplyPatch } from "./applyPatch";

const PATCH = `*** Begin Patch
*** Update File: src/a.ts
*** Move to: src/b.ts
@@ -1 +1 @@
-old
+new
*** Add File: src/c.ts
+created
*** End Patch`;

describe("apply_patch", () => {
  it("识别信封并拆出文件段", () => {
    expect(isApplyPatchText(PATCH)).toBe(true);
    expect(isApplyPatchText("--- a/x\n+++ b/x\n")).toBe(false);
    expect(parseApplyPatch(PATCH)).toEqual([
      {
        action: "Update",
        path: "src/a.ts",
        movePath: "src/b.ts",
        lines: ["@@ -1 +1 @@", "-old", "+new"],
      },
      { action: "Add", path: "src/c.ts", lines: ["+created"] },
    ]);
  });

  it("解析失败时没有文件段,调用方回退原文", () => {
    expect(parseApplyPatch("*** Begin Patch\n*** End Patch")).toEqual([]);
  });
});
