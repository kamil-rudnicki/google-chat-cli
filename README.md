# Google Chat CLI

`gchat` is a Rust CLI for Google Chat. It is script-first: successful commands print pretty JSON to stdout, failures print pretty JSON to stderr, and nothing prints ad-hoc terminal text.

##  Install

### Homebrew

```bash
brew tap kamil-rudnicki/gchat
brew install gchat
gchat --version
gchat auth credentials ~/Downloads/client_secret_....json
gchat auth add you@example.com
gchat chat list
```

## Use

```bash
gchat auth list
gchat chat list
gchat chat messages spaces/AAAA --max 10
gchat chat send --space spaces/AAAA --text "Hello"
gchat chat dm space person@example.com
gchat chat dm send person@example.com --text "Hello"
gchat chat threads spaces/AAAA
gchat search "invoice"
gchat search unread
gchat mark read

# Show entire thread
gchat chat messages spaces/AAAAJrp7YDg \
  --thread spaces/AAAAJrp7YDg/threads/3TAd8wa_By4 \
  --all

# Reply in a thread (full thread name or bare thread id; fails if the thread does not exist)
gchat chat send --space spaces/AAAAJrp7YDg \
  --thread spaces/AAAAJrp7YDg/threads/3TAd8wa_By4 \
  --text "Thanks, looking into it"

# Find all threads that contain specific text
gchat search "what is a status of project PROJ-328?" --all --max 5000 \
  | jq -r '.data.results[] | (.thread.name // .message.thread.name // empty)' \
  | sort -u

# Find matching messages and include the full threads they belong to
gchat search "what is a status of project PROJ-328?" --expand-threads --all --max 5000

# Get all unread messages
gchat search unread --all --max 5000
gchat search unread --include-marked --all --max 5000

# Faster unread export: BASIC view omits read/mute metadata; no display-name
# enrichment skips best-effort follow-up lookups.
gchat --no-display-names search unread --view basic --include-marked --all

# Display-name lookups are cached for 24h in:
# ~/.config/gchat/cache/display-names.json

# Filter truly unread messages as Google API is being moody
gchat search unread --include-marked --all --max 5000 \
  | jq '.data.results |= map(select(.read == false)) | .meta.count = (.data.results | length)'

# Stop showing the current unread backlog in future unread searches.
# This stores a local cutoff; it doesn't change Google Chat thread read state.
gchat mark read

# Try Google's remote space read-state update as well.
# This doesn't clear unread replies inside threads.
gchat mark read --remote --all --max 5000

# Mark spaces and threads returned by Google Chat's unread search as read.
# Add --dry-run to preview the targets.
gchat mark read --remote --include-threads
```

## Save unread messages for later

Build the local CLI with `cargo build --release`. The script uses your default `gchat` account and its read-state write permission. It saves message JSON in `~/.config/gchat/read-later.sqlite3`. To use a different account, pass `--account you@example.com` before the subcommand.

```sh
python3 scripts/chat_read_later.py stash --dry-run
python3 scripts/chat_read_later.py watch
python3 scripts/chat_read_later.py status
python3 scripts/chat_read_later.py restore
```

`watch` scans once a minute while running. For an existing scheduler, run `stash` once per minute instead. The script commits a copy of each message and the original space or thread read timestamp before changing Google Chat. It retains the copy after restoration.

`restore` keeps restored messages unread until you read them in Chat. The minute loop continues to collect new messages in other conversations and threads. Google Chat uses one read timestamp for a conversation and a separate one for each thread. A new message in the same conversation or thread is saved locally and stays unread while earlier restored messages are unread. The script covers the unread messages returned by the search API, which excludes some items including muted spaces and bot DMs.

## Google Chat Home unread

`gchat home unread` reads the same rows as **Google Chat Home → Unread**, including followed threads. It uses your signed-in Google Chrome profile on macOS or Linux. The command returns conversation and thread previews in `.data.items`, in Home order.

Set up the companion once in the Chrome profile you use for Chat.

1. Build from this checkout with `cargo build --release`, then run `./target/release/gchat home setup`. This registers the current binary as the extension’s local Chrome messaging host.
2. Open `chrome://extensions` in that profile, turn on Developer mode, and load the `browser-extension` directory with **Load unpacked**.
3. Run `./target/release/gchat home unread`. Use `--account you@example.com` to check the signed-in account against a specific address. If Chrome disconnects after a binary rebuild or browser restart, reload the extension or click **Reconnect gchat Home** in Chrome’s extensions menu.

Keep Chrome running. The companion can reuse an open Home tab with the right filters. When it needs to load Home or switch filters, it uses a temporary background tab. It reads previews without opening or marking any conversations. Google can change the Home layout, so the command reports an error if it cannot identify the list.

## Configure Google OAuth Client

Use a Google OAuth 2.0 client that is meant for this CLI.

Best option: create an OAuth client with application type `Desktop app`, download its client-secret JSON, then import that file with `gchat auth credentials`.
If you use a `Web application` OAuth client instead, add this exact value under **Authorized redirect URIs** in Google Cloud Console:

```text
http://127.0.0.1:53682/callback
```

Import a Google OAuth desktop-client JSON file:

```bash
gchat auth credentials ~/Downloads/client_secret_....json
```

Then authorize an account:

```bash
gchat auth add you@example.com
```

The default config root is `~/.config/gchat`.

Every command supports `--account <email>` when more than one account is configured.

## Docs

- [Developer notes](docs/DEVELOPER.md)
