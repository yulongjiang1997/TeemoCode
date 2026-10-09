import "katex/dist/katex.min.css";
import type { MarkedExtension, Tokens } from "marked";

const MATH_SOURCE_MAX = 512;
const MATH_FORMULA_MAX = 256;
const MATH_NEW_FORMULA_MAX = 64;
const MATH_TOTAL_SOURCE_MAX = 16_384;
const MATH_HTML_MAX = 2_000_000;
const BLOCK_MATH_RE = /^(?: {0,3})\$\$([\s\S]+?)\$\$(?!\$)[ \t]*(?:\n+|$)|^(?: {0,3})\\\[([\s\S]+?)\\\][ \t]*(?:\n+|$)/;
const INLINE_DOLLAR_RE = /^\$\$([^$\n]+?)\$\$(?!\$)|^\$((?:\\.|[^$\n])+?)\$(?!\$)/;
const INLINE_MATH_START_RES = [/\\\[/, /\\\(/];
const MATH_DEFINITION_RE = /\\(?:def|newcommand|renewcommand|DeclareMathOperator|gdef|edef|xdef)\b/;

type KatexRenderer = typeof import("katex").default;
type MathToken = Tokens.Generic & {
  type: "blockMath" | "inlineMath";
  raw: string;
  text: string;
  displayMode: boolean;
  oversize: boolean;
};

let katexModule: Promise<typeof import("katex")> | null = null;
let katexRenderer: KatexRenderer | null = null;
const readyListeners = new Set<() => void>();

function loadKatex(): Promise<typeof import("katex")> {
  if (katexModule) return katexModule;
  const pending = import("katex").then((module) => {
    katexRenderer = module.default;
    for (const listener of readyListeners) listener();
    return module;
  });
  katexModule = pending;
  void pending.catch(() => {
    if (katexModule === pending) katexModule = null;
  });
  return pending;
}

/** KaTeX 就绪后回调。已就绪则同步调用，不触发整篇 Markdown 重渲。 */
export function subscribeMathReady(listener: () => void): () => void {
  if (katexRenderer) {
    listener();
    return () => {};
  }
  readyListeners.add(listener);
  void loadKatex();
  return () => {
    readyListeners.delete(listener);
  };
}

function isInlineDollarBoundary(source: string, index: number): boolean {
  const prev = index > 0 ? source[index - 1] : "";
  return !prev || /[\s([{]/.test(prev);
}

function inlineDollarMathStart(source: string): number | undefined {
  for (let index = 0; index < source.length; index++) {
    if (source[index] !== "$" || !isInlineDollarBoundary(source, index)) continue;
    if (INLINE_DOLLAR_RE.test(source.slice(index))) return index;
  }
  return undefined;
}

function dollarMathToken(source: string, type: "blockMath" | "inlineMath"): MathToken | undefined {
  if (!isInlineDollarBoundary(source, 0)) return undefined;
  const match = INLINE_DOLLAR_RE.exec(source);
  if (!match) return undefined;
  const raw = match[0];
  const text = (match[1] ?? match[2] ?? "").trim();
  const next = source[raw.length] ?? "";
  if (!text || (next && !/[\s.,;:!?)]/.test(next))) return undefined;
  return mathToken(raw, text, Boolean(match[1]), type);
}

function slashMathToken(
  source: string,
  open: string,
  close: string,
  displayMode: boolean,
  type: "blockMath" | "inlineMath",
): MathToken | undefined {
  if (!source.startsWith(open)) return undefined;
  const end = source.indexOf(close, open.length);
  if (end < 0) return undefined;
  const text = source.slice(open.length, end).trim();
  if (!text) return undefined;
  return mathToken(source.slice(0, end + close.length), text, displayMode, type);
}

function mathToken(raw: string, text: string, displayMode: boolean, type: "blockMath" | "inlineMath"): MathToken {
  return {
    type,
    raw,
    text,
    displayMode,
    oversize: text.length > MATH_SOURCE_MAX || MATH_DEFINITION_RE.test(text),
    tokens: [],
  };
}

function escapeHtml(value: string): string {
  return value.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

function mathPlaceholder(token: MathToken): string {
  const mode = token.displayMode ? "block" : "inline";
  const tag = token.displayMode ? "div" : "span";
  const suffix = token.displayMode ? "\n" : "";
  if (token.oversize) {
    return `<${tag} class="md-math md-math-${mode} md-math-source">${escapeHtml(token.raw)}</${tag}>${suffix}`;
  }
  return `<${tag} class="md-math md-math-${mode}" data-md-math="${mode}">${escapeHtml(token.text)}</${tag}>${suffix}`;
}

export function mathExtension(): MarkedExtension {
  return {
    extensions: [
      {
        name: "blockMath",
        level: "block",
        start(src) {
          const index = src.search(/(?:^|\n) {0,3}(?:\$\$|\\\[)/);
          return index < 0 ? undefined : index;
        },
        tokenizer(src) {
          const match = BLOCK_MATH_RE.exec(src);
          if (!match) return undefined;
          const text = (match[1] ?? match[2] ?? "").trim();
          if (!text) return undefined;
          return mathToken(match[0], text, true, "blockMath");
        },
        renderer(token) {
          return mathPlaceholder(token as MathToken);
        },
      },
      {
        name: "inlineMath",
        level: "inline",
        start(src) {
          const indexes = [...INLINE_MATH_START_RES.map((rule) => src.search(rule)), inlineDollarMathStart(src)].filter(
            (index): index is number => index !== undefined && index >= 0,
          );
          return indexes.length ? Math.min(...indexes) : undefined;
        },
        tokenizer(src) {
          return (
            dollarMathToken(src, "inlineMath") ??
            slashMathToken(src, "\\(", "\\)", false, "inlineMath") ??
            slashMathToken(src, "\\[", "\\]", true, "inlineMath")
          );
        },
        renderer(token) {
          return mathPlaceholder(token as MathToken);
        },
      },
    ],
  };
}

function markMathSource(formula: HTMLElement): void {
  formula.removeAttribute("data-md-math");
  formula.classList.add("md-math-source");
}

function paintFormulas(root: ParentNode): void {
  const renderer = katexRenderer;
  if (!renderer) return;
  const formulas = [...root.querySelectorAll<HTMLElement>("[data-md-math]")];
  if (formulas.length > MATH_FORMULA_MAX) {
    for (const formula of formulas) markMathSource(formula);
    return;
  }
  let rendered = 0;
  let sourceChars = 0;
  for (const formula of formulas) {
    const source = formula.textContent ?? "";
    sourceChars += source.length;
    if (
      rendered >= MATH_NEW_FORMULA_MAX ||
      sourceChars > MATH_TOTAL_SOURCE_MAX ||
      source.length > MATH_SOURCE_MAX ||
      MATH_DEFINITION_RE.test(source)
    ) {
      markMathSource(formula);
      continue;
    }
    try {
      formula.innerHTML = renderer.renderToString(source, {
        displayMode: formula.getAttribute("data-md-math") === "block",
        output: "htmlAndMathml",
        strict: "ignore",
        throwOnError: false,
        trust: false,
        maxSize: 20,
        maxExpand: 100,
      });
      formula.removeAttribute("data-md-math");
      formula.setAttribute("data-md-math-rendered", "");
      rendered += 1;
    } catch {
      markMathSource(formula);
    }
  }
}

/** 只改公式占位节点，不替换周围的复制按钮或链接。 */
export function paintMath(root: ParentNode): void {
  if (!root.querySelector("[data-md-math]")) return;
  if (!katexRenderer) {
    void loadKatex();
    return;
  }
  paintFormulas(root);
}

/** 净化之后再注入 KaTeX HTML，避免公式节点被 DOMPurify 拆掉。未加载时保留占位。 */
export function renderMathHtml(html: string): string {
  if (!html.includes("data-md-math=") || html.length > MATH_HTML_MAX) return html;
  if (!katexRenderer) {
    void loadKatex();
    return html;
  }
  const template = document.createElement("template");
  template.innerHTML = html;
  paintFormulas(template.content);
  return template.innerHTML;
}
