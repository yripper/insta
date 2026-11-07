# config

Last verified: 2026-11-07

## Purpose
Runtime paths and the optional user config. All state lives in `~/.insta` (or `INSTA_HOME`), never in the repo.

## Structure
- `src/config.rs`: `Paths`, `Config`, `DEFAULT_PORT`.

## Public surface
- `DEFAULT_PORT = 7780` (`src/config.rs:12`). 7777 (paperplane), 7778 (wasap) and 7779 (slackline) are taken by the siblings, so all four run side by side.
- `Paths::resolve` (`src/config.rs:23`): `INSTA_HOME` or `$HOME/.insta`, created and chmod 0700. Gives `insta.db`, `config.json`, `style.md`.
- `Config::load` (`src/config.rs:51`): `config.json` `{"port": N}`; missing file means defaults, bad JSON is an error.

## Dependencies
serde, serde_json, anyhow.

## Gotchas & invariants
- The home directory is re-chmodded to 0700 on every resolve: it holds names, triage state and voice profiles.

## Testing
`cargo test config`: one test (env vars are process-global) covering `INSTA_HOME`, the 0700 mode, the default port, an override and bad JSON.
