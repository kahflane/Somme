# Somme

An SSH client like Termius. Rust + Tauri v2 + Next.js. Data in Turso, synced through Somme Cloud.

## Run it

1. `cp env.example .env` and fill `TURSO_API` (Turso platform token) and `TURSO_ORG`.
2. `cargo run -p somme-cloud` starts the sync server on `127.0.0.1:8787`.
3. `pnpm install && pnpm tauri dev` starts the app.

Set `SOMME_CLOUD_URL` when building the app to point it at a deployed Somme Cloud.

## Layout

| Path | Layer |
|---|---|
| `crates/somme-domain` | entities, rules, repository traits |
| `crates/somme-application` | use cases, ports, DTOs |
| `crates/somme-infrastructure` | Turso, russh (SSH, SFTP, tunnels), PTY, crypto, keychain, cloud client |
| `crates/somme-cloud` | Somme Cloud server: accounts, one Turso database per user |
| `src-tauri` | Tauri commands and wiring |
| `src` | Next.js UI |

## Tests

- `cargo test --workspace` runs the unit tests.
- Live tests are marked `#[ignore]`:
  - `SOMME_TEST_SSHD_DIR=<dir> cargo test -p somme-infrastructure --test ssh_e2e -- --ignored` needs an sshd on `127.0.0.1:2222`.
  - `SOMME_CLOUD_URL=http://127.0.0.1:8787 cargo test -p somme-infrastructure --test managed_sync -- --ignored` needs Somme Cloud running.
