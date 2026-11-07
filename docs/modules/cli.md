# cli

Last verified: 2026-11-07

## Purpose
The `insta` binary: the daemon (`start`) and one-off commands.

## Structure
- `src/main.rs`: tracing (`insta=info` default, no ANSI when stdout isn't a TTY) on a `current_thread` tokio runtime.
- `src/cli/mod.rs`: clap `Cli`/`Command`, `check_credentials`, `save_login`, `login`, `set_openai_key`, `status`, `logout`, `run`.
- `src/cli/daemon.rs`: `start`, `ensure_bearer_token`.
- `src/cli/probe.rs`: `probe`, `shape`.

## Public surface
`start`, `login`, `logout`, `status`, `token`, `set-openai-key`, `probe`.

## Key methods
- `daemon::start` (`src/cli/daemon.rs:32`): paths, config, store, bearer token; binds `127.0.0.1:<port>` (a busy port means it's already running: binding is the single-instance guard); builds the send queue (`SystemTimer`, `ThreadRng`), Whisper and `AppState`; spawns the queue worker; tries the session once (log only); serves. On shutdown logs how many unsent jobs were dropped (count only).
- `ensure_bearer_token` (`src/cli/daemon.rs:21`): 32 random bytes as hex in the keyring.
- `save_login` (`src/cli/mod.rs:69`): `check_credentials` (`:58`, token characters only; the error names the cookie, never its value), then `connector.connect`. `Validated(me)` or, while `HttpInstagram` is a stub, `ValidationPending` (detected with `IgError::is_not_implemented`); any other error aborts without saving. Saves the three cookies and clears the challenge flag. Once the client is implemented, validation starts working with no change here.
- `login` (`src/cli/mod.rs:92`): devtools instructions (Application > Cookies > https://www.instagram.com), hidden `rpassword` prompts.
- `set_openai_key` (`src/cli/mod.rs:114`): hidden prompt, saved as `openai_key`.
- `probe::shape` (`src/cli/probe.rs:30`): keys and types only. All-digit keys (Instagram ids) print as `<id>` and objects keyed by them as `map[n] of T`, so no id or value reaches the output. `probe` asks `HttpInstagram::raw` for `PROBE_TARGETS` (`:11`); stubbed for now.

## Dependencies
clap, rpassword, rand, tracing-subscriber, plus config, secrets, store, ig, whisper, domain and mcp.

## Gotchas & invariants
- Measured on 2026-11-07: release binary 5.1 MB (macOS arm64).
- `status` checks the challenge flag first and then doesn't call Instagram.

## Testing
`cargo test cli`: bearer token idempotence, login validated / pending / rejected (nothing saved, value never echoed), `shape` never leaking values or id keys.
