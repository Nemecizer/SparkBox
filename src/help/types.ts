// The shapes of docs/help/manifest.json and tips.json. Types only, so the main bundle can use
// them without pulling the content in.

export type ArticleType = "tutorial" | "howto" | "feature" | "concept" | "reference" | "troubleshooting";

export interface ArticleMeta {
  id: string;
  title: string;
  type: ArticleType;
  area: string;
  file: string;
  summary: string;
  /** true: proven on the owner's cluster. false: test systems only. null: doesn't touch the cluster. */
  verified: boolean | null;
  related: string[];
  keywords: string[];
  proof?: string;
  /** About the test build only. Search keeps it back until the query asks for it. */
  testBuild?: boolean;
}

export interface TocGroup {
  title: string | null;
  /** A concept to read first, shown at the top of the group. It lives in its own section. */
  lead?: string;
  articles: string[];
}

export interface TocSection {
  id: string;
  title: string;
  groups: TocGroup[];
}

export interface HelpMenuEntry {
  label?: string;
  article?: string | null;
  command?: string;
  /** Opens a section's page of articles. */
  section?: string;
  /** Stands for every group and article of this section of the table of contents. */
  toc?: string;
  separator?: boolean;
  accelerator?: string;
  submenu?: HelpMenuEntry[];
}

export interface Manifest {
  calmLine: string;
  notArticles: string[];
  areas: { id: string; title: string }[];
  toc: TocSection[];
  helpMenu: HelpMenuEntry[];
  windowHelp: Record<string, string>;
  settingsHelp: Record<string, string>;
  packages: Record<string, { folder: string }>;
  articles: ArticleMeta[];
}

/** One entry of docs/help/tips.json. */
export interface Tip {
  title: string;
  body: string;
  /** Where Read the full guide lands: an article id, with an optional #heading. */
  article: string;
  /** Up to three How-To ids. Without it, the article's own related How-Tos are used. */
  howto?: string[];
}

/** What an info icon shows, whatever kind of topic it names. */
export interface ResolvedTip {
  title: string;
  /** What the icon explains, for its tooltip and screen-reader name: the tip, or the heading. */
  name: string;
  body: string;
  article: string;
  anchor?: string;
  howtos: { id: string; title: string }[];
}
