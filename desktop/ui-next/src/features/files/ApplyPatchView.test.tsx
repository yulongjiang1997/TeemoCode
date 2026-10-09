import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { ApplyPatchView } from "./ApplyPatchView";

describe("ApplyPatchView", () => {
  it("按文件段展示增删行", () => {
    render(
      <ApplyPatchView
        text={"*** Begin Patch\n*** Update File: a.ts\n-old\n+new\n*** End Patch"}
      />,
    );
    expect(screen.getByText("Update File: a.ts")).toBeTruthy();
    expect(screen.getByText("new")).toBeTruthy();
    expect(screen.getByText("old")).toBeTruthy();
  });

  it("没有文件段时展示原文", () => {
    render(<ApplyPatchView text={"*** Begin Patch\n*** End Patch"} />);
    expect(document.querySelector("pre")?.textContent).toContain("*** Begin Patch");
  });
});
