- Read ~/p/gh/levonk/dnshub/AGENTS.md
- Run execute-upsert skill, if it's missing do a `devbox run -- pnpm skills add levonk/skills-releases --agents devin,universal`
- on the tasks index file under ~/p/gh/levonk/dnshub/internal-docs/feature/ for the dnshub project
- do not use `npx` or `npm` we use `devbox run -- pnpm dlx` or `devbox run -- pnpm`
- execute-upsert says to not stop processing, via subagents, until all the stories and tasks are addressed, this means marking things blocked until there is no more work you can do

