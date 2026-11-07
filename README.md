# insta

A small local daemon that lets Claude (Claude Code, Claude Desktop) triage, read and answer your Instagram **one-on-one DMs** **as you**. It exposes an MCP server on `127.0.0.1`. Written in Rust: one binary.

Sibling of paperplane (Telegram), wasap (WhatsApp) and slackline (Slack).

> **Status:** the Instagram client (`src/ig/http.rs`) is a stub for now: every Instagram call returns "not implemented yet" until the web endpoints are verified. Everything around it (queue, triage, voice, MCP server, CLI) is built and tested against a fake.

> **Heads-up:** insta uses your Instagram **browser session** (the cookies the Instagram website uses). That's not an official API, it may break whenever Instagram changes its web client, and it may go against Instagram's terms. Instagram is quick to flag automated behavior, so insta paces every call and queues every send. Messages go out exactly as if you typed them. Use it on your own account only.

## What it does

- `get_pending`: one-on-one chats where the other person wrote last, newest first, minus the ones you muted or labeled.
- Read any chat by `@username` or part of a name. Reels, posts, stories, GIFs, images and voice notes show as short markers.
- Send replies as you, only after you approve the exact text. Sends are **queued**: the first goes at once, then one every 20–60 s (random), at most 30 per hour. `send_status` shows how each one went.
- Labels: `done`, `waiting` (both cleared by their next message) and `snoozed` (until a time). Watch levels: `normal` / `muted`.
- Voice: Claude learns how you write per category of chat (friends, work...) and per chat, from your own messages fetched live, and saves a short style description. It refreshes it when it goes stale.
- `transcribe_voice`: turns a voice note into text with OpenAI Whisper (your key, your cost). Only when you ask.

Reading through insta never marks anything seen. Instagram shows "Seen" to the other person, so only `mark_read` and sending do that.

## What's stored

- `~/.insta/` (override with `INSTA_HOME`), mode 0700:
  - `insta.db` (mode 0600): watch levels, categories, labels, a cache of who each chat is with, voice profiles and notes (style descriptions Claude wrote), and the ids and times of your own messages. **No message text.**
  - `config.json` (optional): `{"port": 7780}`.
  - `style.md` (optional): your writing style, appended to the instructions Claude gets. Re-read on every new session, no restart needed.
- The OS keyring, service `insta`: the three Instagram cookies, the MCP bearer token and (optionally) your OpenAI API key.
  - macOS: the Keychain, through `/usr/bin/security`, so rebuilding never brings back the access prompt.
  - Linux: the Secret Service (GNOME Keyring or KWallet), through `secret-tool`.
- Queued messages live only in memory. Restarting insta drops unsent ones.

Nothing is logged except counts, job ids, status codes and error kinds.

## Setup

Requires Rust (1.94+) and pm2, on macOS or Linux.

On Linux (Ubuntu/Debian) also install `secret-tool` and use a desktop session with an unlocked keyring:

```sh
sudo apt install build-essential libsecret-tools
```

```sh
cargo build --release
./target/release/insta login            # paste the three cookies (hidden input)
./target/release/insta set-openai-key   # optional, for transcribe_voice
pm2 start ecosystem.config.cjs
claude mcp add --scope user --transport http insta http://127.0.0.1:7780/mcp \
  --header "Authorization: Bearer $(./target/release/insta token)"
```

Where `login` gets its values from: open https://www.instagram.com in your browser, logged in, and open the dev tools (Cmd+Option+I on macOS, Ctrl+Shift+I on Linux). In Application > Cookies > `https://www.instagram.com`, copy the Value of `sessionid`, `csrftoken` and `ds_user_id`.

If Instagram logs you out, or asks for a security check ("challenge", "please wait a few minutes"), insta stops calling Instagram entirely. Open Instagram in the browser, make sure it works there, then run `insta login` again. No restart is needed.

## Commands

| Command | What it does |
|---|---|
| `insta start` | Runs the MCP server (pm2 runs this) |
| `insta login` / `logout` | Saves or removes the Instagram session in the OS keyring |
| `insta status` | Shows whether the session works, and as whom |
| `insta token` | Prints the MCP bearer token |
| `insta set-openai-key` | Saves an OpenAI API key for `transcribe_voice` |
| `insta probe` | Prints the structure (never the values) of Instagram's responses, for debugging |

## Development

`cargo test`. The tests use wiremock, a virtual clock and an in-memory fake Instagram with fake ids. Module docs live in `docs/modules/`, starting at the index (`README.md`).

## License

MIT
