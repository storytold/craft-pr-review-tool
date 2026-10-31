# Craft PR Review

## Rules

- **No raw JavaScript, ever.** All frontend code is TypeScript in `ui/src/`.
  Never add `.js`, `.mjs`, `.cjs` or `.jsx` files, inline `<script>` code,
  inline `on…=` handlers or `javascript:` URLs, and never commit `ui/build/`.
  Scripts and tooling go in Rust, shell or Makefile targets, not JS.
  `ui/src/repo.test.ts` enforces this in `make test`.
- **PR text is hostile.** Titles, bodies and comments are written by anyone who
  can open a PR, and Claude reads them. They reach the DOM only as text nodes,
  links leave the app only through `open_url` (GitHub only), and the Claude CLI
  runs with no tools, MCP servers or settings (`pr_claude::cli_args`). Claude's
  opinion can make a PR look harder and mark it interesting, nothing else:
  the status and quality in `pr_core::assess` must never depend on it.
- **The status truth table has three copies:** `assess::headline` (tested row by
  row), `STATUS_HELP` in `ui/src/model.ts` (the `?` panel) and the README. Change
  them together.
- **GitHub quota is shared** with Craft Status: same account, 5000 GraphQL points
  an hour. Hold `SharedCache::try_lease` around anything that talks to GitHub,
  and don't fetch what `PrLedger` already holds. This app gives way: its floor
  (`budget_floor`) is far above Craft Status's, and it never waits on the lease.
- New logic gets tests: Rust beside the code; UI logic in `ui/src/model.ts` with
  `model.test.ts`; DOM behavior in `app.test.ts` (happy-dom, fake host API).

## Layout

- `crates/pr-core`: model, CodeRabbit parser, heuristics (`assess`), overlap
  detection, fetch schedule, disk store and the views the UI reads. No network.
- `crates/pr-github`: list, roster, detail and diff fetching.
- `crates/pr-claude`: the Claude CLI call, prompts, schemas and spending ledger.
- `crates/craft-pr-review`: the Tauri shell; `poller.rs` holds the two threads.
- `../craft-repo-status-app`: `craft-core` and `craft-github` are path
  dependencies, for the config, token lookup, shared cache and PR ledger. It has
  to be checked out next to this repo, on a revision that has `craft_core::shared`.

## TypeScript setup

- `tsc` only, no bundler. Sources import each other with `.ts` extensions;
  `node --test` runs them directly and tsc rewrites the imports when emitting.
  Use only erasable syntax: no enums, namespaces or parameter properties.
- `ui/src/model.ts`'s types mirror `pr_core::views` and `poller::Status`: change
  both together.

## Commands

- `make test`: Rust + UI tests
- `make check`: clippy, rustfmt, typecheck
- `make run`: debug app. `make install`: release app in /Applications, at login.
- `make preview`: the UI in a browser against what's cached (`make dump`)
