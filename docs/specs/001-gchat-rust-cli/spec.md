# Spec: gchat Rust CLI for Google Chat

## Goal

Build `gchat`, a Rust command-line client for Google Chat that is script-first, account-aware, and always returns pretty-printed JSON. The CLI should cover authentication, listing spaces, reading messages, sending messages, direct messages, thread views, and Google Chat message search including unread search.

This is not a wrapper around `gogcli`. Use the linked `gogcli` command docs as behavioral inspiration, but the product name, binary, config root, command grammar, and output contract belong to `gchat`.

## Hard Requirements

1. The binary name must be `gchat`.
2. The implementation language must be Rust.
3. The default config root must be `~/.config/gchat`.
4. `gchat auth credentials ~/Downloads/client_secret_....json` must import OAuth client credentials into the config root.
5. `gchat auth add you@gmail.com` must authorize and store an account token for that email.
6. Every successful command must print formatted JSON to stdout, never tables, colorized text, YAML, TSV, or one-line JSON.
7. Every failure must print formatted JSON to stderr and exit non-zero.
8. GitHub Actions must build automatically on push and pull requests.
9. GitHub Actions must create release artifacts when a version tag is pushed.
10. macOS installation must be supported through Homebrew.

## Blunt Constraint

Homebrew "cask" is the wrong default packaging vehicle for a bare CLI binary. A CLI should normally ship as a Homebrew formula. If the project later ships a signed `.pkg` or `.app`, add a cask then. For this spec, the required macOS deliverable is a Homebrew tap formula, because that is what users should actually install:

```bash
brew tap <owner>/gchat
brew install gchat
```

If a cask is still required for political or distribution reasons, it is an optional extra and must not replace the formula.

## References

- Google Chat user auth guide: https://developers.google.com/workspace/chat/authenticate-authorize-chat-user
- Google Chat spaces.list: https://developers.google.com/workspace/chat/api/reference/rest/v1/spaces/list
- Google Chat spaces.findDirectMessage: https://developers.google.com/workspace/chat/api/reference/rest/v1/spaces/findDirectMessage
- Google Chat spaces.setup: https://developers.google.com/workspace/chat/api/reference/rest/v1/spaces/setup
- Google Chat spaces.messages.list: https://developers.google.com/workspace/chat/api/reference/rest/v1/spaces.messages/list
- Google Chat spaces.messages.create: https://developers.google.com/workspace/chat/api/reference/rest/v1/spaces.messages/create
- Google Chat spaces.messages.search: https://developers.google.com/workspace/chat/api/reference/rest/v1/spaces.messages/search
- `gog chat dm`: https://github.com/openclaw/gogcli/blob/main/docs/commands/gog-chat-dm.md
- `gog chat messages`: https://github.com/openclaw/gogcli/blob/main/docs/commands/gog-chat-messages.md
- `gog chat spaces`: https://github.com/openclaw/gogcli/blob/main/docs/commands/gog-chat-spaces.md
- `gog chat threads`: https://github.com/openclaw/gogcli/blob/main/docs/commands/gog-chat-threads.md

## Command Contract

### Top-level

| Command | Purpose |
| --- | --- |
| `gchat auth credentials <client-secret-json>` | Import OAuth client credentials. |
| `gchat auth add <email>` | Add or refresh an authenticated Google account. |
| `gchat auth list` | List configured accounts without secrets. |
| `gchat auth remove <email>` | Remove an account token. |
| `gchat chat list` | List Google Chat spaces. |
| `gchat chat spaces list` | Alias for `gchat chat list`. |
| `gchat chat messages <space-id>` | List messages in a space. |
| `gchat chat messages <space-id> --max 10` | List recent messages with a result cap. |
| `gchat chat send --space <space-id> --text "Hello"` | Send a text message. |
| `gchat chat dm space <email>` | Find or create a direct-message space with a user. |
| `gchat chat dm send <email> --text "Hello"` | Send a direct message to a user. |
| `gchat chat threads <space-id>` | Derive thread summaries from messages in a space. |
| `gchat search <query>` | Search messages using Chat API search. |
| `gchat search unread` | Search unread messages using `is_unread()`. |

### Global Flags

| Flag | Applies To | Behavior |
| --- | --- | --- |
| `--account <email>` | authenticated commands | Select the stored account. |
| `--config-dir <path>` | all commands | Override `~/.config/gchat`; env alias: `GCHAT_CONFIG_DIR`. |
| `--max <n>` | list/search commands | Maximum returned items; default depends on command. |
| `--page-token <token>` | list/search commands | Request a specific page. |
| `--all` | list/search commands | Follow pagination until exhausted or `--max` reached. |
| `--verbose` | all commands | Add diagnostic fields in JSON; do not print free-form logs. |
| `--version` | all commands | Return JSON version metadata. |
| `--help` | all commands | Return JSON help metadata. |

Do not add `--json`. JSON is not a mode. JSON is the product contract.

## Output Contract

All output must use `serde_json::to_writer_pretty`.

Success envelope:

```json
{
  "ok": true,
  "command": "chat.messages",
  "account": "you@gmail.com",
  "data": {},
  "meta": {
    "nextPageToken": null,
    "count": 0,
    "truncated": false
  }
}
```

Error envelope:

```json
{
  "ok": false,
  "command": "chat.messages",
  "error": {
    "code": "google_api_error",
    "message": "Google Chat API returned PERMISSION_DENIED.",
    "details": {
      "status": 403,
      "googleStatus": "PERMISSION_DENIED"
    }
  }
}
```

Rules:

1. stdout is used only for successful JSON.
2. stderr is used only for failed JSON.
3. Do not print browser URLs, progress messages, warnings, or logs outside JSON.
4. Secrets, refresh tokens, access tokens, client secrets, and authorization codes must never be printed.
5. Message text from Google Chat is untrusted user content. Preserve it as JSON string data, not terminal markup.
6. `--help` and `--version` still emit JSON.

## Exit Codes

| Code | Meaning |
| --- | --- |
| `0` | Success. |
| `2` | Usage error, invalid argument, missing config, or missing auth. |
| `3` | No results when a future `--fail-empty` flag is used. |
| `4` | Google API returned an error. |
| `5` | Local IO, config, keychain, or token storage failure. |
| `6` | OAuth browser/callback flow failed. |

## Config and Storage

Default root:

```text
~/.config/gchat/
  config.json
  oauth-client.json
  accounts/
    you@gmail.com.json
  cache/
    spaces.json
```

Requirements:

1. Create `~/.config/gchat` with `0700` permissions on Unix-like systems.
2. Store secret-bearing files with `0600` permissions on Unix-like systems.
3. `oauth-client.json` stores the imported Google OAuth client metadata.
4. Account files store token metadata, expiry, requested scopes, and account email.
5. Prefer OS keychain storage for refresh tokens through a Rust keyring crate. If keychain storage is unavailable, fall back to encrypted or restricted-permission file storage and report that storage mode in JSON.
6. `config.json` tracks schema version, default account, and non-secret preferences.
7. Never mutate the original `~/Downloads/client_secret_....json` file.
8. Never write access tokens into the OAuth client credential file.

## Authentication

### `gchat auth credentials <path>`

Behavior:

1. Expand `~` in `<path>`.
2. Validate that the file is JSON and contains a Google OAuth desktop client shape, usually an `installed` object.
3. Copy normalized credentials into `~/.config/gchat/oauth-client.json`.
4. Return JSON showing imported client metadata, not secrets.

Success example:

```json
{
  "ok": true,
  "command": "auth.credentials",
  "data": {
    "configDir": "/Users/kamil/.config/gchat",
    "clientId": "redacted.apps.googleusercontent.com",
    "source": "/Users/kamil/Downloads/client_secret_123.json",
    "storedAt": "/Users/kamil/.config/gchat/oauth-client.json"
  },
  "meta": {
    "credentialSecretStored": true
  }
}
```

### `gchat auth add <email>`

Behavior:

1. Require imported credentials before starting OAuth.
2. Start an installed-app OAuth flow using a loopback redirect server.
3. Request offline access so the CLI can refresh access tokens.
4. Request only the scopes needed for the implemented feature set.
5. Verify that the authenticated account matches `<email>` when Google returns identity information. If identity cannot be verified from the granted scopes, store the email as user-supplied and report `verified: false`.
6. Store refresh token securely.
7. Store token expiry and granted scopes.

Minimum scopes:

```text
https://www.googleapis.com/auth/chat.spaces.readonly
https://www.googleapis.com/auth/chat.spaces.create
https://www.googleapis.com/auth/chat.messages.readonly
https://www.googleapis.com/auth/chat.messages.create
https://www.googleapis.com/auth/chat.users.readstate.readonly
```

Notes:

1. `chat.spaces.create` is needed only because `gchat chat dm space` may create a DM through `spaces.setup`.
2. `chat.users.readstate.readonly` is needed only for `gchat search unread`.
3. If Google refuses a granular scope combination for an account, fail with a JSON error that states the missing scope and the command that needs it.

## Chat API Mapping

| `gchat` command | Google API | Notes |
| --- | --- | --- |
| `gchat chat list` | `GET /v1/spaces` | Use user auth; expose `pageSize`, `pageToken`, optional `filter`. |
| `gchat chat messages <space-id>` | `GET /v1/{parent=spaces/*}/messages` | Default order should be recent-first for CLI ergonomics. |
| `gchat chat send --space <space-id> --text <text>` | `POST /v1/{parent=spaces/*}/messages` | User auth can send text. |
| `gchat chat dm space <email>` | `GET /v1/spaces:findDirectMessage`, then `POST /v1/spaces:setup` if missing | For setup, use `spaceType: DIRECT_MESSAGE` and one membership. |
| `gchat chat dm send <email> --text <text>` | DM space lookup/setup, then message create | Must return both the space and created message. |
| `gchat chat threads <space-id>` | `messages.list`, grouped by `thread.name` | Google Chat REST has thread read-state endpoints, not a general thread-list endpoint. |
| `gchat search <query>` | `POST /v1/spaces/-/messages:search` | Developer Preview; expose API limitations in `meta.previewApi`. |
| `gchat search unread` | `POST /v1/spaces/-/messages:search` with `filter: "is_unread()"` | Requires read-state scope. |

Use raw REST through `reqwest` unless a maintained Rust Google Chat client proves it covers all required endpoints cleanly. Do not hide API names in the code; keep endpoint mapping testable.

## Command Details

### `gchat chat list`

Default:

1. Fetch up to 100 spaces.
2. Return an array under `data.spaces`.
3. Preserve Google resource names like `spaces/AAAA...`.
4. Include `nextPageToken` in `meta` when returned.

Options:

| Option | Behavior |
| --- | --- |
| `--max <n>` | Clamp to API max of 1000 per request and stop after `n` total results. |
| `--page-token <token>` | Pass through to Google API. |
| `--all` | Fetch all pages until no token remains. |
| `--type SPACE|GROUP_CHAT|DIRECT_MESSAGE` | Translate to `spaces.list` filter. |

Acceptance:

```bash
gchat chat list
```

returns:

```json
{
  "ok": true,
  "command": "chat.list",
  "account": "you@gmail.com",
  "data": {
    "spaces": []
  },
  "meta": {
    "count": 0,
    "nextPageToken": null,
    "truncated": false
  }
}
```

### `gchat chat messages <space-id>`

Default:

1. Accept both raw IDs and resource names:
   - `AAAA...` becomes `spaces/AAAA...`.
   - `spaces/AAAA...` is used as-is.
2. Fetch recent messages first.
3. Default `--max` is `50`.
4. Return messages under `data.messages`.

Options:

| Option | Behavior |
| --- | --- |
| `--max <n>` | Maximum returned messages. |
| `--page-token <token>` | Pass through to Google API. |
| `--thread <thread-resource>` | Apply `thread.name = ...` filter. |
| `--before <rfc3339>` | Apply `createTime < ...` filter. |
| `--after <rfc3339>` | Apply `createTime > ...` filter. |
| `--include-deleted` | Set `showDeleted=true`. |

Acceptance:

```bash
gchat chat messages spaces/AAAA --max 10
```

must call `spaces.messages.list` and return no more than 10 messages.

### `gchat chat send --space <space-id> --text <text>`

Behavior:

1. Require non-empty `--text`.
2. Reject text whose UTF-8 size exceeds Google Chat message size limits before the API call when possible.
3. Send a text-only message as the authenticated user.
4. Support `--thread <thread-resource>` for replies.
5. Return the created Google Chat message.

Acceptance:

```bash
gchat chat send --space spaces/AAAA --text "Hello"
```

returns the created message under `data.message`.

### `gchat chat dm space <email>`

Behavior:

1. Normalize email to lowercase for config lookup and request construction.
2. First call `spaces.findDirectMessage` with `name=users/<email>`.
3. If Google returns 404, call `spaces.setup` with:

```json
{
  "space": {
    "spaceType": "DIRECT_MESSAGE",
    "singleUserBotDm": false
  },
  "memberships": [
    {
      "member": {
        "name": "users/person@example.com",
        "type": "HUMAN"
      }
    }
  ]
}
```

4. Return whether the space was found or created in `meta.action`.

### `gchat chat dm send <email> --text <text>`

Behavior:

1. Resolve or create the DM space using `gchat chat dm space` behavior.
2. Send the message through `spaces.messages.create`.
3. Return both the DM space and message:

```json
{
  "ok": true,
  "command": "chat.dm.send",
  "account": "you@gmail.com",
  "data": {
    "space": {},
    "message": {}
  },
  "meta": {
    "dmSpaceAction": "found"
  }
}
```

### `gchat chat threads <space-id>`

Behavior:

1. Use `messages.list` and group messages by `thread.name`.
2. Return thread summaries under `data.threads`.
3. Each thread summary includes:
   - `name`
   - `messageCount`
   - `firstMessage`
   - `lastMessage`
   - `lastCreateTime`
4. This is a derived view, not a separate Google API thread-list call.

### `gchat search <query>`

Behavior:

1. Call `POST https://chat.googleapis.com/v1/spaces/-/messages:search`.
2. Put the user query into the API `filter` field exactly, unless composing extra filters from CLI flags.
3. Default `pageSize` is 25.
4. Max page size is 100.
5. Default `orderBy` is `createTime desc`.
6. Return results under `data.results`.
7. Always include `meta.previewApi: true` because Google marks `spaces.messages.search` as Developer Preview.
8. Always include `meta.searchLimitations` with a concise machine-readable list because the API does not return every message type.

Options:

| Option | Behavior |
| --- | --- |
| `--space <space-id>` | Add `space.name = "spaces/..."` to the filter. |
| `--sender <email-or-user>` | Add `sender.name = "users/..."` to the filter. |
| `--after <rfc3339>` | Add `createTime >= ...`. |
| `--before <rfc3339>` | Add `createTime < ...`. |
| `--has-link` | Add `has_link()`. |
| `--attachments` | Add `attachment:*`. |
| `--view basic|full` | Map to `SEARCH_MESSAGES_VIEW_BASIC` or `SEARCH_MESSAGES_VIEW_FULL`. |
| `--order createTime|relevance` | Use descending order only; reject ascending order. |

### `gchat search unread`

Behavior:

1. Equivalent to `gchat search "is_unread()"`.
2. When additional query terms are provided in a future form, compose them with `AND`.
3. Require `chat.users.readstate.readonly` or fail before the API call if granted scopes are known and insufficient.
4. For full unread metadata, request `SEARCH_MESSAGES_VIEW_FULL` when possible.

Acceptance:

```bash
gchat search unread
```

must send:

```json
{
  "filter": "is_unread()",
  "pageSize": 25,
  "orderBy": "createTime desc",
  "view": "SEARCH_MESSAGES_VIEW_FULL"
}
```

## Rust Architecture

Suggested crate layout:

```text
src/
  main.rs
  cli.rs
  output.rs
  config.rs
  auth/
    mod.rs
    oauth.rs
    token_store.rs
  google/
    mod.rs
    chat.rs
    error.rs
  commands/
    mod.rs
    auth.rs
    chat.rs
    search.rs
```

Suggested dependencies:

| Crate | Purpose |
| --- | --- |
| `clap` | CLI parsing. |
| `serde`, `serde_json` | JSON config, API payloads, output. |
| `tokio` | Async runtime. |
| `reqwest` | Google REST calls. |
| `oauth2` | Installed-app OAuth flow. |
| `directories` | Config directory resolution. |
| `keyring` | OS secret storage. |
| `thiserror` | Error modeling. |
| `tracing` | Internal diagnostics, serialized into JSON only when needed. |
| `uuid` | OAuth state/request IDs. |
| `urlencoding` | Safe query construction where needed. |

Design rules:

1. Keep API request/response structs serializable and testable.
2. Centralize output rendering so no command can accidentally print plain text.
3. Centralize error conversion into the error envelope.
4. Use integration tests with mocked HTTP for Google API behavior.
5. Treat OAuth and token storage as separate modules so storage can move from file fallback to keychain without touching command logic.

## GitHub Actions

### CI workflow

File: `.github/workflows/ci.yml`

Triggers:

```yaml
on:
  pull_request:
  push:
    branches:
      - main
```

Required jobs:

1. `cargo fmt --check`
2. `cargo clippy --all-targets --all-features -- -D warnings`
3. `cargo test --all-features`
4. Build on:
   - `ubuntu-latest`
   - `macos-latest`
   - `windows-latest`

### Release workflow

File: `.github/workflows/release.yml`

Trigger:

```yaml
on:
  push:
    tags:
      - "v*.*.*"
```

Required behavior:

1. Validate the tag version matches `Cargo.toml` package version without the leading `v`.
2. Build release artifacts for:
   - `x86_64-unknown-linux-gnu`
   - `aarch64-unknown-linux-gnu`
   - `x86_64-apple-darwin`
   - `aarch64-apple-darwin`
   - `x86_64-pc-windows-msvc`
3. Package archives as `.tar.gz` for Unix and `.zip` for Windows.
4. Generate SHA-256 checksums.
5. Create a GitHub Release for the tag.
6. Upload binaries and checksum files.
7. Update the Homebrew formula in the configured tap after release artifacts exist.

`cargo-dist` is acceptable if it satisfies the above and keeps the release workflow smaller. If `cargo-dist` is used, check in its generated config and document the tag workflow in `README.md`.

## Homebrew Distribution

Required primary path: Homebrew formula in a tap repository.

Formula requirements:

1. Formula name: `gchat`.
2. Install the macOS archive matching the user's architecture.
3. Install shell completions if generated.
4. Run a smoke test:

```ruby
system "#{bin}/gchat", "--version"
```

5. The smoke test must receive JSON and assert it contains `"ok": true`.

Optional cask:

1. Only add a cask if shipping a signed `.pkg` or `.app`.
2. Cask name must not conflict with the formula.
3. The cask must not be the only documented macOS install path.

## Security and Privacy

1. Do not log tokens, authorization codes, client secrets, or raw OAuth callback URLs.
2. Do not print account token file paths unless `--verbose` is set.
3. Do not cache message bodies by default.
4. If any caching is added, cache only non-secret space metadata unless explicitly configured.
5. Redact emails only where needed for logs; command JSON can include the active account because the user explicitly selected it.
6. Use TLS through `reqwest` defaults; do not disable certificate validation.
7. Validate and normalize Google resource names before building URLs.

## Test Plan

### Unit tests

1. Config path resolution uses `~/.config/gchat` by default.
2. `GCHAT_CONFIG_DIR` and `--config-dir` override defaults predictably.
3. `gchat auth credentials` rejects non-JSON and malformed OAuth client files.
4. Output renderer always pretty-prints JSON.
5. Error renderer writes the documented envelope.
6. Space ID normalization accepts both `spaces/AAA` and `AAA`.
7. Search filter composition preserves quoted user queries.
8. Unread search composes `is_unread()` correctly.

### Integration tests with mocked Google API

1. `chat list` calls `GET /v1/spaces`.
2. `chat messages` calls `GET /v1/spaces/{id}/messages`.
3. `chat send` calls `POST /v1/spaces/{id}/messages` with `{ "text": "..." }`.
4. `dm space` calls `findDirectMessage` first.
5. `dm space` calls `spaces.setup` after mocked 404.
6. `dm send` sends into the resolved DM space.
7. `chat threads` groups mocked messages by `thread.name`.
8. `search` calls `POST /v1/spaces/-/messages:search`.
9. `search unread` sends `filter: "is_unread()"`.
10. Google API errors convert to structured JSON and correct exit codes.

### Release tests

1. CI passes on Linux, macOS, and Windows.
2. A dry-run release can produce all expected artifacts.
3. Version-tag release refuses to run if tag and `Cargo.toml` differ.
4. Homebrew formula smoke test validates JSON output from `gchat --version`.

## MVP Cut

The first useful release is complete when these commands work end to end:

1. `gchat auth credentials ~/Downloads/client_secret_....json`
2. `gchat auth add you@gmail.com`
3. `gchat chat list`
4. `gchat chat messages <space-id>`
5. `gchat chat messages <space-id> --max 10`
6. `gchat chat send --space <space-id> --text "Hello"`
7. `gchat chat dm space <email>`
8. `gchat chat dm send <email> --text "Hello"`
9. `gchat chat threads <space-id>`
10. `gchat search <query>`
11. `gchat search unread`
12. CI and tag-based releases work.
13. Homebrew formula installation works on macOS.

Anything beyond that is not MVP. Attachments, reactions, message deletion, admin search, slash commands, app-auth bot mode, and rich card messages are future work.
