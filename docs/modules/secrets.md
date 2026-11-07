# secrets

Last verified: 2026-11-07

## Purpose
OS keyring storage for the Instagram cookies, the MCP bearer token and the OpenAI key. Service `insta`. macOS uses the Keychain, Linux the Secret Service through `secret-tool`.

## Structure
- `src/secrets.rs`: `CommandRunner` + `Cli` (spawns the keyring CLI, turns "not installed" into an install hint), the `SecretStore` trait, `Keychain<R>` (macOS), `SecretTool<R>` (Linux), `default_store()`, `assert_safe`, and `tests_support::MemSecrets` (test-only in-memory store shared by other modules' tests, `src/secrets.rs:303`).

## Public surface
- Accounts (`src/secrets.rs:11`-`15`): `sessionid`, `csrftoken`, `ds_user_id`, `bearer_token`, `openai_key`.
- `Keychain`: get (`src/secrets.rs:90`, exit 44 `ITEM_NOT_FOUND` → `None`, `src/secrets.rs:180`), set (`src/secrets.rs:99`, `security -i` with the command on **stdin**), delete (`src/secrets.rs:110`, not-found is success).
- `SecretTool`: get (`src/secrets.rs:133`), set (`src/secrets.rs:146`, value on stdin), delete (`src/secrets.rs:159`).
- `default_store()` (`src/secrets.rs:169`) picks the backend by `target_os`.
- `assert_safe` (`src/secrets.rs:185`, public so `insta login` can pre-check cookie values).

## Dependencies
tokio::process, async-trait, anyhow. Same design as paperplane's `src/secrets/keychain.ts`.

## Gotchas & invariants
- `assert_safe` allows only `[A-Za-z0-9+/=_.:%-]`, because Keychain values are interpolated into a `security -i` line. `%` and `:` are there for the URL-encoded `sessionid` cookie. `SecretTool` passes values on stdin and needs no restriction.
- macOS uses `/usr/bin/security`, not a native crate: the Keychain ACL belongs to `security`, so rebuilding never re-prompts.
- Never falls back to plaintext files.

## Testing
`cargo test secrets`: fake `CommandRunner`, no real keyring touched.
