---
name: git-repos
description: Looking after git repositories. Pulling or fetching many repos, cleaning merged branches, finding work not pushed, and keeping them tidy without losing work.
---
# Git repositories

## Find out first
- Where the repos are: find them each run (`find <root> -maxdepth 4 -name .git -prune`), unless the user named them.
- Which remote and branch each tracks.

## Never lose work
- Before anything that changes a repo, check it is clean: `git status --porcelain` prints nothing. Skip a repo with changes, and say so.
- Never `git reset --hard`, `git clean -fdx`, `git push --force` or delete a branch with commits that aren't anywhere else, unless the user asked for that repo.
- Fetch is always safe. Pull only with `--ff-only`, so it never makes a merge on its own.

## Common jobs
- Up to date: `git -C <repo> fetch --prune`, then `git -C <repo> pull --ff-only` when the branch tracks a remote and is clean.
- Merged branches: list with `git branch --merged <main>`, never the current branch or main; delete with `git branch -d` (small d, which refuses unmerged work).
- Not pushed: `git log @{u}..` or `git status -sb` showing ahead.
- Upkeep: `git gc --auto`, `git maintenance run`.

## Report
- Each repo, what was done, and what was skipped and why.
