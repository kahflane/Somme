---
name: backups
description: Backing up files, folders, servers or databases. Making copies that can be restored, keeping a few, and proving they work.
---
# Backups

## Find out first
- What to keep safe, how big it is, and how fast it changes.
- Where copies go: another disk, another server, or cloud storage. A copy on the same disk is not a backup.
- Which tools are there: rsync, tar, zip, restic, borg, rclone, the database's dump tool.
- How many copies to keep, or for how long. Ask if the user didn't say.

## Make a good copy
- Write to a temporary name, then rename it once it is whole, so a half copy never looks finished.
- Name each copy by date and time (`date +%Y-%m-%d_%H%M`), so copies sort and never overwrite each other.
- For a database, use its dump tool, never the files of a running database. See the databases skill.
- Compress with gzip or zstd where it helps. Passwords come from vault values or the tool's own config, never the command.

## Prove it
- Check the copy is there, not empty and readable: `tar -tzf`, `gzip -t`, the tool's own check (`restic check`), or compare file counts and sizes with the source.
- Now and then, restore into a temp folder and compare.

## Keep a few
- Remove copies past the count or age, oldest first, and only ones this automation made: its own name pattern, in its own folder. Never remove the newest good copy.
- Say what was copied, its size, how long it took, and how many copies are kept.

## Every run
- If the source is missing or empty, fail loudly. Never replace good copies with an empty one.
- If there isn't room where copies go, stop and say so before starting.
- Tell the user when a backup fails: a notify step on the failed way out.
