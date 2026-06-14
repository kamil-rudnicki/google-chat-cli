# gchat

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

Authorize an account:

```bash
gchat auth add you@example.com
```

The default config root is `~/.config/gchat`.

## Use

```bash
gchat auth list
gchat chat list
gchat chat messages spaces/AAAA --max 10
gchat chat send --space spaces/AAAA --text "Hello"
gchat chat dm space person@example.com
gchat chat dm send person@example.com --text "Hello"
gchat chat threads spaces/AAAA
gchat search 'text:"invoice"'
gchat search unread
```

Every command supports `--account <email>` when more than one account is configured.
