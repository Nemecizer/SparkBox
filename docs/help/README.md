# Maintaining SparkBox Help

Help is bundled Markdown. Update the relevant article and tips whenever a visible behavior changes. Follow STYLE.md and INFO-ICONS.md; use American spelling and existing semantic colors.

## Files

- manifest.json: article metadata, contents, window/Settings mapping and verification status.
- Article Markdown: the explanation, steps, errors and related links.
- tips.json: short contextual tips pointing to the articles.
- terms.json and phrases.json: glossary and search vocabulary.
- shots.json and img/: picture recipes and assets for Light and Dark.
- ../../src/help/: rendering, search, interaction and validation code.

Keep article frontmatter consistent with manifest.json. Use help: links for articles and the supported open: links for windows. Mark claims according to actual evidence; Demo is not native or cluster acceptance. Public preparation resets prior native verification labels rather than claiming the export was tested against a new user's hardware.

## Checks

From the repository root:

```sh
VITE_HELP_RELEASE=1 npx vitest run src/help/help.test.ts
npx vitest run src/help
```

The release build requires every declared picture and its Dark counterpart. Accessibility, search, terminology, links and rendering are tested too. Fix the content or recipe; do not disable those checks.

## Public export pictures

The old raster captures containing maintainer details are excluded. Required guide pictures use the sanitized browser Demo. Start `PORT=1739 npm run dev:mock`, then run `node scripts/capture-public-help.mjs` in another terminal. It uses a disposable headless Chrome profile, captures Light and Dark, and closes that browser afterward. Stop the Demo server yourself. Native SparkBox and the cluster are never used.

The optional SPARKBOX_CAPTURE_FORBIDDEN environment variable is a pipe-separated list of text that must not appear in a capture. It is a local review aid; never put private identifiers in a public recipe or commit. Review the generated pictures visually as well.

Generate diagrams with `node tools/agents/docs-diagrams.mjs`. Recipe IDs and captions must match the Markdown. Keep only pictures that accurately describe the current screen.

## Public distribution

All pictured nodes, accounts, addresses and fingerprints are synthetic examples. A fresh native installation has no inventory. Regenerate public-* pictures using scripts/capture-public-help.mjs and a local browser Demo; do not capture the maintainer's live app. See ../DISTRIBUTION-PRIVACY.md for the publication checks.
