import { describe, expect, it } from "vitest";
import { renderMarkdown } from "./markdown";

describe("renderMarkdown", () => {
  it("never renders a remote image, only a link to it", () => {
    const html = renderMarkdown("![x](https://a/b)");
    expect(html).not.toContain("<img");
    expect(html).toContain('href="https://a/b"');
    expect(html).toContain("Image: x");
  });

  it("drops raw HTML that could load or run something", () => {
    const html = renderMarkdown(
      'Hi <img src=x onerror="alert(1)"> <svg onload="alert(1)"></svg> <iframe src="https://e"></iframe>' +
        '<form action="https://e"><input name=a></form><style>body{display:none}</style><math><mi>x</mi></math>',
    );
    for (const tag of ["<img", "<svg", "<iframe", "<form", "<input", "<style", "<math", "onerror", "onload"]) {
      expect(html).not.toContain(tag);
    }
    expect(html).toContain("Hi");
  });

  it("keeps only http(s) and mailto links", () => {
    expect(renderMarkdown("[ok](https://example.com)")).toContain('href="https://example.com"');
    expect(renderMarkdown("[mail](mailto:a@b.c)")).toContain('href="mailto:a@b.c"');
    for (const bad of ["javascript:alert(1)", "data:text/html,hi", "file:///etc/passwd", "tauri://localhost"]) {
      expect(renderMarkdown(`[x](${bad})`)).not.toContain("href=");
    }
  });

  it("keeps code blocks and their copy button", () => {
    const html = renderMarkdown("```bash\necho <hi>\n```");
    expect(html).toContain('<button type="button" class="code-copy">Copy</button>');
    expect(html).toContain("echo &lt;hi&gt;");
  });

  it("removes style attributes", () => {
    expect(renderMarkdown('<span style="position:fixed">x</span>')).not.toContain("style=");
  });
});
