---
name: deploys
description: Deploying or updating an app or service on servers. Getting the new version, restarting safely, checking health, and going back when it fails.
---
# Deploys

## Find out first
- How the app runs now: a systemd service, docker compose, a process manager (pm2, supervisor), a web server in front.
- Where its code or image comes from, and how it is built.
- How to tell it is healthy: a health URL, a port, a status command, a log line.
- Which servers, and whether they can go one at a time.

## Order
1. Check the server first: free disk, and the service is up now.
2. Record what runs now so you can go back: the git commit (`git rev-parse HEAD`), the image tag, or the release folder.
3. Get the new version without stopping anything: `git fetch` then checkout, `docker compose pull`, or a new release folder.
4. Restart or reload the least you can. Prefer reload where the service has one.
5. Check health a few times, with a wait between.
6. If it is not healthy, go back to what was recorded, restart again, and fail the run so the user hears.

## Many servers
- One at a time ("one_by_one": true), stopping at the first that fails, so they are never all down.
- An ask step before the first restart, unless the user said not to.

## Scope and report
- The services it restarts, by name, the app's folder, and sudo only if the service needs it.
- Say the old version, the new version, and the health result on each server.
