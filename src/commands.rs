use crate::auth;
use crate::cli::{
    AuthCommand, ChatCommand, Cli, Command, DmCommand, ListSpacesArgs, MarkCommand, MarkReadArgs,
    MessagesArgs, SearchArgs, SearchOrder, SearchView, SpacesCommand,
};
use crate::config::{ConfigStore, normalize_email};
use crate::error::AppError;
use crate::google::{ChatClient, MessageFilters};
use crate::output::{SuccessEnvelope, success, write_progress};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

pub async fn run(cli: Cli) -> Result<SuccessEnvelope, AppError> {
    let root = ConfigStore::resolve_root(cli.config_dir.clone())?;
    let store = ConfigStore::new(root)?;

    match &cli.command {
        Command::Auth { command } => run_auth(&store, command).await,
        Command::Chat { command } => run_chat(&store, &cli, command).await,
        Command::Search(args) => run_search(&store, &cli, args).await,
        Command::Mark { command } => run_mark(&store, &cli, command).await,
    }
}

async fn run_auth(store: &ConfigStore, command: &AuthCommand) -> Result<SuccessEnvelope, AppError> {
    match command {
        AuthCommand::Credentials(args) => {
            let imported = store.import_credentials(&args.client_secret_json)?;
            Ok(success(
                "auth.credentials",
                None,
                serde_json::to_value(imported)?,
                json!({ "credentialSecretStored": true }),
            ))
        }
        AuthCommand::Add(args) => {
            let (data, meta) = auth::add_account(store, &args.email).await?;
            Ok(success(
                "auth.add",
                Some(normalize_email(&args.email)),
                data,
                meta,
            ))
        }
        AuthCommand::List => {
            let accounts = store.list_accounts()?;
            let config = store.load_config()?;
            let count = accounts.len();
            Ok(success(
                "auth.list",
                None,
                json!({ "accounts": accounts }),
                json!({
                    "count": count,
                    "defaultAccount": config.default_account
                }),
            ))
        }
        AuthCommand::Remove(args) => {
            let removed = store.remove_account(&args.email)?;
            Ok(success(
                "auth.remove",
                None,
                json!({ "email": normalize_email(&args.email), "removed": removed }),
                json!({}),
            ))
        }
    }
}

async fn run_chat(
    store: &ConfigStore,
    cli: &Cli,
    command: &ChatCommand,
) -> Result<SuccessEnvelope, AppError> {
    match command {
        ChatCommand::List(args) => list_spaces(store, cli, args, "chat.list").await,
        ChatCommand::Spaces {
            command: SpacesCommand::List(args),
        } => list_spaces(store, cli, args, "chat.list").await,
        ChatCommand::Messages(args) => list_messages(store, cli, args).await,
        ChatCommand::Send(args) => {
            let (account, client) = authenticated_client(store, cli, "chat.send").await?;
            let message = client
                .send_message("chat.send", &args.space, &args.text, args.thread.as_deref())
                .await?;
            Ok(chat_success(
                "chat.send",
                account,
                cli,
                &client,
                json!({ "message": message }),
                json!({}),
            )
            .await)
        }
        ChatCommand::Dm { command } => run_dm(store, cli, command).await,
        ChatCommand::Threads(args) => list_threads(store, cli, args).await,
    }
}

async fn list_spaces(
    store: &ConfigStore,
    cli: &Cli,
    args: &ListSpacesArgs,
    command: &str,
) -> Result<SuccessEnvelope, AppError> {
    let (account, client) = authenticated_client(store, cli, command).await?;
    let page = client
        .list_spaces(
            command,
            cli.max,
            cli.page_token.as_deref(),
            cli.all,
            args.space_type.as_ref(),
            cli.progress,
        )
        .await?;
    let count = page.items.len();
    let next_page_token = page.next_page_token.clone();
    let truncated = page.truncated;
    Ok(chat_success(
        command,
        account,
        cli,
        &client,
        json!({ "spaces": page.items }),
        json!({
            "count": count,
            "nextPageToken": next_page_token,
            "truncated": truncated
        }),
    )
    .await)
}

async fn list_messages(
    store: &ConfigStore,
    cli: &Cli,
    args: &MessagesArgs,
) -> Result<SuccessEnvelope, AppError> {
    let (account, client) = authenticated_client(store, cli, "chat.messages").await?;
    let page = client
        .list_messages(
            "chat.messages",
            &args.space_id,
            cli.max,
            cli.page_token.as_deref(),
            cli.all,
            MessageFilters {
                thread: args.thread.clone(),
                before: args.before.clone(),
                after: args.after.clone(),
                include_deleted: args.include_deleted,
            },
            cli.progress,
        )
        .await?;
    let count = page.items.len();
    let next_page_token = page.next_page_token.clone();
    let truncated = page.truncated;
    Ok(chat_success(
        "chat.messages",
        account,
        cli,
        &client,
        json!({ "messages": page.items }),
        json!({
            "count": count,
            "nextPageToken": next_page_token,
            "truncated": truncated
        }),
    )
    .await)
}

async fn run_dm(
    store: &ConfigStore,
    cli: &Cli,
    command: &DmCommand,
) -> Result<SuccessEnvelope, AppError> {
    match command {
        DmCommand::Space(args) => {
            let (account, client) = authenticated_client(store, cli, "chat.dm.space").await?;
            let (space, action) = client
                .find_or_create_dm("chat.dm.space", &args.email)
                .await?;
            Ok(chat_success(
                "chat.dm.space",
                account,
                cli,
                &client,
                json!({ "space": space }),
                json!({ "action": action }),
            )
            .await)
        }
        DmCommand::Send(args) => {
            let (account, client) = authenticated_client(store, cli, "chat.dm.send").await?;
            let (space, action) = client
                .find_or_create_dm("chat.dm.send", &args.email)
                .await?;
            let space_name = space.get("name").and_then(Value::as_str).ok_or_else(|| {
                AppError::google_api(
                    "chat.dm.send",
                    200,
                    json!({
                        "error": {
                            "message": "spaces.findDirectMessage/setup response did not include a space name",
                            "status": "MALFORMED_RESPONSE"
                        },
                        "body": space.clone()
                    }),
                )
            })?;
            let message = client
                .send_message("chat.dm.send", space_name, &args.text, None)
                .await?;
            Ok(chat_success(
                "chat.dm.send",
                account,
                cli,
                &client,
                json!({ "space": space, "message": message }),
                json!({ "dmSpaceAction": action }),
            )
            .await)
        }
    }
}

async fn list_threads(
    store: &ConfigStore,
    cli: &Cli,
    args: &crate::cli::ThreadsArgs,
) -> Result<SuccessEnvelope, AppError> {
    let (account, client) = authenticated_client(store, cli, "chat.threads").await?;
    let page = client
        .list_messages(
            "chat.threads",
            &args.space_id,
            cli.max.or(Some(200)),
            cli.page_token.as_deref(),
            cli.all,
            MessageFilters {
                thread: None,
                before: None,
                after: None,
                include_deleted: false,
            },
            cli.progress,
        )
        .await?;
    let threads = summarize_threads(&page.items);
    let thread_count = threads.len();
    let message_count = page.items.len();
    let next_page_token = page.next_page_token.clone();
    let truncated = page.truncated;
    Ok(chat_success(
        "chat.threads",
        account,
        cli,
        &client,
        json!({ "threads": threads }),
        json!({
            "count": thread_count,
            "messageCount": message_count,
            "nextPageToken": next_page_token,
            "truncated": truncated
        }),
    )
    .await)
}

async fn run_search(
    store: &ConfigStore,
    cli: &Cli,
    args: &SearchArgs,
) -> Result<SuccessEnvelope, AppError> {
    let unread = args.query.len() == 1 && args.query[0].eq_ignore_ascii_case("unread");
    let command = if unread { "search.unread" } else { "search" };
    let (account, client, scopes) = authenticated_client_with_scopes(store, cli, command).await?;
    let mut local_unread_cutoff = None;
    if unread {
        auth::require_any_scope(
            &scopes,
            &[auth::CHAT_READSTATE_READONLY, auth::CHAT_READSTATE],
            command,
        )?;
        if !args.include_marked {
            local_unread_cutoff = store.load_unread_search_after(&account, command)?;
        }
    }

    let (mut page, filter, view, order) = client
        .search_messages(
            command,
            args,
            cli.max,
            cli.page_token.as_deref(),
            cli.all,
            unread,
            cli.progress,
        )
        .await?;
    let local_unread_cutoff_applied =
        unread && !args.include_marked && local_unread_cutoff.is_some() && args.after.is_none();
    let mut local_unread_hidden_count = 0;
    if local_unread_cutoff_applied {
        let cutoff = local_unread_cutoff.as_deref().and_then(parse_rfc3339_utc);
        if let Some(cutoff) = cutoff {
            let mut saw_older_result = false;
            page.items.retain(|result| {
                let Some(create_time) =
                    search_result_create_time(result).and_then(|value| parse_rfc3339_utc(&value))
                else {
                    return true;
                };
                if create_time < cutoff {
                    local_unread_hidden_count += 1;
                    saw_older_result = true;
                    false
                } else {
                    true
                }
            });
            if saw_older_result && matches!(order, SearchOrder::CreateTime) {
                page.next_page_token = None;
                page.truncated = false;
            }
        }
    }
    let count = page.items.len();
    let next_page_token = page.next_page_token.clone();
    let truncated = page.truncated;
    Ok(chat_success(
        command,
        account,
        cli,
        &client,
        json!({ "results": page.items }),
        json!({
            "count": count,
            "nextPageToken": next_page_token,
            "truncated": truncated,
            "previewApi": true,
            "filter": filter,
            "localUnreadCutoff": local_unread_cutoff,
            "localUnreadCutoffApplied": local_unread_cutoff_applied,
            "localUnreadHiddenCount": local_unread_hidden_count,
            "view": view.api_value(),
            "orderBy": order.api_value(),
            "searchLimitations": [
                "developer_preview_api",
                "not_all_message_types_are_searchable"
            ]
        }),
    )
    .await)
}

async fn run_mark(
    store: &ConfigStore,
    cli: &Cli,
    command: &MarkCommand,
) -> Result<SuccessEnvelope, AppError> {
    match command {
        MarkCommand::Read(args) => mark_read(store, cli, args).await,
    }
}

async fn mark_read(
    store: &ConfigStore,
    cli: &Cli,
    args: &MarkReadArgs,
) -> Result<SuccessEnvelope, AppError> {
    let command = "mark.read";
    let read_at = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    if args.space.is_some() && !args.remote {
        return Err(AppError::usage(
            command,
            "`--space` only applies to `gchat mark read --remote`",
            json!({ "fix": "drop `--space` for a local cutoff, or add `--remote`" }),
        ));
    }

    let account = store.select_account(cli.account.as_deref(), command)?;
    let remote_client = if args.remote {
        let (_, client, scopes) = authenticated_client_with_scopes(store, cli, command).await?;
        if args.dry_run {
            auth::require_any_scope(
                &scopes,
                &[auth::CHAT_READSTATE_READONLY, auth::CHAT_READSTATE],
                command,
            )?;
        } else {
            auth::require_scope(&scopes, auth::CHAT_READSTATE, command)?;
        }
        Some(client)
    } else {
        None
    };

    let local_baseline = if args.dry_run {
        None
    } else {
        Some(store.save_unread_search_after(&account, &read_at, command)?)
    };

    let mut remote_marked_spaces = Vec::new();
    let mut remote_unread_result_count = 0;
    let mut remote_top_level_result_count = 0;
    let mut remote_thread_reply_result_count = 0;
    let mut remote_already_read_result_count = 0;

    if let Some(client) = remote_client {
        let mut targets = if let Some(space) = args.space.as_deref() {
            let space = crate::google::normalize_space_name(space)?;
            let target = SpaceReadTarget {
                space: space.clone(),
                ..SpaceReadTarget::default()
            };
            BTreeMap::from([(space, target)])
        } else {
            unread_space_targets(&client, cli, command).await?
        };

        remote_unread_result_count = targets
            .values()
            .map(|target| target.unread_result_count)
            .sum();
        remote_top_level_result_count = targets
            .values()
            .map(|target| target.top_level_result_count)
            .sum();
        remote_thread_reply_result_count = targets
            .values()
            .map(|target| target.thread_reply_result_count)
            .sum();
        remote_already_read_result_count = targets
            .values()
            .map(|target| target.already_read_result_count)
            .sum();

        let total = targets.len();
        for (index, target) in targets.values_mut().enumerate() {
            let response = if args.dry_run {
                None
            } else {
                Some(
                    client
                        .update_space_read_state(command, &target.space, &read_at)
                        .await?,
                )
            };
            remote_marked_spaces.push(target.to_json(&read_at, response));
            if cli.progress {
                write_progress(
                    command,
                    "mark.remote_space",
                    index + 1,
                    Some(total),
                    json!({
                        "dryRun": args.dry_run
                    }),
                );
            }
        }
    } else {
        if cli.progress {
            write_progress(
                command,
                "mark.local_cutoff",
                1,
                Some(1),
                json!({
                    "dryRun": args.dry_run
                }),
            );
        }
    }

    Ok(success(
        command,
        Some(account),
        json!({
            "localBaseline": local_baseline,
            "remoteMarkedSpaces": remote_marked_spaces
        }),
        json!({
            "count": remote_marked_spaces.len(),
            "dryRun": args.dry_run,
            "remote": args.remote,
            "readAt": read_at,
            "localUnreadCutoffStored": !args.dry_run,
            "remoteUnreadResultCount": remote_unread_result_count,
            "remoteTopLevelResultCount": remote_top_level_result_count,
            "remoteThreadReplyResultCount": remote_thread_reply_result_count,
            "remoteAlreadyReadResultCount": remote_already_read_result_count,
            "updateMask": "lastReadTime",
            "readStateLimitations": [
                "local_cutoff_affects_gchat_search_unread_only",
                "space_read_state_only",
                "thread_replies_unaffected_by_space_read_state",
                "search_api_does_not_return_all_message_types"
            ]
        }),
    ))
}

async fn unread_space_targets(
    client: &ChatClient,
    cli: &Cli,
    command: &str,
) -> Result<BTreeMap<String, SpaceReadTarget>, AppError> {
    let search_args = SearchArgs {
        query: vec!["unread".to_string()],
        space: None,
        sender: None,
        after: None,
        before: None,
        has_link: false,
        attachments: false,
        include_marked: true,
        view: Some(SearchView::Full),
        order: SearchOrder::CreateTime,
    };
    let (page, _, _, _) = client
        .search_messages(
            command,
            &search_args,
            cli.max.or(Some(5000)),
            cli.page_token.as_deref(),
            true,
            true,
            cli.progress,
        )
        .await?;

    let mut targets = BTreeMap::new();
    for result in page.items {
        let Some(space) = search_result_space_name(&result) else {
            continue;
        };
        let target = targets
            .entry(space.clone())
            .or_insert_with(|| SpaceReadTarget {
                space,
                ..SpaceReadTarget::default()
            });
        target.unread_result_count += 1;
        if search_result_read(&result) {
            target.already_read_result_count += 1;
        }
        if search_result_is_thread_root(&result) {
            target.top_level_result_count += 1;
        } else {
            target.thread_reply_result_count += 1;
        }
        target.latest_unread_create_time = max_string_option(
            target.latest_unread_create_time.take(),
            search_result_create_time(&result),
        );
    }
    Ok(targets)
}

async fn authenticated_client(
    store: &ConfigStore,
    cli: &Cli,
    command: &str,
) -> Result<(String, ChatClient), AppError> {
    let (account, client, _) = authenticated_client_with_scopes(store, cli, command).await?;
    Ok((account, client))
}

async fn authenticated_client_with_scopes(
    store: &ConfigStore,
    cli: &Cli,
    command: &str,
) -> Result<(String, ChatClient, Vec<String>), AppError> {
    let account = store.select_account(cli.account.as_deref(), command)?;
    let (_, token) = auth::access_token(store, &account, command).await?;
    let client = ChatClient::new(token.token);
    Ok((account, client, token.scopes))
}

fn verbose_meta(cli: &Cli, client: &ChatClient, mut meta: Value) -> Value {
    if cli.verbose
        && let Value::Object(ref mut object) = meta
    {
        object.insert("apiBase".to_string(), json!(client.base_url()));
    }
    meta
}

async fn chat_success(
    command: &str,
    account: String,
    cli: &Cli,
    client: &ChatClient,
    mut data: Value,
    meta: Value,
) -> SuccessEnvelope {
    if !cli.no_display_names {
        client.enrich_display_names(&mut data).await;
    }
    success(
        command,
        Some(account),
        data,
        verbose_meta(cli, client, meta),
    )
}

fn summarize_threads(messages: &[Value]) -> Vec<Value> {
    #[derive(Default)]
    struct ThreadAccumulator {
        name: String,
        message_count: usize,
        first_message: Option<Value>,
        last_message: Option<Value>,
        last_create_time: Option<String>,
    }

    let mut threads: BTreeMap<String, ThreadAccumulator> = BTreeMap::new();
    for message in messages {
        let thread_name = message
            .get("thread")
            .and_then(|thread| thread.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("threads/unknown")
            .to_string();
        let create_time = message
            .get("createTime")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let entry = threads
            .entry(thread_name.clone())
            .or_insert_with(|| ThreadAccumulator {
                name: thread_name,
                ..ThreadAccumulator::default()
            });
        entry.message_count += 1;
        if entry.first_message.is_none() {
            entry.first_message = Some(message.clone());
        }
        if entry.last_create_time.as_deref().unwrap_or_default() <= create_time.as_str() {
            entry.last_create_time = Some(create_time);
            entry.last_message = Some(message.clone());
        }
    }

    threads
        .into_values()
        .map(|thread| {
            let mut object = Map::new();
            object.insert("name".to_string(), json!(thread.name));
            object.insert("messageCount".to_string(), json!(thread.message_count));
            object.insert(
                "firstMessage".to_string(),
                thread.first_message.unwrap_or(Value::Null),
            );
            object.insert(
                "lastMessage".to_string(),
                thread.last_message.unwrap_or(Value::Null),
            );
            object.insert("lastCreateTime".to_string(), json!(thread.last_create_time));
            Value::Object(object)
        })
        .collect()
}

#[derive(Default)]
struct SpaceReadTarget {
    space: String,
    unread_result_count: usize,
    top_level_result_count: usize,
    thread_reply_result_count: usize,
    already_read_result_count: usize,
    latest_unread_create_time: Option<String>,
}

impl SpaceReadTarget {
    fn to_json(&self, read_at: &str, response: Option<Value>) -> Value {
        json!({
            "space": {
                "name": self.space
            },
            "readAt": read_at,
            "unreadResultCount": self.unread_result_count,
            "topLevelResultCount": self.top_level_result_count,
            "threadReplyResultCount": self.thread_reply_result_count,
            "alreadyReadResultCount": self.already_read_result_count,
            "latestUnreadCreateTime": self.latest_unread_create_time,
            "response": response,
        })
    }
}

fn search_result_message(result: &Value) -> &Value {
    result.get("message").unwrap_or(result)
}

fn search_result_space_name(result: &Value) -> Option<String> {
    let message = search_result_message(result);
    if let Some(space) = message
        .get("space")
        .and_then(|space| space.get("name"))
        .and_then(Value::as_str)
    {
        return Some(space.to_string());
    }

    message
        .get("name")
        .and_then(Value::as_str)
        .and_then(|name| {
            name.split_once("/messages/")
                .map(|(space, _)| space.to_string())
        })
}

fn search_result_create_time(result: &Value) -> Option<String> {
    search_result_message(result)
        .get("createTime")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn search_result_read(result: &Value) -> bool {
    result.get("read").and_then(Value::as_bool).unwrap_or(false)
}

fn search_result_is_thread_root(result: &Value) -> bool {
    let message = search_result_message(result);
    let Some(message_name) = message.get("name").and_then(Value::as_str) else {
        return false;
    };
    let Some(thread_name) = message
        .get("thread")
        .and_then(|thread| thread.get("name"))
        .and_then(Value::as_str)
    else {
        return false;
    };
    let Some(message_id) = message_name.split_once("/messages/").map(|(_, id)| id) else {
        return false;
    };
    let Some(thread_id) = thread_name.split_once("/threads/").map(|(_, id)| id) else {
        return false;
    };
    let mut parts = message_id.split('.');
    matches!(
        (parts.next(), parts.next(), parts.next()),
        (Some(left), Some(right), None) if left == thread_id && right == thread_id
    )
}

fn max_string_option(left: Option<String>, right: Option<String>) -> Option<String> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (Some(left), None) => Some(left),
        (None, Some(right)) => Some(right),
        (None, None) => None,
    }
}

fn parse_rfc3339_utc(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thread_summary_groups_messages() {
        let messages = vec![
            json!({
                "name": "spaces/A/messages/1",
                "createTime": "2026-01-01T00:00:00Z",
                "thread": { "name": "spaces/A/threads/T" }
            }),
            json!({
                "name": "spaces/A/messages/2",
                "createTime": "2026-01-02T00:00:00Z",
                "thread": { "name": "spaces/A/threads/T" }
            }),
        ];
        let threads = summarize_threads(&messages);
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0]["messageCount"], 2);
        assert_eq!(threads[0]["lastCreateTime"], "2026-01-02T00:00:00Z");
    }

    #[test]
    fn detects_thread_root_search_results() {
        let root = json!({
            "message": {
                "name": "spaces/A/messages/T.T",
                "thread": { "name": "spaces/A/threads/T" }
            }
        });
        let reply = json!({
            "message": {
                "name": "spaces/A/messages/T.R",
                "thread": { "name": "spaces/A/threads/T" }
            }
        });

        assert!(search_result_is_thread_root(&root));
        assert!(!search_result_is_thread_root(&reply));
    }

    #[test]
    fn extracts_space_from_search_result_message_name_fallback() {
        let result = json!({
            "message": {
                "name": "spaces/A/messages/T.R"
            }
        });

        assert_eq!(
            search_result_space_name(&result),
            Some("spaces/A".to_string())
        );
    }
}
