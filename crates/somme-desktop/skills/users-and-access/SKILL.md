---
name: users-and-access
description: User accounts, SSH keys, sudo and file permissions on servers. Checking who has access, and adding or removing access carefully.
---
# Users and access

## Looking
- Accounts that can log in: `getent passwd` with a login shell; last logins with `lastlog` or `last`.
- SSH keys: `~/.ssh/authorized_keys` for each user; show key types and comments, not whole keys.
- Sudo: `getent group sudo wheel`, and `/etc/sudoers.d`.
- Files open to everyone: `find <dir> -perm -o+w -type f`.

## Changing
- Only change the accounts, keys or files the user named. Add an ask step before each change.
- Never remove the account or key Somme connects with, and never lock out the last way in. Check another way in works first.
- Edit sudo rules only through `visudo -c -f <file>` to check them before they take effect.
- Make a copy of `authorized_keys` before changing it, and change it with a tool that adds or removes one line, not by writing the file again.

## Report
- Who has access, what changed, and anything that looks wrong, like old accounts with sudo or keys with no comment.
