---
name: watching-and-alerts
description: Watching something on a schedule, such as disk, memory, a site, a service, a job or a log, and telling the user when it needs them without flooding them.
---
# Watching and alerts

## Find out first
- What to watch and where, and the number or sign that means trouble. Ask for the limit if the user didn't give one.
- How the user wants to hear: a notification, or an http step to a chat or webhook whose address is a vault value.

## Measure, then decide
- Get the number in a form steps can test: one value per line, or JSON. For example `df -P`, `free -m`, `curl -s -o /dev/null -w '%{http_code}' <url>`, `systemctl is-active <service>`.
- Use an if step (above, below, contains) to decide. Use a decide step only when the sign can't be measured.
- Watch everything the rule covers, found each run: every disk, every server with a tag, every service that matches. Not a list made today.

## Don't flood the user
- Tell once when it goes bad and once when it is fine again. Keep the last state in a small file (put its path in the scope) and compare.
- Say what is wrong, where, the number and the limit, in one line.
- Don't fix things on its own unless the user asked for that too.

## Every run
- If the check can't run (no network, server down), say so plainly. Never treat it as fine.
