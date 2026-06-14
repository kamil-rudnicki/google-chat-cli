use crate::auth;
use crate::cli::{
    AuthCommand, ChatCommand, Cli, Command, DmCommand, ListSpacesArgs, MessagesArgs, SearchArgs,
    SpacesCommand,
};
use crate::config::{ConfigStore, normalize_email};
use crate::error::AppError;
use crate::google::{ChatClient, MessageFilters};
use crate::output::{SuccessEnvelope, success};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

pub async fn run(cli: Cli) -> Result<SuccessEnvelope, AppError> {
    let root = ConfigStore::resolve_root(cli.config_dir.clone())?;
    let store = ConfigStore::new(root)?;

    match &cli.command {
        Command::Auth { command } => run_auth(&store, command).await,
        Command::Chat { command } => run_chat(&store, &cli, command).await,
        Command::Search(args) => run_search(&store, &cli, args).await,
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
            Ok(success(
                "chat.send",
                Some(account),
                json!({ "message": message }),
                verbose_meta(cli, &client, json!({})),
            ))
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
        )
        .await?;
    let count = page.items.len();
    let next_page_token = page.next_page_token.clone();
    let truncated = page.truncated;
    Ok(success(
        command,
        Some(account),
        json!({ "spaces": page.items }),
        verbose_meta(
            cli,
            &client,
            json!({
                "count": count,
                "nextPageToken": next_page_token,
                "truncated": truncated
            }),
        ),
    ))
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
        )
        .await?;
    let count = page.items.len();
    let next_page_token = page.next_page_token.clone();
    let truncated = page.truncated;
    Ok(success(
        "chat.messages",
        Some(account),
        json!({ "messages": page.items }),
        verbose_meta(
            cli,
            &client,
            json!({
                "count": count,
                "nextPageToken": next_page_token,
                "truncated": truncated
            }),
        ),
    ))
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
            Ok(success(
                "chat.dm.space",
                Some(account),
                json!({ "space": space }),
                verbose_meta(cli, &client, json!({ "action": action })),
            ))
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
            Ok(success(
                "chat.dm.send",
                Some(account),
                json!({ "space": space, "message": message }),
                verbose_meta(cli, &client, json!({ "dmSpaceAction": action })),
            ))
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
        )
        .await?;
    let threads = summarize_threads(&page.items);
    let thread_count = threads.len();
    let message_count = page.items.len();
    let next_page_token = page.next_page_token.clone();
    let truncated = page.truncated;
    Ok(success(
        "chat.threads",
        Some(account),
        json!({ "threads": threads }),
        verbose_meta(
            cli,
            &client,
            json!({
                "count": thread_count,
                "messageCount": message_count,
                "nextPageToken": next_page_token,
                "truncated": truncated
            }),
        ),
    ))
}

async fn run_search(
    store: &ConfigStore,
    cli: &Cli,
    args: &SearchArgs,
) -> Result<SuccessEnvelope, AppError> {
    let unread = args.query.len() == 1 && args.query[0].eq_ignore_ascii_case("unread");
    let command = if unread { "search.unread" } else { "search" };
    let (account, client, scopes) = authenticated_client_with_scopes(store, cli, command).await?;
    if unread {
        auth::require_scope(&scopes, auth::CHAT_READSTATE_READONLY, command)?;
    }

    let (page, filter, view, order) = client
        .search_messages(
            command,
            args,
            cli.max,
            cli.page_token.as_deref(),
            cli.all,
            unread,
        )
        .await?;
    let count = page.items.len();
    let next_page_token = page.next_page_token.clone();
    let truncated = page.truncated;
    Ok(success(
        command,
        Some(account),
        json!({ "results": page.items }),
        verbose_meta(
            cli,
            &client,
            json!({
                "count": count,
                "nextPageToken": next_page_token,
                "truncated": truncated,
                "previewApi": true,
                "filter": filter,
                "view": view.api_value(),
                "orderBy": order.api_value(),
                "searchLimitations": [
                    "developer_preview_api",
                    "not_all_message_types_are_searchable"
                ]
            }),
        ),
    ))
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
}
