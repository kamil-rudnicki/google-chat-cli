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
```

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
