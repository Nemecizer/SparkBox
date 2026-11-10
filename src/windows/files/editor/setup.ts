// The editor's CodeMirror setup: line numbers, history, find and replace (⌘F, ⌘⌥F),
// go to line (⌘L or ⌃G), bracket matching, visible trailing whitespace, and a theme made of
// SparkBox's tokens so it follows Light and Dark. Theme, language, wrapping and read-only
// sit in compartments, so they change without rebuilding the editor.
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { bracketMatching, foldGutter, foldKeymap, HighlightStyle, indentOnInput, syntaxHighlighting } from "@codemirror/language";
import { gotoLine, highlightSelectionMatches, openSearchPanel, search, searchKeymap } from "@codemirror/search";
import { Compartment, EditorState, type Extension } from "@codemirror/state";
import {
  drawSelection, EditorView, highlightActiveLine, highlightActiveLineGutter, highlightSpecialChars, highlightTrailingWhitespace,
  keymap, lineNumbers, rectangularSelection,
} from "@codemirror/view";
import { tags as t } from "@lezer/highlight";
import { languageExtension, type LanguageId } from "./languages";

export const themeSlot = new Compartment();
export const languageSlot = new Compartment();
export const wrapSlot = new Compartment();
export const readOnlySlot = new Compartment();

/** Colours come from CSS variables (editor.css maps them onto P1's tokens per theme). */
export function editorTheme(dark: boolean): Extension {
  return EditorView.theme(
    {
      "&": { height: "100%", fontSize: "var(--editor-font-size, 12.5px)", color: "var(--text)", backgroundColor: "var(--surface)" },
      ".cm-scroller": { fontFamily: "var(--mono)", lineHeight: "1.5" },
      ".cm-content": { caretColor: "var(--accent)", padding: "6px 0" },
      ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--accent)", borderLeftWidth: "2px" },
      "&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, .cm-selectionBackground, ::selection": {
        backgroundColor: "var(--cm-selection)",
      },
      ".cm-gutters": { backgroundColor: "var(--surface-2)", color: "var(--muted)", borderRight: "1px solid var(--hairline)" },
      ".cm-activeLineGutter": { backgroundColor: "var(--cm-active-line)", color: "var(--text-2)" },
      ".cm-activeLine": { backgroundColor: "var(--cm-active-line)" },
      ".cm-trailingSpace": { backgroundColor: "var(--cm-trailing)" },
      ".cm-matchingBracket, &.cm-focused .cm-matchingBracket": { backgroundColor: "var(--cm-bracket)", outline: "1px solid var(--accent)", color: "inherit" },
      ".cm-nonmatchingBracket": { backgroundColor: "var(--crit-bg)", color: "var(--crit-text)" },
      ".cm-searchMatch": { backgroundColor: "var(--cm-match)", outline: "1px solid var(--warning-border, var(--warning))" },
      ".cm-searchMatch.cm-searchMatch-selected": { backgroundColor: "var(--cm-match-current)" },
      ".cm-selectionMatch": { backgroundColor: "var(--cm-selection-match)" },
      ".cm-panels": { backgroundColor: "var(--chrome)", color: "var(--text)", fontFamily: "var(--font)", fontSize: "var(--fs-sm, 11px)" },
      ".cm-panels.cm-panels-top": { borderBottom: "1px solid var(--hairline)" },
      ".cm-panels.cm-panels-bottom": { borderTop: "1px solid var(--hairline)" },
      ".cm-tooltip": { backgroundColor: "var(--surface)", border: "1px solid var(--border)", borderRadius: "var(--r-md, 6px)", boxShadow: "var(--shadow-pop)" },
      ".cm-diagnostic-error": { borderLeftColor: "var(--critical)" },
      ".cm-diagnostic-warning": { borderLeftColor: "var(--warning)" },
      ".cm-lintRange-error": { backgroundImage: "none", textDecoration: "underline wavy var(--critical)", textUnderlineOffset: "3px" },
      ".cm-lintRange-warning": { backgroundImage: "none", textDecoration: "underline wavy var(--warning)", textUnderlineOffset: "3px" },
      ".cm-foldPlaceholder": { backgroundColor: "var(--surface-3)", border: "1px solid var(--hairline)", color: "var(--text-2)" },
    },
    { dark },
  );
}

const highlight = HighlightStyle.define([
  { tag: [t.keyword, t.operatorKeyword, t.modifier], color: "var(--cm-keyword)" },
  { tag: [t.string, t.special(t.string), t.regexp], color: "var(--cm-string)" },
  { tag: [t.number, t.bool, t.null, t.atom], color: "var(--cm-number)" },
  { tag: [t.comment, t.lineComment, t.blockComment], color: "var(--cm-comment)", fontStyle: "italic" },
  { tag: [t.propertyName, t.attributeName, t.definition(t.propertyName)], color: "var(--cm-property)" },
  { tag: [t.function(t.variableName), t.function(t.propertyName), t.className, t.typeName], color: "var(--cm-function)" },
  { tag: [t.heading, t.strong], fontWeight: "600" },
  { tag: [t.meta, t.processingInstruction, t.labelName], color: "var(--cm-meta)" },
  { tag: [t.variableName, t.self], color: "var(--text)" },
  { tag: [t.punctuation, t.bracket, t.separator], color: "var(--text-2)" },
  { tag: t.invalid, color: "var(--crit-text)" },
]);

/** Opens find with the replace field focused (⌘⌥F). */
function openReplace(view: EditorView): boolean {
  openSearchPanel(view);
  requestAnimationFrame(() => view.dom.querySelector<HTMLInputElement>(".cm-search input[name=replace]")?.focus());
  return true;
}

export interface SetupOptions {
  language: LanguageId;
  dark: boolean;
  wrap: boolean;
  readOnly: boolean;
  onSave?: () => void;
  onChange?: (state: EditorState) => void;
}

export function extensions(o: SetupOptions): Extension[] {
  return [
    lineNumbers(),
    highlightActiveLineGutter(),
    foldGutter(),
    highlightSpecialChars(),
    highlightTrailingWhitespace(),
    history(),
    drawSelection(),
    rectangularSelection(),
    EditorState.allowMultipleSelections.of(true),
    indentOnInput(),
    bracketMatching(),
    highlightActiveLine(),
    highlightSelectionMatches(),
    search({ top: true }),
    syntaxHighlighting(highlight),
    keymap.of([
      { key: "Mod-s", preventDefault: true, run: () => (o.onSave?.(), true) },
      { key: "Mod-l", preventDefault: true, run: gotoLine },
      { key: "Ctrl-g", preventDefault: true, run: gotoLine },
      { key: "Mod-Alt-f", preventDefault: true, run: openReplace },
      ...searchKeymap,
      ...historyKeymap,
      ...foldKeymap,
      indentWithTab,
      ...defaultKeymap,
    ]),
    themeSlot.of(editorTheme(o.dark)),
    languageSlot.of(languageExtension(o.language)),
    wrapSlot.of(o.wrap ? EditorView.lineWrapping : []),
    readOnlySlot.of(EditorState.readOnly.of(o.readOnly)),
    EditorView.updateListener.of((u) => {
      if (u.docChanged || u.selectionSet) o.onChange?.(u.state);
    }),
    EditorView.contentAttributes.of({ "aria-label": "File contents", spellcheck: "false", autocorrect: "off", autocapitalize: "off" }),
  ];
}

export { openReplace, openSearchPanel };
