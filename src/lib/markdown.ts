// Renders model output as Markdown. Output is untrusted: the HTML is sanitized
// (no images, SVG, MathML, frames, forms or styles; links only to http(s) and
// mailto), remote images become a link you choose to open, and links open in
// the default browser instead of inside SparkBox.
import DOMPurify from "dompurify";
import { Marked, Renderer } from "marked";
import { openUrl } from "@tauri-apps/plugin-opener";

function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

const SAFE_URL = /^(?:https?:|mailto:)/i;

const renderer = new Renderer();
renderer.code = ({ text, lang }) =>
  `<div class="code-block"><div class="code-head"><span>${escapeHtml(lang ?? "")}</span>` +
  `<button type="button" class="code-copy">Copy</button></div><pre><code>${escapeHtml(text)}</code></pre></div>`;
// A remote image would load (and could track) the moment the reply renders; show a link instead.
renderer.image = ({ href, text }) => {
  const label = escapeHtml(text || "image");
  if (!SAFE_URL.test(href ?? "")) return `<span class="md-img-blocked">[Image: ${label}]</span>`;
  return `<a class="md-img" href="${escapeHtml(href)}" title="Opens ${escapeHtml(href)} in your browser">Image: ${label}</a>`;
};

const md = new Marked({ renderer, gfm: true });

const PURIFY = {
  FORBID_TAGS: ["img", "svg", "math", "iframe", "form", "style", "input", "button", "textarea", "select", "video", "audio", "object", "embed"],
  FORBID_ATTR: ["style", "srcset", "src", "action", "formaction"],
  ALLOWED_URI_REGEXP: SAFE_URL,
};

export function renderMarkdown(src: string): string {
  const html = md.parse(src, { async: false }) as string;
  // Copy buttons are SparkBox's own; put them back after sanitizing (button is forbidden in model output).
  const clean = DOMPurify.sanitize(html.replace(/<button type="button" class="code-copy">Copy<\/button>/g, "<span data-copy></span>"), PURIFY) as string;
  return clean.replace(/<span data-copy=""><\/span>/g, '<button type="button" class="code-copy">Copy</button>');
}

/** Click handler for rendered Markdown: copy buttons and external links. */
export function onMarkdownClick(e: React.MouseEvent) {
  const target = e.target as HTMLElement;
  const copy = target.closest(".code-copy");
  if (copy) {
    const code = copy.closest(".code-block")?.querySelector("code")?.textContent ?? "";
    navigator.clipboard.writeText(code).then(() => {
      copy.textContent = "Copied";
      setTimeout(() => (copy.textContent = "Copy"), 1200);
    });
    return;
  }
  const link = target.closest("a");
  if (link) {
    e.preventDefault();
    const href = link.getAttribute("href") ?? "";
    if (SAFE_URL.test(href)) openUrl(href).catch(() => {});
  }
}

export async function copyText(text: string) {
  await navigator.clipboard.writeText(text);
}
