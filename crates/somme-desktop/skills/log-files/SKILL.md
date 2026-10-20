---
name: log-files
description: Log files. Keeping them small, rotating and compressing them, reading them for errors, and summing them up.
---
# Log files

## Find out first
- Which program writes them, where, and how big they get: `ls -la`, `du -sh`.
- Whether something already rotates them: logrotate (`/etc/logrotate.d`), newsyslog on macOS, journald, or the program's own setting.

## Keeping them small
- Prefer the system's way: a logrotate or newsyslog rule, or `journalctl --vacuum-size=` and `--vacuum-time=`. Then the program reopens its log the right way.
- Don't delete a log a program has open: the space isn't freed and the program keeps writing to the lost file. Empty it in place with `: > <file>` only if the user agrees, or rotate it.
- Compress old ones (`gzip`), and remove only rotated ones past an age (like `*.log.*.gz` older than 30 days), found each run.

## Reading them
- Read only the part since the last run: keep the last size or time in a small file, or use `journalctl --since`.
- Count errors by kind (`grep -c`, `sort | uniq -c | sort -rn | head`), and show a few lines of each.
- Logs can hold secrets and personal data. Send a summary to AI, never whole logs.
