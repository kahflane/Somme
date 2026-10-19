# Somme

An SSH client like Termius. Native Rust app built with GPUI. Data in Turso, synced through Somme Cloud.

## Run it

- `crates/somme-desktop/macos-app.sh` builds `target/debug/Somme.app` with the name and icon, then prints its path. Add `--release` for a release build.
- `cargo run -p somme-desktop` runs it without the app bundle.

The app uses the hosted Somme Cloud at `https://somme-cloud.fly.dev`. Set `SOMME_CLOUD_URL=http://127.0.0.1:8787` to use a local one instead.

## Release

Somme looks for a newer GitHub release every few hours and shows a button on the rail. On macOS it can update itself, but only with releases signed by your update key.

1. Once: `cargo run -p somme-desktop -- --update-keygen ~/somme-update.key`. It saves the private key there (keep it safe, never in the repo) and prints the public key. Put that line in `crates/somme-desktop/assets/update-key.pub` and commit it. Then `gh secret set SOMME_UPDATE_SIGNING_KEY < ~/somme-update.key`.
2. Raise `version` in the root `Cargo.toml` and commit it.
3. Push a tag `v<version>`. The Release workflow builds every system and publishes the release: one Mac app for Apple chip and Intel (`Somme-<version>-macos-universal.zip`), Windows, Linux, `SHA256SUMS` and `SHA256SUMS.sig`.

Copies built before step 1 still see the update, but only offer its release page.

## Somme Cloud

To run it locally: `cp env.example .env`, fill `TURSO_API` (Turso platform token) and `TURSO_ORG`, then `cargo run -p somme-cloud`. It listens on `127.0.0.1:8787`.

### Deploy

It runs on Fly.io (`fly.toml`, `crates/somme-cloud/Dockerfile`). From the repo root:

1. Once: `fly secrets set TURSO_API=... TURSO_ORG=... -a somme-cloud`
2. `fly deploy --remote-only --ha=false`

## Layout

| Path | Layer |
|---|---|
| `crates/somme-domain` | entities, rules, repository traits |
| `crates/somme-application` | use cases, ports, DTOs |
| `crates/somme-infrastructure` | Turso, russh (SSH, SFTP, tunnels), PTY, crypto, keychain, cloud client |
| `crates/somme-cloud` | Somme Cloud server: accounts, one Turso database per user |
| `crates/somme-desktop` | the GPUI app |

## Tests

- `cargo test --workspace` runs the unit tests.
- Live tests are marked `#[ignore]`:
  - `SOMME_TEST_SSHD_DIR=<dir> cargo test -p somme-infrastructure --test ssh_e2e -- --ignored` needs an sshd on `127.0.0.1:2222`.
  - `SOMME_CLOUD_URL=http://127.0.0.1:8787 cargo test -p somme-infrastructure --test managed_sync -- --ignored` needs Somme Cloud running.
