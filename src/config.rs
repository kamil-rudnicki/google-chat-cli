use crate::error::AppError;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

const CONFIG_SCHEMA_VERSION: u32 = 1;
const DISPLAY_NAME_CACHE_SCHEMA_VERSION: u32 = 1;
const DISPLAY_NAME_CACHE_TTL_HOURS: i64 = 24;
#[cfg(target_os = "macos")]
const SERVICE_NAME: &str = "gchat";

#[derive(Debug, Clone)]
pub struct ConfigStore {
    pub root: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigFile {
    pub schema_version: u32,
    pub default_account: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthClientFile {
    pub client_id: String,
    pub client_secret: String,
    pub auth_uri: String,
    pub token_uri: String,
    pub redirect_uris: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RawOAuthFile {
    installed: Option<RawOAuthClient>,
    web: Option<RawOAuthClient>,
}

#[derive(Debug, Deserialize)]
struct RawOAuthClient {
    client_id: String,
    client_secret: String,
    auth_uri: String,
    token_uri: String,
    #[serde(default)]
    redirect_uris: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountRecord {
    pub email: String,
    pub verified: bool,
    pub scopes: Vec<String>,
    pub token_storage: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    pub expires_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountSummary {
    pub email: String,
    pub verified: bool,
    pub scopes: Vec<String>,
    pub token_storage: String,
    pub expires_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedCredentials {
    pub config_dir: String,
    pub client_id: String,
    pub source: String,
    pub stored_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadBaseline {
    pub unread_search_after: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadBaselinesFile {
    accounts: BTreeMap<String, ReadBaseline>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplayNameCacheEntry {
    pub display_name: String,
    pub cached_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplayNameCache {
    pub schema_version: u32,
    pub entries: BTreeMap<String, DisplayNameCacheEntry>,
}

impl Default for DisplayNameCache {
    fn default() -> Self {
        Self {
            schema_version: DISPLAY_NAME_CACHE_SCHEMA_VERSION,
            entries: BTreeMap::new(),
        }
    }
}

impl DisplayNameCache {
    pub fn fresh_display_name(&self, resource_name: &str, now: DateTime<Utc>) -> Option<String> {
        let entry = self.entries.get(resource_name)?;
        if entry.display_name.trim().is_empty() {
            return None;
        }
        let cached_at = DateTime::parse_from_rfc3339(&entry.cached_at)
            .ok()?
            .with_timezone(&Utc);
        if now.signed_duration_since(cached_at) > Duration::hours(DISPLAY_NAME_CACHE_TTL_HOURS) {
            return None;
        }
        Some(entry.display_name.clone())
    }

    pub fn insert(&mut self, resource_name: String, display_name: String, now: DateTime<Utc>) {
        self.schema_version = DISPLAY_NAME_CACHE_SCHEMA_VERSION;
        self.entries.insert(
            resource_name,
            DisplayNameCacheEntry {
                display_name,
                cached_at: now.to_rfc3339(),
            },
        );
    }

    pub fn retain_fresh(&mut self, now: DateTime<Utc>) -> bool {
        let before = self.entries.len();
        self.entries.retain(|_, entry| {
            DateTime::parse_from_rfc3339(&entry.cached_at)
                .map(|cached_at| {
                    now.signed_duration_since(cached_at.with_timezone(&Utc))
                        <= Duration::hours(DISPLAY_NAME_CACHE_TTL_HOURS)
                })
                .unwrap_or(false)
        });
        before != self.entries.len()
    }
}

impl ConfigStore {
    pub fn new(root: PathBuf) -> Result<Self, AppError> {
        let store = Self { root };
        store.ensure_dirs()?;
        store.ensure_config_file()?;
        Ok(store)
    }

    pub fn resolve_root(cli_config_dir: Option<PathBuf>) -> Result<PathBuf, AppError> {
        if let Some(path) = cli_config_dir {
            return expand_tilde(path);
        }

        if let Ok(path) = std::env::var("GCHAT_CONFIG_DIR")
            && !path.trim().is_empty()
        {
            return expand_tilde(PathBuf::from(path));
        }

        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| AppError::local_io("config", "HOME is not set", json!({})))?;

        Ok(home.join(".config").join("gchat"))
    }

    pub fn config_path(&self) -> PathBuf {
        self.root.join("config.json")
    }

    pub fn oauth_client_path(&self) -> PathBuf {
        self.root.join("oauth-client.json")
    }

    pub fn accounts_dir(&self) -> PathBuf {
        self.root.join("accounts")
    }

    pub fn cache_dir(&self) -> PathBuf {
        self.root.join("cache")
    }

    pub fn read_baselines_path(&self) -> PathBuf {
        self.cache_dir().join("read-baselines.json")
    }

    pub fn display_name_cache_path(&self) -> PathBuf {
        self.cache_dir().join("display-names.json")
    }

    pub fn account_path(&self, email: &str) -> PathBuf {
        self.accounts_dir()
            .join(format!("{}.json", normalize_email(email)))
    }

    pub fn load_config(&self) -> Result<ConfigFile, AppError> {
        let bytes = fs::read(self.config_path()).map_err(|error| {
            AppError::local_io(
                "config",
                "failed to read config file",
                json!({ "path": self.config_path(), "ioError": error.to_string() }),
            )
        })?;
        serde_json::from_slice(&bytes).map_err(|error| {
            AppError::local_io(
                "config",
                "failed to parse config file",
                json!({ "path": self.config_path(), "jsonError": error.to_string() }),
            )
        })
    }

    pub fn save_config(&self, config: &ConfigFile) -> Result<(), AppError> {
        write_json_restricted(&self.config_path(), config, "config")
    }

    pub fn import_credentials(&self, source: &Path) -> Result<ImportedCredentials, AppError> {
        let source = expand_tilde(source.to_path_buf())?;
        let bytes = fs::read(&source).map_err(|error| {
            AppError::local_io(
                "auth.credentials",
                "failed to read OAuth client credentials",
                json!({ "path": source, "ioError": error.to_string() }),
            )
        })?;

        let raw: RawOAuthFile = serde_json::from_slice(&bytes).map_err(|error| {
            AppError::usage(
                "auth.credentials",
                "OAuth client credentials are not valid JSON",
                json!({ "path": source, "jsonError": error.to_string() }),
            )
        })?;

        let raw_client = raw.installed.or(raw.web).ok_or_else(|| {
            AppError::usage(
                "auth.credentials",
                "OAuth client JSON must contain an installed or web client object",
                json!({ "path": source }),
            )
        })?;

        if raw_client.client_id.trim().is_empty()
            || raw_client.client_secret.trim().is_empty()
            || raw_client.auth_uri.trim().is_empty()
            || raw_client.token_uri.trim().is_empty()
        {
            return Err(AppError::usage(
                "auth.credentials",
                "OAuth client JSON is missing required client fields",
                json!({ "path": source }),
            ));
        }

        let normalized = OAuthClientFile {
            client_id: raw_client.client_id,
            client_secret: raw_client.client_secret,
            auth_uri: raw_client.auth_uri,
            token_uri: raw_client.token_uri,
            redirect_uris: raw_client.redirect_uris,
        };

        write_json_restricted(&self.oauth_client_path(), &normalized, "auth.credentials")?;

        Ok(ImportedCredentials {
            config_dir: self.root.display().to_string(),
            client_id: redact_client_id(&normalized.client_id),
            source: source.display().to_string(),
            stored_at: self.oauth_client_path().display().to_string(),
        })
    }

    pub fn load_oauth_client(&self, command: &str) -> Result<OAuthClientFile, AppError> {
        let path = self.oauth_client_path();
        let bytes = fs::read(&path).map_err(|error| {
            AppError::missing_auth(
                command,
                "OAuth client credentials have not been imported",
                json!({
                    "path": path,
                    "ioError": error.to_string(),
                    "fix": "run `gchat auth credentials <client-secret-json>`"
                }),
            )
        })?;

        serde_json::from_slice(&bytes).map_err(|error| {
            AppError::local_io(
                command,
                "stored OAuth client credentials are malformed",
                json!({ "path": path, "jsonError": error.to_string() }),
            )
        })
    }

    pub fn save_account_with_refresh(
        &self,
        mut account: AccountRecord,
        refresh_token: &str,
    ) -> Result<AccountRecord, AppError> {
        if store_refresh_token_in_keychain(&account.email, refresh_token) {
            account.token_storage = "keychain".to_string();
            account.refresh_token = None;
        } else {
            account.token_storage = "file".to_string();
            account.refresh_token = Some(refresh_token.to_string());
        }

        write_json_restricted(&self.account_path(&account.email), &account, "auth.add")?;
        let mut config = self.load_config()?;
        if config.default_account.is_none() {
            config.default_account = Some(account.email.clone());
            self.save_config(&config)?;
        }
        Ok(account)
    }

    pub fn load_account(&self, email: &str, command: &str) -> Result<AccountRecord, AppError> {
        let email = normalize_email(email);
        let path = self.account_path(&email);
        let bytes = fs::read(&path).map_err(|error| {
            AppError::missing_auth(
                command,
                "account is not configured",
                json!({
                    "email": email,
                    "path": path,
                    "ioError": error.to_string(),
                    "fix": "run `gchat auth add <email>`"
                }),
            )
        })?;
        serde_json::from_slice(&bytes).map_err(|error| {
            AppError::local_io(
                command,
                "stored account file is malformed",
                json!({ "path": path, "jsonError": error.to_string() }),
            )
        })
    }

    pub fn load_refresh_token(
        &self,
        account: &AccountRecord,
        command: &str,
    ) -> Result<String, AppError> {
        if account.token_storage == "keychain" {
            if let Some(token) = load_refresh_token_from_keychain(&account.email) {
                return Ok(token);
            }
            return Err(AppError::missing_auth(
                command,
                "refresh token is missing from the keychain",
                json!({
                    "email": account.email,
                    "tokenStorage": account.token_storage,
                    "fix": "run `gchat auth add <email>` again"
                }),
            ));
        }

        account.refresh_token.clone().ok_or_else(|| {
            AppError::missing_auth(
                command,
                "refresh token is missing from account storage",
                json!({
                    "email": account.email,
                    "tokenStorage": account.token_storage,
                    "fix": "run `gchat auth add <email>` again"
                }),
            )
        })
    }

    pub fn list_accounts(&self) -> Result<Vec<AccountSummary>, AppError> {
        let mut accounts = Vec::new();
        for entry in fs::read_dir(self.accounts_dir()).map_err(|error| {
            AppError::local_io(
                "auth.list",
                "failed to read accounts directory",
                json!({ "path": self.accounts_dir(), "ioError": error.to_string() }),
            )
        })? {
            let entry = entry.map_err(|error| {
                AppError::local_io(
                    "auth.list",
                    "failed to read an account directory entry",
                    json!({ "ioError": error.to_string() }),
                )
            })?;
            if entry.path().extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let bytes = fs::read(entry.path()).map_err(|error| {
                AppError::local_io(
                    "auth.list",
                    "failed to read account file",
                    json!({ "path": entry.path(), "ioError": error.to_string() }),
                )
            })?;
            let account: AccountRecord = serde_json::from_slice(&bytes).map_err(|error| {
                AppError::local_io(
                    "auth.list",
                    "failed to parse account file",
                    json!({ "path": entry.path(), "jsonError": error.to_string() }),
                )
            })?;
            accounts.push(AccountSummary {
                email: account.email,
                verified: account.verified,
                scopes: account.scopes,
                token_storage: account.token_storage,
                expires_at: account.expires_at,
                created_at: account.created_at,
                updated_at: account.updated_at,
            });
        }
        accounts.sort_by(|left, right| left.email.cmp(&right.email));
        Ok(accounts)
    }

    pub fn load_unread_search_after(
        &self,
        email: &str,
        command: &str,
    ) -> Result<Option<String>, AppError> {
        let baselines = self.load_read_baselines(command)?;
        Ok(baselines
            .accounts
            .get(&normalize_email(email))
            .map(|baseline| baseline.unread_search_after.clone()))
    }

    pub fn save_unread_search_after(
        &self,
        email: &str,
        unread_search_after: &str,
        command: &str,
    ) -> Result<ReadBaseline, AppError> {
        let mut baselines = self.load_read_baselines(command)?;
        let baseline = ReadBaseline {
            unread_search_after: unread_search_after.to_string(),
            updated_at: chrono::Utc::now().to_rfc3339(),
        };
        baselines
            .accounts
            .insert(normalize_email(email), baseline.clone());
        write_json_restricted(&self.read_baselines_path(), &baselines, command)?;
        Ok(baseline)
    }

    pub fn load_display_name_cache(&self) -> DisplayNameCache {
        let path = self.display_name_cache_path();
        let Ok(bytes) = fs::read(&path) else {
            return DisplayNameCache::default();
        };
        serde_json::from_slice(&bytes).unwrap_or_default()
    }

    pub fn save_display_name_cache(
        &self,
        cache: &DisplayNameCache,
        command: &str,
    ) -> Result<(), AppError> {
        write_json_restricted(&self.display_name_cache_path(), cache, command)
    }

    fn load_read_baselines(&self, command: &str) -> Result<ReadBaselinesFile, AppError> {
        let path = self.read_baselines_path();
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ReadBaselinesFile::default());
            }
            Err(error) => {
                return Err(AppError::local_io(
                    command,
                    "failed to read read-baselines file",
                    json!({ "path": path, "ioError": error.to_string() }),
                ));
            }
        };
        serde_json::from_slice(&bytes).map_err(|error| {
            AppError::local_io(
                command,
                "failed to parse read-baselines file",
                json!({ "path": path, "jsonError": error.to_string() }),
            )
        })
    }

    pub fn remove_account(&self, email: &str) -> Result<bool, AppError> {
        let email = normalize_email(email);
        delete_refresh_token_from_keychain(&email);
        let path = self.account_path(&email);
        match fs::remove_file(&path) {
            Ok(()) => {
                let mut config = self.load_config()?;
                if config.default_account.as_deref() == Some(email.as_str()) {
                    config.default_account = None;
                    self.save_config(&config)?;
                }
                Ok(true)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(AppError::local_io(
                "auth.remove",
                "failed to remove account file",
                json!({ "path": path, "ioError": error.to_string() }),
            )),
        }
    }

    pub fn select_account(
        &self,
        requested: Option<&str>,
        command: &str,
    ) -> Result<String, AppError> {
        if let Some(email) = requested {
            let email = normalize_email(email);
            self.load_account(&email, command)?;
            return Ok(email);
        }

        let config = self.load_config()?;
        if let Some(email) = config.default_account {
            self.load_account(&email, command)?;
            return Ok(email);
        }

        let accounts = self.list_accounts()?;
        if accounts.len() == 1 {
            return Ok(accounts[0].email.clone());
        }

        Err(AppError::missing_auth(
            command,
            "no account selected",
            json!({
                "configuredAccounts": accounts.iter().map(|account| account.email.clone()).collect::<Vec<_>>(),
                "fix": "pass `--account <email>` or run `gchat auth add <email>`"
            }),
        ))
    }

    fn ensure_dirs(&self) -> Result<(), AppError> {
        create_dir_private(&self.root, "config")?;
        create_dir_private(&self.accounts_dir(), "config")?;
        create_dir_private(&self.cache_dir(), "config")?;
        Ok(())
    }

    fn ensure_config_file(&self) -> Result<(), AppError> {
        if self.config_path().exists() {
            return Ok(());
        }
        self.save_config(&ConfigFile {
            schema_version: CONFIG_SCHEMA_VERSION,
            default_account: None,
        })
    }
}

pub fn normalize_email(email: &str) -> String {
    email.trim().to_ascii_lowercase()
}

pub fn expand_tilde(path: PathBuf) -> Result<PathBuf, AppError> {
    let path_string = path.to_string_lossy();
    if path_string == "~" {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| AppError::local_io("config", "HOME is not set", json!({})))?;
        return Ok(home);
    }
    if let Some(rest) = path_string.strip_prefix("~/") {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| AppError::local_io("config", "HOME is not set", json!({})))?;
        return Ok(home.join(rest));
    }
    Ok(path)
}

fn redact_client_id(client_id: &str) -> String {
    if let Some((prefix, _)) = client_id.split_once(".apps.googleusercontent.com") {
        let visible = prefix.chars().take(8).collect::<String>();
        return format!("{visible}....apps.googleusercontent.com");
    }
    let visible = client_id.chars().take(8).collect::<String>();
    format!("{visible}...")
}

fn create_dir_private(path: &Path, command: &str) -> Result<(), AppError> {
    fs::create_dir_all(path).map_err(|error| {
        AppError::local_io(
            command,
            "failed to create config directory",
            json!({ "path": path, "ioError": error.to_string() }),
        )
    })?;

    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|error| {
        AppError::local_io(
            command,
            "failed to set config directory permissions",
            json!({ "path": path, "ioError": error.to_string() }),
        )
    })?;

    Ok(())
}

fn write_json_restricted<T: Serialize>(
    path: &Path,
    value: &T,
    command: &str,
) -> Result<(), AppError> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| {
        AppError::local_io(
            command,
            "failed to serialize JSON",
            json!({ "path": path, "jsonError": error.to_string() }),
        )
    })?;

    #[cfg(unix)]
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| {
            AppError::local_io(
                command,
                "failed to open file for writing",
                json!({ "path": path, "ioError": error.to_string() }),
            )
        })?;

    #[cfg(not(unix))]
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)
        .map_err(|error| {
            AppError::local_io(
                command,
                "failed to open file for writing",
                json!({ "path": path, "ioError": error.to_string() }),
            )
        })?;

    file.write_all(&bytes).map_err(|error| {
        AppError::local_io(
            command,
            "failed to write file",
            json!({ "path": path, "ioError": error.to_string() }),
        )
    })?;
    file.write_all(b"\n").map_err(|error| {
        AppError::local_io(
            command,
            "failed to finish writing file",
            json!({ "path": path, "ioError": error.to_string() }),
        )
    })?;

    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|error| {
        AppError::local_io(
            command,
            "failed to set file permissions",
            json!({ "path": path, "ioError": error.to_string() }),
        )
    })?;

    Ok(())
}

#[cfg(target_os = "macos")]
fn store_refresh_token_in_keychain(email: &str, refresh_token: &str) -> bool {
    keyring::Entry::new(SERVICE_NAME, email)
        .and_then(|entry| entry.set_password(refresh_token))
        .is_ok()
}

#[cfg(not(target_os = "macos"))]
fn store_refresh_token_in_keychain(_email: &str, _refresh_token: &str) -> bool {
    false
}

#[cfg(target_os = "macos")]
fn load_refresh_token_from_keychain(email: &str) -> Option<String> {
    keyring::Entry::new(SERVICE_NAME, email)
        .ok()
        .and_then(|entry| entry.get_password().ok())
}

#[cfg(not(target_os = "macos"))]
fn load_refresh_token_from_keychain(_email: &str) -> Option<String> {
    None
}

#[cfg(target_os = "macos")]
fn delete_refresh_token_from_keychain(email: &str) {
    if let Ok(entry) = keyring::Entry::new(SERVICE_NAME, email) {
        let _ = entry.delete_password();
    }
}

#[cfg(not(target_os = "macos"))]
fn delete_refresh_token_from_keychain(_email: &str) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_path_uses_dot_config_gchat() {
        let root = ConfigStore::resolve_root(None).unwrap();
        assert!(root.ends_with(Path::new(".config").join("gchat")));
    }

    #[test]
    fn normalizes_email_lowercase() {
        assert_eq!(
            normalize_email(" K.RUDNICKI@TIMECAMP.COM "),
            "k.rudnicki@timecamp.com"
        );
    }

    #[test]
    fn display_name_cache_returns_fresh_entries() {
        let now = Utc::now();
        let mut cache = DisplayNameCache::default();
        cache.insert("users/123".to_string(), "Ada Lovelace".to_string(), now);

        assert_eq!(
            cache.fresh_display_name("users/123", now),
            Some("Ada Lovelace".to_string())
        );
    }

    #[test]
    fn display_name_cache_ignores_stale_entries() {
        let now = Utc::now();
        let mut cache = DisplayNameCache::default();
        cache.insert(
            "users/123".to_string(),
            "Ada Lovelace".to_string(),
            now - Duration::hours(25),
        );

        assert_eq!(cache.fresh_display_name("users/123", now), None);
    }
}
