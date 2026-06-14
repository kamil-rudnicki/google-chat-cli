use crate::cli::{SearchArgs, SearchOrder, SearchView, SpaceType};
use crate::error::AppError;
use reqwest::{Client, Method};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

const DEFAULT_API_BASE: &str = "https://chat.googleapis.com/v1";
const DEFAULT_PEOPLE_API_BASE: &str = "https://people.googleapis.com/v1";

#[derive(Debug, Clone)]
pub struct ChatClient {
    http: Client,
    base_url: String,
    access_token: String,
}

#[derive(Debug, Clone)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_page_token: Option<String>,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub struct MessageFilters {
    pub thread: Option<String>,
    pub before: Option<String>,
    pub after: Option<String>,
    pub include_deleted: bool,
}

impl ChatClient {
    pub fn new(access_token: String) -> Self {
        let base_url =
            std::env::var("GCHAT_API_BASE").unwrap_or_else(|_| DEFAULT_API_BASE.to_string());
        Self {
            http: Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
            access_token,
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub async fn enrich_display_names(&self, value: &mut Value) {
        let targets = collect_display_name_targets(value);
        if targets.is_empty() {
            return;
        }

        let mut display_names = DisplayNames::default();
        for space_name in &targets.spaces {
            let mut display_name = self.fetch_space_display_name(space_name).await;
            if display_name.is_none() && targets.space_member_fallbacks.contains(space_name) {
                display_name = self.fetch_space_member_display_name(space_name).await;
            }
            if let Some(display_name) = display_name {
                display_names
                    .spaces
                    .insert(space_name.clone(), display_name);
            }
        }
        for chunk in targets.users.chunks(200) {
            display_names
                .users
                .append(&mut self.fetch_user_display_names(chunk).await);
        }

        apply_display_names(value, &display_names);
    }

    pub async fn list_spaces(
        &self,
        command: &str,
        max: Option<usize>,
        page_token: Option<&str>,
        all: bool,
        space_type: Option<&SpaceType>,
    ) -> Result<Page<Value>, AppError> {
        let limit = max.unwrap_or(100);
        let mut collected = Vec::new();
        let mut token = page_token.map(ToOwned::to_owned);
        let mut next_page_token = None;

        loop {
            let remaining = limit.saturating_sub(collected.len());
            if remaining == 0 {
                break;
            }
            let page_size = remaining.clamp(1, 1000);
            let mut query = vec![("pageSize".to_string(), page_size.to_string())];
            if let Some(token) = token.as_deref() {
                query.push(("pageToken".to_string(), token.to_string()));
            }
            if let Some(space_type) = space_type {
                query.push((
                    "filter".to_string(),
                    format!("spaceType = \"{}\"", space_type.api_value()),
                ));
            }

            let body = self
                .request_json(command, Method::GET, "spaces", query, None)
                .await?;
            let mut spaces = take_array(&body, "spaces");
            collected.append(&mut spaces);
            next_page_token = body
                .get("nextPageToken")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);

            if !all || next_page_token.is_none() {
                break;
            }
            token.clone_from(&next_page_token);
        }

        let truncated = next_page_token.is_some() && (!all || collected.len() >= limit);
        Ok(Page {
            items: collected,
            next_page_token,
            truncated,
        })
    }

    pub async fn list_messages(
        &self,
        command: &str,
        space: &str,
        max: Option<usize>,
        page_token: Option<&str>,
        all: bool,
        filters: MessageFilters,
    ) -> Result<Page<Value>, AppError> {
        let parent = normalize_space_name(space)?;
        let limit = max.unwrap_or(50);
        let mut collected = Vec::new();
        let mut token = page_token.map(ToOwned::to_owned);
        let mut next_page_token = None;

        loop {
            let remaining = limit.saturating_sub(collected.len());
            if remaining == 0 {
                break;
            }
            let page_size = remaining.clamp(1, 1000);
            let mut query = vec![
                ("pageSize".to_string(), page_size.to_string()),
                ("orderBy".to_string(), "createTime desc".to_string()),
            ];
            if let Some(token) = token.as_deref() {
                query.push(("pageToken".to_string(), token.to_string()));
            }
            if filters.include_deleted {
                query.push(("showDeleted".to_string(), "true".to_string()));
            }
            if let Some(filter) = message_filter(&filters) {
                query.push(("filter".to_string(), filter));
            }

            let body = self
                .request_json(
                    command,
                    Method::GET,
                    &format!("{parent}/messages"),
                    query,
                    None,
                )
                .await?;
            let mut messages = take_array(&body, "messages");
            collected.append(&mut messages);
            next_page_token = body
                .get("nextPageToken")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);

            if !all || next_page_token.is_none() {
                break;
            }
            token.clone_from(&next_page_token);
        }

        let truncated = next_page_token.is_some() && (!all || collected.len() >= limit);
        Ok(Page {
            items: collected,
            next_page_token,
            truncated,
        })
    }

    pub async fn send_message(
        &self,
        command: &str,
        space: &str,
        text: &str,
        thread: Option<&str>,
    ) -> Result<Value, AppError> {
        validate_message_text(text)?;
        let parent = normalize_space_name(space)?;
        let mut body = Map::new();
        body.insert("text".to_string(), json!(text));
        if let Some(thread) = thread {
            body.insert("thread".to_string(), json!({ "name": thread }));
        }

        self.request_json(
            command,
            Method::POST,
            &format!("{parent}/messages"),
            Vec::new(),
            Some(Value::Object(body)),
        )
        .await
    }

    pub async fn find_or_create_dm(
        &self,
        command: &str,
        email: &str,
    ) -> Result<(Value, &'static str), AppError> {
        let email = normalize_user_email(email)?;
        let query = vec![("name".to_string(), format!("users/{email}"))];
        match self
            .request_json(
                command,
                Method::GET,
                "spaces:findDirectMessage",
                query,
                None,
            )
            .await
        {
            Ok(space) => Ok((space, "found")),
            Err(error) if error.status() == Some(404) => {
                let body = json!({
                    "space": {
                        "spaceType": "DIRECT_MESSAGE",
                        "singleUserBotDm": false
                    },
                    "memberships": [
                        {
                            "member": {
                                "name": format!("users/{email}"),
                                "type": "HUMAN"
                            }
                        }
                    ]
                });
                let space = self
                    .request_json(
                        command,
                        Method::POST,
                        "spaces:setup",
                        Vec::new(),
                        Some(body),
                    )
                    .await?;
                Ok((space, "created"))
            }
            Err(error) => Err(error),
        }
    }

    pub async fn search_messages(
        &self,
        command: &str,
        args: &SearchArgs,
        max: Option<usize>,
        page_token: Option<&str>,
        all: bool,
        unread: bool,
    ) -> Result<(Page<Value>, String, SearchView, SearchOrder), AppError> {
        let query = if unread {
            "is_unread()".to_string()
        } else {
            args.query.join(" ")
        };
        let filter = compose_search_filter(&query, args)?;
        let view = if unread {
            SearchView::Full
        } else {
            args.view.clone()
        };
        let order = args.order.clone();
        let limit = max.unwrap_or(25);
        let mut collected = Vec::new();
        let mut token = page_token.map(ToOwned::to_owned);
        let mut next_page_token = None;

        loop {
            let remaining = limit.saturating_sub(collected.len());
            if remaining == 0 {
                break;
            }
            let page_size = remaining.clamp(1, 100);
            let mut body = json!({
                "filter": filter,
                "pageSize": page_size,
                "orderBy": order.api_value(),
                "view": view.api_value(),
            });
            if let Some(token) = token.as_deref() {
                body["pageToken"] = json!(token);
            }

            let response = self
                .request_json(
                    command,
                    Method::POST,
                    "spaces/-/messages:search",
                    Vec::new(),
                    Some(body),
                )
                .await?;
            let mut results = take_array(&response, "messages");
            if results.is_empty() {
                results = take_array(&response, "results");
            }
            collected.append(&mut results);
            next_page_token = response
                .get("nextPageToken")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);

            if !all || next_page_token.is_none() {
                break;
            }
            token.clone_from(&next_page_token);
        }

        let truncated = next_page_token.is_some() && (!all || collected.len() >= limit);
        Ok((
            Page {
                items: collected,
                next_page_token,
                truncated,
            },
            filter,
            view,
            order,
        ))
    }

    async fn request_json(
        &self,
        command: &str,
        method: Method,
        path: &str,
        query: Vec<(String, String)>,
        body: Option<Value>,
    ) -> Result<Value, AppError> {
        let url = format!("{}/{}", self.base_url, path.trim_start_matches('/'));
        let mut request = self
            .http
            .request(method, url)
            .bearer_auth(&self.access_token)
            .query(&query);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.map_err(|error| {
            AppError::google_api(
                command,
                0,
                json!({ "message": error.to_string(), "kind": "network" }),
            )
        })?;

        let status = response.status();
        let text = response.text().await.map_err(|error| {
            AppError::google_api(
                command,
                status.as_u16(),
                json!({ "message": error.to_string(), "kind": "read_response" }),
            )
        })?;
        let body = if text.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(&text).unwrap_or_else(|_| json!({ "raw": text }))
        };

        if !status.is_success() {
            return Err(AppError::google_api(command, status.as_u16(), body));
        }

        Ok(body)
    }

    async fn fetch_space_display_name(&self, space_name: &str) -> Option<String> {
        let url = format!("{}/{}", self.base_url, space_name);
        let body = self.get_json_best_effort(url, Vec::new()).await?;
        non_empty_string(body.get("displayName"))
    }

    async fn fetch_space_member_display_name(&self, space_name: &str) -> Option<String> {
        let url = format!("{}/{space_name}/members", self.base_url);
        let query = vec![
            ("pageSize".to_string(), "20".to_string()),
            ("filter".to_string(), "member.type = \"HUMAN\"".to_string()),
        ];
        let body = self.get_json_best_effort(url, query).await?;
        let user_names: Vec<String> = body
            .get("memberships")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|membership| {
                membership
                    .get("member")
                    .and_then(|member| member.get("name"))
                    .and_then(Value::as_str)
                    .filter(|name| is_exact_user_resource_name(name))
                    .map(ToOwned::to_owned)
            })
            .collect();
        if user_names.is_empty() {
            return None;
        }

        let display_names = self.fetch_user_display_names(&user_names).await;
        let mut names: Vec<String> = user_names
            .iter()
            .filter_map(|user_name| display_names.get(user_name).cloned())
            .collect();
        if names.is_empty() {
            return None;
        }

        let remaining = names.len().saturating_sub(5);
        names.truncate(5);
        if remaining > 0 {
            names.push(format!("{remaining} more"));
        }
        Some(names.join(", "))
    }

    async fn fetch_user_display_names(&self, user_names: &[String]) -> BTreeMap<String, String> {
        let user_names: Vec<&str> = user_names
            .iter()
            .filter_map(|name| {
                let id = name.strip_prefix("users/")?;
                if id == "app" {
                    None
                } else {
                    Some(name.as_str())
                }
            })
            .collect();
        if user_names.is_empty() {
            return BTreeMap::new();
        }

        let people_base = std::env::var("GCHAT_PEOPLE_API_BASE")
            .unwrap_or_else(|_| DEFAULT_PEOPLE_API_BASE.to_string());
        let url = format!("{}/people:batchGet", people_base.trim_end_matches('/'));
        let mut query = vec![
            ("personFields".to_string(), "names".to_string()),
            (
                "sources".to_string(),
                "READ_SOURCE_TYPE_PROFILE".to_string(),
            ),
        ];
        for user_name in &user_names {
            if let Some(id) = user_name.strip_prefix("users/") {
                query.push(("resourceNames".to_string(), format!("people/{id}")));
            }
        }

        let Some(body) = self.get_json_best_effort(url, query).await else {
            return BTreeMap::new();
        };

        let mut display_names = BTreeMap::new();
        for response in body
            .get("responses")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(person) = response.get("person") else {
                continue;
            };
            let Some(resource_name) = person.get("resourceName").and_then(Value::as_str) else {
                continue;
            };
            let Some(id) = resource_name.strip_prefix("people/") else {
                continue;
            };
            let Some(display_name) = person_display_name(person) else {
                continue;
            };
            display_names.insert(format!("users/{id}"), display_name);
        }
        display_names
    }

    async fn get_json_best_effort(
        &self,
        url: String,
        query: Vec<(String, String)>,
    ) -> Option<Value> {
        let response = self
            .http
            .get(url)
            .bearer_auth(&self.access_token)
            .query(&query)
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        response.json::<Value>().await.ok()
    }
}

pub fn normalize_space_name(input: &str) -> Result<String, AppError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(AppError::usage(
            "chat",
            "space id is empty",
            json!({ "space": input }),
        ));
    }
    if input.contains('?') || input.contains('#') || input.contains(' ') {
        return Err(AppError::usage(
            "chat",
            "space id contains invalid characters",
            json!({ "space": input }),
        ));
    }
    if input.starts_with("spaces/") {
        Ok(input.to_string())
    } else {
        Ok(format!("spaces/{input}"))
    }
}

pub fn compose_search_filter(query: &str, args: &SearchArgs) -> Result<String, AppError> {
    let mut filters = vec![query.trim().to_string()];
    if filters[0].is_empty() {
        return Err(AppError::usage(
            "search",
            "search query must not be empty",
            json!({}),
        ));
    }
    if let Some(space) = args.space.as_deref() {
        filters.push(format!("space.name = \"{}\"", normalize_space_name(space)?));
    }
    if let Some(sender) = args.sender.as_deref() {
        filters.push(format!(
            "sender.name = \"{}\"",
            normalize_user_name(sender)?
        ));
    }
    if let Some(after) = args.after.as_deref() {
        filters.push(format!("createTime >= \"{after}\""));
    }
    if let Some(before) = args.before.as_deref() {
        filters.push(format!("createTime < \"{before}\""));
    }
    if args.has_link {
        filters.push("has_link()".to_string());
    }
    if args.attachments {
        filters.push("attachment:*".to_string());
    }
    Ok(filters.join(" AND "))
}

fn message_filter(filters: &MessageFilters) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(thread) = filters.thread.as_deref() {
        parts.push(format!("thread.name = \"{thread}\""));
    }
    if let Some(after) = filters.after.as_deref() {
        parts.push(format!("createTime > \"{after}\""));
    }
    if let Some(before) = filters.before.as_deref() {
        parts.push(format!("createTime < \"{before}\""));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" AND "))
    }
}

fn validate_message_text(text: &str) -> Result<(), AppError> {
    if text.trim().is_empty() {
        return Err(AppError::usage(
            "chat.send",
            "message text must not be empty",
            json!({}),
        ));
    }
    if text.len() > 32_000 {
        return Err(AppError::usage(
            "chat.send",
            "message text is too large",
            json!({ "bytes": text.len(), "limit": 32000 }),
        ));
    }
    Ok(())
}

fn normalize_user_email(email: &str) -> Result<String, AppError> {
    let email = email.trim().to_ascii_lowercase();
    if email.contains('@') && !email.contains(char::is_whitespace) {
        Ok(email)
    } else {
        Err(AppError::usage(
            "chat.dm",
            "email address is malformed",
            json!({ "email": email }),
        ))
    }
}

fn normalize_user_name(sender: &str) -> Result<String, AppError> {
    let sender = sender.trim();
    if sender.starts_with("users/") {
        Ok(sender.to_string())
    } else {
        Ok(format!("users/{}", normalize_user_email(sender)?))
    }
}

fn take_array(body: &Value, key: &str) -> Vec<Value> {
    body.get(key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

#[derive(Debug, Default)]
struct DisplayNameTargets {
    spaces: Vec<String>,
    space_member_fallbacks: Vec<String>,
    users: Vec<String>,
}

impl DisplayNameTargets {
    fn is_empty(&self) -> bool {
        self.spaces.is_empty() && self.users.is_empty()
    }
}

#[derive(Debug, Default)]
struct DisplayNames {
    spaces: BTreeMap<String, String>,
    users: BTreeMap<String, String>,
}

fn collect_display_name_targets(value: &Value) -> DisplayNameTargets {
    fn walk(
        value: &Value,
        parent_key: Option<&str>,
        spaces: &mut BTreeSet<String>,
        space_member_fallbacks: &mut BTreeSet<String>,
        users: &mut BTreeSet<String>,
    ) {
        match value {
            Value::Object(object) => {
                if object_missing_display_name(object)
                    && let Some(name) = object.get("name").and_then(Value::as_str)
                {
                    if is_exact_space_resource_name(name)
                        && should_resolve_space_display_name(parent_key, object)
                    {
                        spaces.insert(name.to_string());
                        if should_derive_space_display_name_from_members(parent_key, object) {
                            space_member_fallbacks.insert(name.to_string());
                        }
                    } else if is_exact_user_resource_name(name) {
                        users.insert(name.to_string());
                    }
                }
                for (key, child) in object {
                    walk(
                        child,
                        Some(key.as_str()),
                        spaces,
                        space_member_fallbacks,
                        users,
                    );
                }
            }
            Value::Array(array) => {
                for child in array {
                    walk(child, parent_key, spaces, space_member_fallbacks, users);
                }
            }
            _ => {}
        }
    }

    let mut spaces = BTreeSet::new();
    let mut space_member_fallbacks = BTreeSet::new();
    let mut users = BTreeSet::new();
    walk(
        value,
        None,
        &mut spaces,
        &mut space_member_fallbacks,
        &mut users,
    );
    DisplayNameTargets {
        spaces: spaces.into_iter().collect(),
        space_member_fallbacks: space_member_fallbacks.into_iter().collect(),
        users: users.into_iter().collect(),
    }
}

fn apply_display_names(value: &mut Value, display_names: &DisplayNames) {
    match value {
        Value::Object(object) => {
            if object_missing_display_name(object)
                && let Some(name) = object.get("name").and_then(Value::as_str)
            {
                let display_name = if is_exact_space_resource_name(name) {
                    display_names.spaces.get(name)
                } else if is_exact_user_resource_name(name) {
                    display_names.users.get(name)
                } else {
                    None
                };
                if let Some(display_name) = display_name {
                    object.insert("displayName".to_string(), json!(display_name));
                }
            }
            for child in object.values_mut() {
                apply_display_names(child, display_names);
            }
        }
        Value::Array(array) => {
            for child in array {
                apply_display_names(child, display_names);
            }
        }
        _ => {}
    }
}

fn object_missing_display_name(object: &Map<String, Value>) -> bool {
    object
        .get("displayName")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
        .is_empty()
}

fn should_resolve_space_display_name(
    parent_key: Option<&str>,
    object: &Map<String, Value>,
) -> bool {
    parent_key == Some("space") || is_compact_resource_object(object)
}

fn should_derive_space_display_name_from_members(
    parent_key: Option<&str>,
    object: &Map<String, Value>,
) -> bool {
    parent_key == Some("space") || is_compact_resource_object(object)
}

fn is_compact_resource_object(object: &Map<String, Value>) -> bool {
    object
        .keys()
        .all(|key| matches!(key.as_str(), "name" | "displayName"))
}

fn is_exact_space_resource_name(name: &str) -> bool {
    name.strip_prefix("spaces/")
        .is_some_and(|id| !id.is_empty() && !id.contains('/'))
}

fn is_exact_user_resource_name(name: &str) -> bool {
    name.strip_prefix("users/")
        .is_some_and(|id| !id.is_empty() && !id.contains('/'))
}

fn non_empty_string(value: Option<&Value>) -> Option<String> {
    value?
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn person_display_name(person: &Value) -> Option<String> {
    for name in person
        .get("names")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(display_name) = non_empty_string(name.get("displayName")) {
            return Some(display_name);
        }
        if let Some(unstructured_name) = non_empty_string(name.get("unstructuredName")) {
            return Some(unstructured_name);
        }
        let given_name = non_empty_string(name.get("givenName"));
        let family_name = non_empty_string(name.get("familyName"));
        let combined = [given_name.as_deref(), family_name.as_deref()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
        if !combined.is_empty() {
            return Some(combined);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{SearchArgs, SearchOrder, SearchView};

    #[test]
    fn normalizes_raw_space_id() {
        assert_eq!(normalize_space_name("AAAA").unwrap(), "spaces/AAAA");
        assert_eq!(normalize_space_name("spaces/AAAA").unwrap(), "spaces/AAAA");
    }

    #[test]
    fn search_filter_preserves_user_query_and_adds_filters() {
        let args = SearchArgs {
            query: vec!["from(\"me\")".to_string()],
            space: Some("AAAA".to_string()),
            sender: Some("person@example.com".to_string()),
            after: Some("2026-01-01T00:00:00Z".to_string()),
            before: None,
            has_link: true,
            attachments: false,
            view: SearchView::Basic,
            order: SearchOrder::CreateTime,
        };
        let filter = compose_search_filter("from(\"me\")", &args).unwrap();
        assert!(filter.starts_with("from(\"me\") AND"));
        assert!(filter.contains("space.name = \"spaces/AAAA\""));
        assert!(filter.contains("sender.name = \"users/person@example.com\""));
        assert!(filter.contains("has_link()"));
    }

    #[test]
    fn display_name_targets_include_referenced_space_and_sender() {
        let data = json!({
            "results": [
                {
                    "message": {
                        "name": "spaces/A/messages/M",
                        "sender": {
                            "name": "users/123",
                            "type": "HUMAN"
                        },
                        "space": {
                            "name": "spaces/A"
                        },
                        "thread": {
                            "name": "spaces/A/threads/T"
                        }
                    }
                }
            ],
            "spaces": [
                {
                    "name": "spaces/B",
                    "spaceType": "DIRECT_MESSAGE",
                    "spaceUri": "https://chat.google.com/dm/B"
                }
            ]
        });

        let targets = collect_display_name_targets(&data);

        assert_eq!(targets.spaces, vec!["spaces/A"]);
        assert_eq!(targets.space_member_fallbacks, vec!["spaces/A"]);
        assert_eq!(targets.users, vec!["users/123"]);
    }

    #[test]
    fn apply_display_names_adds_names_without_touching_message_resources() {
        let mut data = json!({
            "message": {
                "name": "spaces/A/messages/M",
                "sender": {
                    "name": "users/123",
                    "type": "HUMAN"
                },
                "space": {
                    "name": "spaces/A"
                }
            }
        });
        let mut display_names = DisplayNames::default();
        display_names
            .spaces
            .insert("spaces/A".to_string(), "Direct Person".to_string());
        display_names
            .users
            .insert("users/123".to_string(), "Direct Person".to_string());

        apply_display_names(&mut data, &display_names);

        assert_eq!(
            data["message"]["sender"]["displayName"],
            json!("Direct Person")
        );
        assert_eq!(
            data["message"]["space"]["displayName"],
            json!("Direct Person")
        );
        assert!(data["message"].get("displayName").is_none());
    }

    #[test]
    fn person_display_name_uses_best_available_name() {
        let person = json!({
            "names": [
                {
                    "givenName": "Ada",
                    "familyName": "Lovelace"
                }
            ]
        });

        assert_eq!(
            person_display_name(&person),
            Some("Ada Lovelace".to_string())
        );
    }
}
