# insta

Local daemon that uses a personal Instagram web session (`sessionid`, `csrftoken`, `ds_user_id` cookies) and exposes an MCP server (Streamable HTTP on `127.0.0.1`, default port 7780) so Claude can read, triage and answer the user's Instagram one-on-one DMs as the user. Written in Rust. Runs under pm2 on macOS. Sibling of paperplane (Telegram), wasap (WhatsApp) and slackline (Slack).

## Hard rules

- **Public repo.** Never commit anything personal: no tokens, cookies, DB files, real Instagram ids (user/thread/item), usernames, names or message text (including in fixtures, tests and logs). Fixtures use short fake ids like thread `TH001`, user `U001`, item `I001`. All runtime state lives in `~/.insta/` (override with `INSTA_HOME`), never in the repo.
- Secrets live in the OS keyring (service `insta`): the macOS Keychain through `/usr/bin/security`, or the Linux Secret Service through `secret-tool`. Never in env files or on disk. That includes the OpenAI key (account `openai_key`).
- Never log message content, transcripts, audio, tokens, cookies or keys.
- No message text is stored on disk, not even the user's own (voice learning fetches it live). SQLite holds triage state, a metadata cache, voice profiles/notes (Claude-written style descriptions) and message *ids* only.
- Only `src/ig/` makes HTTP calls to Instagram (including its CDN). Everything else goes through the `InstagramApi` trait. Only `src/whisper/` calls OpenAI.
- Never guess Instagram endpoints, headers or JSON shapes: they must be verified against the live web client first.
- Reading never marks anything seen (Instagram shows "Seen" to the other person). Only `mark_read` and the send queue do.
- Sends always go through the send queue (20–60 s random gaps, 30/hour cap). Never add a path that sends directly or retries a send that may have gone out.
- A challenge / "please wait" response stops everything until `insta login`. Never auto-retry around it.

## Module Documentation Convention (MANDATORY)

Every module has a living doc at `docs/modules/<module>.md` (flat file, one per module). `docs/modules/README.md` is the index that routes a module's source path → its doc. These are the fast on-ramp for anyone — human or agent — touching a module.

**Progressive disclosure — do NOT load all docs at once.** When you're about to touch a module, open `docs/modules/README.md`, find the ONE doc matching the code you're changing, and read only that. Never pull the whole `docs/modules/` folder into context.

**The workflow rule:**
1. **Before modifying a module, read its `docs/modules/<module>.md` first.** It holds the file map, key methods with `file:line`, dependencies, and gotchas.
2. **After modifying a module, update its doc in the same change.** New/removed endpoints, changed behavior, new gotchas, dependency changes — all go into the doc before the work is done. Bump the "Last verified" date.
3. Doc claims must be verified against source and cite `file:line`. Never document something you haven't confirmed exists.
4. **Adding a new module?** Create its `docs/modules/<module>.md` and add a row to `docs/modules/README.md` in the same change.

Docs follow a shared template: Purpose, Structure, Endpoints/Public surface, Key methods (`file:line`), Dependencies, Gotchas & invariants, Testing.
