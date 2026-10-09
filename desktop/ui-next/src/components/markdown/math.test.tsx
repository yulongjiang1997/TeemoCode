import { render, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { Markdown } from "./Markdown";

describe("KaTeX", () => {
  it("不把货币 $100 当公式,独立 $x$ 才渲染", async () => {
    render(<Markdown source={"价格 $100,公式 $x$"} />);
    expect(document.body.textContent).toContain("$100");
    await waitFor(() => expect(document.querySelector(".katex")).toBeTruthy());
    expect(document.body.textContent).toContain("$100");
    expect(document.querySelector(".katex-mathml")?.textContent).toContain("x");
  });
});
