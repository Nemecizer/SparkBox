import DOMPurify from "dompurify";
import { Marked, Renderer } from "marked";
import { sourceUrl } from "./library";

const esc = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
/** No HTML, embedded media, app commands, or executable action data from a document. */
export function renderGuide(text: string, revision: string, path: string): string {
  const renderer = new Renderer();
  renderer.html = () => "";
  renderer.image = ({ href, text }) => `<p>[Image: ${esc(text)} — <a href="${esc(resolve(href, revision, path))}">open source image</a>]</p>`;
  renderer.link = ({ href, text }) => `<a href="${esc(resolve(href, revision, path))}">${esc(text)}</a>`;
  renderer.code = ({ text, lang }) => `<div class="code-block"><div class="code-head"><span>${esc(lang ?? "")}</span><span class="howto-copy-placeholder"></span></div><pre><code>${esc(text)}</code></pre></div>`;
  const parsed = new Marked({ renderer, gfm: true }).parse(text, { async: false }) as string;
  const safe = DOMPurify.sanitize(parsed, { ALLOWED_TAGS: ["h1", "h2", "h3", "h4", "h5", "h6", "p", "a", "ul", "ol", "li", "strong", "em", "del", "blockquote", "pre", "code", "table", "thead", "tbody", "tr", "th", "td", "hr", "br", "div", "span"], ALLOWED_ATTR: ["href", "class"], ALLOWED_URI_REGEXP: /^https?:/i, ALLOW_DATA_ATTR: false });
  return safe.replace(/<span class="howto-copy-placeholder"><\/span>/g, '<button type="button" class="code-copy">Copy</button>');
}
export function resolve(href: string, revision: string, path: string): string {
  // Relative links stay at the reviewed commit. Privileged app schemes never survive.
  if (/^[a-z][a-z\d+.-]*:/i.test(href)) return /^https?:\/\//i.test(href) ? href : "";
  if (href.startsWith("//") || href.includes("\\")) return "";
  try { return new URL(href, sourceUrl(revision, path)).href; } catch { return ""; }
}
