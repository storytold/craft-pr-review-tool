Craft PR Review
===============

A tray app for working through the open PRs on the craft apps. It polls GitHub,
grades how close each PR is to landing, reads CodeRabbit's findings, and has
Claude summarize PRs and group the ones possibly related. It runs locally, next to
[Craft Status](https://github.com/storytold/craft-repo-status-app), and shares its account, config and cache.

## Views

**Apps** (`⌘1`) has one row per app, and every column sorts: open PRs, how many
have been inspected, landable, promising, reviewable, CI not run, approved, high
quality, easy/medium/hard, needs work,
conflicts, failing CI, CodeRabbit flags and nits, security flags and nits, PRs by
core, frequent and new authors, interesting PRs, possibly related groups and ages.

**An app** lists its PRs, best first, with sortable columns (including who approved
each one and what it's possibly related to), filters, search and **Group possibly
related**. Drafts are hidden unless asked for.

**A PR** has the verdict and the reasons behind it, Claude's assessment, CodeRabbit
findings worst first, security flags, the author's standing, the PRs it's possibly related to,
checks, reviews, files and the description. **Read the diff** asks Claude for a
closer look.

**Review queue** (`⌘2`) is what someone can act on across every app: Landable,
Promising, Reviewable and CI not run.

`Esc` goes back a level and hides the window at the root. `/` searches.

## How PRs are graded

All of it is deterministic, and the PR view shows the working
(`crates/pr-core/src/assess.rs`).

A PR has independent **flags** (Approved, Conflicted, CI failed…) and one **status**
worked out from them, from this truth table. The first matching row wins, and `–`
means that column doesn't matter. The `?` beside a PR table explains all of this in
the app. A *problem* is a failed check, requested changes, an unresolved critical or
major CodeRabbit finding, a failed security check, or an open conversation started by
a person. *Running* means CI or CodeRabbit is going, or GitHub hasn't worked out
mergeability.

| Draft | Problem | Conflicted | Inspected | Running | CI | Approved | Quality ≥ 75 | Status |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Y | – | – | – | – | – | – | – | **Draft** |
| N | Y | – | – | – | – | – | – | **Needs work** |
| N | N | Y | – | – | – | – | – | **Conflicted** |
| N | N | N | N | – | – | – | – | **Not inspected** |
| N | N | N | Y | Y | – | – | – | **Checking** |
| N | N | N | Y | N | not run | – | – | **CI not run** |
| N | N | N | Y | N | pass | Y | – | **Landable** |
| N | N | N | Y | N | pass | N | Y | **Promising** |
| N | N | N | Y | N | pass | N | N | **Reviewable** |

- **Approved** means GitHub's review decision is approved, on the latest commit. An
  approval of an older commit shows as a *Stale approval* flag and doesn't make a PR
  Landable. The Approved by column also lists approvals GitHub doesn't count, usually
  from people without write access, in grey.
- **Cancelled and skipped checks** don't count as failures or as CI having run. Until a
  PR is inspected, only GitHub's overall result is known, which counts a cancelled
  run as a failure.
- **The Status column** sorts best first, in the order Landable, Promising, Reviewable,
  CI not run, Checking, Not inspected, Conflicted, Needs work, Draft. Quality, ease and
  the author only break ties within a status.
- **Possibly related** never changes a status. Each PR lists the others it may relate to,
  and links go both ways: click a number to jump to its row, or ⌘-click to open it.
- **Ease** adds up size (not counting lockfiles), files, spread, conflicts, failing
  CI, flags and sensitive paths, less a point for authors who know the codebase.
- **Quality** starts at 60. It moves with CI, conflicts, reviews, description, a
  linked issue, tests, CodeRabbit's findings and the author's record. Until a PR is
  inspected it shows as `~` and can't count as high quality.
- **CodeRabbit**: critical and major findings are *flags*; minor and trivial are
  *nits*. Resolved and outdated threads don't count.
- **Security** means CodeRabbit findings in its `Security & Privacy` category (or
  titled like one), failed checks named like security checks, and `sensitive_paths`
  changed by someone without write access. The last doesn't count against a PR, since the
  author can't do anything about it: it asks the reviewer to look closer.
- **Authors**: *core* has write access or better, or is in the repo's
  `contributors/people.toml`. Contributors may add their own entry there, so set
  `people_toml_is_core = false` for a stricter list. *Frequent* has more than 30 PRs
  merged across the craft apps, counted from merge history. *New* is GitHub's
  first-time contributor.

## Claude

Claude runs through the Claude Code CLI (`claude -p`), using its existing login.
It works through inspected PRs best first, and regroups each repo daily.

- **Spending**: at most `max_runs_per_hour` background calls (12) and
  `max_usd_per_day` ($5, as reported by the CLI). The footer shows the day's total.
  At these limits only part of the backlog gets summarized each day, so change them
  in the config, along with `model` and `enabled`.
- **Safety**: PR text is written by strangers, so Claude gets no tools, MCP servers,
  settings or project files, and its answers are only shown as text. It can mark a PR
  harder or interesting, but can't change its status or raise its quality or ease.

## Working with Craft Status

- **Config**: the repo list and GitHub token come from `~/.craft_status_config.toml`,
  read and never written. Repos owned by anyone outside `first_party_owners`, such
  as solidcraft, are left out. The token is looked up the same way as Craft Status's.
- **Shared cache**, `~/Library/Caches/craft-github-cache/`, which either app can
  rebuild:
  - `prs/<owner>@<repo>.json`: a ledger of each repo's PRs, kept up to date by one
    walk that serves both apps. If either walked it in the last 5 minutes, it isn't
    walked again. This app only lists PRs when the ledger shows an open one changed.
    A repo's first ledger is seeded from what Craft Status's `raw/` cache held about
    the last week's PRs. That cache has no titles or open lists, so PR rows still
    come from GitHub: they appear page by page, smallest repo first.
  - `lease.lock`: a file lock held while talking to GitHub, so the apps take turns
    and can't overwrite each other's updates. It releases if an app dies. Craft Status
    waits for it; this app gives way.
  - `budget.json`: the latest rate-limit reading and any backoff. When GitHub limits
    either app, both pause.
- **Craft Status's snapshot** supplies app icons and open counts before the first
  fetch here, and an early relist when it spots a new PR.
- **Budget**: the account has 5000 GraphQL points an hour. Inspecting PRs and walking
  merge history stop at `max_points_per_hour` (2500), and everything pauses below
  `budget_floor` (1000), well above the 150 where Craft Status pauses itself.

Only a Craft Status built with the shared cache takes part. An older build keeps
working, but it won't take turns or save this app any quota.

## Polling

There are too many open PRs to inspect in one go (about 7,900, at 16 points per 25),
so work is split into tiers:

1. **Lists** of every open PR, every 30 min if anything changed, and a full rewalk
   every 4 h, which is the only way to see CI finishing on a PR nobody touched.
2. **Who can write**, daily.
3. **Merge history**, once, for the landed counts (about 160 pages).
4. **Inspection**, best candidates first: findings, files, checks, reviews. A first
   pass over everything takes a couple of hours, and after that only changed PRs
   are refetched. The footer counts what's left.

This app's own cache is `~/Library/Caches/craft-pr-review/`.

## Tray and window

It lives in the menu bar with no Dock icon. Left-click the tray icon to show or hide
the window; right-click for Refresh Now, Always on Top, Edit Config… and Quit. `⌘⇧Y`
summons it from anywhere. `⌘R` refreshes what's on screen.

## Configuration: `~/.craft_pr_review_config.toml`

Created from [the template](craft_pr_review_config.template.toml) on first launch and
re-read while running.

## Build, install, test

Needs Rust stable, Node.js 24+, `cargo install tauri-cli --version "^2"`, Xcode CLT,
`gh auth login` or a token, and Claude Code for the Claude features. Craft Status's
repo has to be checked out next to this one.

```sh
npm ci
make run        # debug app
make install    # release .app → /Applications, start at login, launch
make uninstall  # quit, remove the app and login item (keeps config + cache)
make test       # Rust + UI tests
make check      # clippy + rustfmt + tsc type-check
make preview    # the UI in a browser against what's cached
```
