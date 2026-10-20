---
name: freeing-disk-space
description: Freeing disk space on a computer or server. Finding what is big, what can safely go, and cleaning it by hand, on a schedule or when space runs low.
---
# Freeing disk space

## Find out first
- Which disk is short, and how full: `df -h`.
- Where the space went: `du -xhd1 <folder> 2>/dev/null | sort -h | tail`, one level at a time, starting from the folders the user named. Don't walk the whole disk in one look.
- What made each big thing: a build tool, a package manager, an app's cache, logs, downloads, containers, old backups.

## What can usually go
Things that can be made again and that nobody is using now:
- Build output and installed packages beside the project file that makes them again: node_modules beside package.json, target beside Cargo.toml, .venv beside pyproject.toml or requirements.txt, build or dist beside its build file. Check git ignores it: `git -C <project> check-ignore -q <folder>`.
- Caches that have a cleaning command of their own. Use it, since it knows what is safe: `npm cache clean --force`, `pip cache purge`, `brew cleanup`, `go clean -cache`, `apt-get clean`, `journalctl --vacuum-time=14d`, and for containers see the containers skill.
- Old logs and temp files that nothing has open, past an age.

## What must stay
Anything git tracks, documents, photos, mail, settings, keys, databases, virtual machine disks, and anything the user made by hand. When a folder mixes these, leave it.

## Make it decide each run
- Find targets by rule when the step runs, never from a list made today. For example, find folders by name, keep only those with the project file beside them, and only those not changed for N days: `find <root> -name node_modules -type d -prune -mtime +N`, then test for package.json in the parent.
- Check free space first and stop once there is enough. Don't clean more than needed.
- Skip projects being worked on now: changed in the last day or two, or a running process inside them.
- Say what was removed, each size, and the free space before and after.

## Preview and check
- Preview: the same find, listing paths and sizes (`du -sh`), changing nothing.
- Check: free space is above the goal, or each removed path is gone.

## Scope
The paths are the folders the rule removes, like `~/Projects/**/node_modules`, never their parents. Set rebuildable only for things that can be made again. Deletes of anything else go to the Somme bin.
