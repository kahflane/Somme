---
name: containers
description: Docker or Podman containers. Cleaning images and build cache safely, updating compose apps, restarting unhealthy containers, and checking them.
---
# Containers

## Find out first
- Docker or Podman, compose or not: `docker version`, `docker compose ls`, `docker ps`.
- How much space they use: `docker system df`.

## Somme's values
Somme fills in `{{...}}` itself, so a Go template like `--format '{{.Names}}'` comes out empty. Use `--format json` or `-q` and read that instead.

## Cleaning
- Use prune with an age filter: `docker image prune -a --filter until=168h`, `docker container prune --filter until=24h`, `docker builder prune --filter until=168h`.
- Never remove volumes unless the user asked: volumes hold data. No `docker volume prune` and no `docker system prune --volumes` on your own say.
- Preview with `docker system df` and `docker image ls --filter dangling=true`.

## Updating compose apps
- In the app's folder: `docker compose pull`, then `docker compose up -d`, which only recreates what changed.
- Check with `docker compose ps` and the app's health. Keep the old image tag so you can go back.

## Restarting
- Find unhealthy ones each run: `docker ps --filter health=unhealthy -q`. Restart only those, and say which.
