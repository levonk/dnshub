---
type: Practice
title: Docs Site Taxonomy — Audience, Status, and Area Custom Fields
description: Extend a docs site's frontmatter with custom taxonomy fields — audience, status, area — that mirror the project's issue/PR labels, enabling filtered views, stale-doc detection, and cross-referencing between docs and code.
tags: [documentation, docs-site, taxonomy, frontmatter, audience, status, area, astro-starlight, docusaurus, mkdocs]
date:
  created: "2026-08-10"
  knowledge-basis: "2026-08-10"
  last-used: "2026-08-10"
sources:
  - id: astro-starlight-docs
    resource: "https://starlight.astro.tools/"
    title: "Astro Starlight Documentation"
  - id: docusaurus-docs
    resource: "https://docusaurus.io/docs"
    title: "Docusaurus Documentation"
---

# Docs Site Taxonomy — Audience, Status, and Area Custom Fields

## Failure Mode

A docs site grows to hundreds of pages. Without a taxonomy, the team
cannot answer basic questions:

- **Who is this page for?** — a page titled "Database Migrations" could be
  for end users (how to run them), developers (how to write them), or
  operators (how to debug them). Without an `audience` field, the page
  tries to serve all three and serves none well.
- **Is this page current?** — a page written two years ago still appears in
  the sidebar with the same prominence as a page written last week.
  Without a `status` field, stale docs are indistinguishable from
  authoritative docs.
- **Which codebase area does this page cover?** — a page describes the
  workflow engine, but there is no link from the page to the
  `packages/workflows/` directory or the `area:workflows` GitHub label.
  Without an `area` field, docs and code live in separate universes.

## Symptoms

- A new contributor asks "which docs are stale?" and no one can answer
  without reading every page.
- A user follows a doc that describes a deprecated workflow and files a
  bug when it does not work.
- The sidebar lists 200 pages with no grouping by audience — an operator
  has to scroll past developer-facing internals to find the deployment
  guide.
- A doc references a code module by name but there is no machine-readable
  link between the doc and the code, so when the module is renamed, the doc
  is not updated.

## Practice

Extend the docs site's frontmatter schema with custom taxonomy fields that
mirror the project's issue/PR labels. Three high-value fields:

### `audience` — who the doc targets

An array field (a doc can target multiple audiences). Common values:

- `user` — end users running the software
- `developer` — people building on or contributing to the software
- `operator` — people deploying and maintaining the software

Use cases: filter the sidebar to show only operator-facing docs; generate
an "onboarding for developers" reading list; warn a user who lands on a
developer-facing page that they may want the user guide instead.

### `status` — doc lifecycle state

An enum field with a default. Common values:

- `current` — up to date and authoritative (default)
- `deprecated` — superseded, kept for reference; render a banner
- `research` — exploratory / not authoritative; render a caution

Use cases: render a "This doc is deprecated, see <link>" banner on
deprecated pages; generate a "stale docs" report listing all non-current
pages; exclude `research` pages from the sidebar by default.

For work-in-progress docs, use the site generator's built-in `draft: true`
frontmatter instead of a `status` value — drafts are excluded from
production builds entirely.

### `area` — package or domain area

An enum field mirroring the project's `area:*` GitHub labels. Common
values: `adapters`, `cli`, `config`, `database`, `orchestrator`, `server`,
`web`, `workflows`, etc.

Use cases: cross-reference docs with code (a doc tagged `area:workflows`
links to `packages/workflows/`); filter docs by area when investigating a
bug in a specific subsystem; generate an area-indexed landing page.

### Schema definition

Define the custom fields in the docs site's content schema so the build
fails on invalid values. All custom fields should be optional (with a
sensible default for `status`) so existing pages do not break when the
taxonomy is introduced.

## Concrete Instances

### Astro Starlight (custom Zod schema)

Starlight's `docsSchema()` accepts an `extend` function to add custom
fields via Zod. The extended schema is defined in `content.config.ts`:

```typescript
import { defineCollection } from 'astro:content';
import { docsLoader } from '@astrojs/starlight/loaders';
import { docsSchema } from '@astrojs/starlight/schema';
import { z } from 'astro/zod';

export const collections = {
  docs: defineCollection({
    loader: docsLoader(),
    schema: docsSchema({
      extend: z.object({
        audience: z
          .array(z.enum(['user', 'developer', 'operator']))
          .optional(),
        status: z
          .enum(['current', 'deprecated', 'research'])
          .default('current'),
        area: z
          .enum(['adapters', 'cli', 'database', 'server', 'web', 'workflows'])
          .optional(),
      }),
    }),
  }),
};
```

The build fails if a page sets `status: "draft"` (not in the enum) or
`area: "unknown"` (not in the enum). Starlight's built-in fields (`title`,
`description`, `sidebar`, `template`, `hero`, `draft`) are inherited
automatically.

### Docusaurus (custom frontmatter + plugins)

Docusaurus does not enforce a frontmatter schema by default, but custom
plugins can validate frontmatter at build time. The `@docusaurus/plugin-
content-docs` plugin accepts a `frontMatter` hook for preprocessing. For
schema validation, use a custom plugin that runs Zod on each doc's
frontmatter during the build:

```javascript
// plugins/validate-frontmatter/index.js
module.exports = function validateFrontmatter() {
  return {
    name: 'validate-frontmatter',
    async loadContent() {
      // Validate audience, status, area in each doc's frontmatter
    },
  };
};
```

Docusaurus sidebar generation can filter by frontmatter fields, and
custom theme components can render banners based on `status` (e.g. a
"Deprecated" banner).

### MkDocs (meta plugin + custom templates)

MkDocs (with the Material theme) supports custom frontmatter via the
`meta` plugin. Frontmatter fields are available in templates as
`page.meta.<field>`. Custom templates can render banners based on
`status` and filter sidebar entries based on `audience`. MkDocs does not
enforce a schema — invalid values are silently ignored — so validation
must be done in a pre-build script (e.g. a Python script that parses
frontmatter and checks values against an enum).

## Related Concepts

- [Diagram Tool Selection](diagram-tool-selection.md) — diagrams embedded
  in docs should follow the same taxonomy; an architecture diagram in a
  `developer`-facing doc has different detail level than one in a
  `user`-facing doc.
- [Color Contrast Practices](color-contrast.md) — status banners
  (deprecated, research) must meet WCAG contrast ratios.
- [Rust Doc Comment Patterns](rust-doc-comment-patterns.md) — the
  `# Examples` / `# Errors` / `# Panics` sections are a micro-taxonomy
  within a single doc page; the site-level taxonomy (audience, status,
  area) is the macro-level equivalent.
