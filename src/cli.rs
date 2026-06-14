use clap::{Args, Parser, Subcommand, ValueEnum};
use serde_json::{Value, json};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "gchat",
    disable_help_flag = true,
    disable_version_flag = true,
    subcommand_required = true
)]
pub struct Cli {
    #[arg(long, global = true)]
    pub account: Option<String>,

    #[arg(long, global = true, env = "GCHAT_CONFIG_DIR")]
    pub config_dir: Option<PathBuf>,

    #[arg(long, global = true)]
    pub max: Option<usize>,

    #[arg(long, global = true)]
    pub page_token: Option<String>,

    #[arg(long, global = true)]
    pub all: bool,

    #[arg(long, global = true)]
    pub verbose: bool,

    #[arg(long, global = true)]
    pub progress: bool,

    #[arg(long, global = true)]
    pub no_display_names: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
    Chat {
        #[command(subcommand)]
        command: ChatCommand,
    },
    Search(SearchArgs),
    Mark {
        #[command(subcommand)]
        command: MarkCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum AuthCommand {
    Credentials(CredentialsArgs),
    Add(AccountEmailArgs),
    List,
    Remove(AccountEmailArgs),
}

#[derive(Debug, Args)]
pub struct CredentialsArgs {
    pub client_secret_json: PathBuf,
}

#[derive(Debug, Args)]
pub struct AccountEmailArgs {
    pub email: String,
}

#[derive(Debug, Subcommand)]
pub enum ChatCommand {
    List(ListSpacesArgs),
    Spaces {
        #[command(subcommand)]
        command: SpacesCommand,
    },
    Messages(MessagesArgs),
    Send(SendArgs),
    Dm {
        #[command(subcommand)]
        command: DmCommand,
    },
    Threads(ThreadsArgs),
}

#[derive(Debug, Subcommand)]
pub enum SpacesCommand {
    List(ListSpacesArgs),
}

#[derive(Debug, Args)]
pub struct ListSpacesArgs {
    #[arg(long = "type")]
    pub space_type: Option<SpaceType>,
}

#[derive(Debug, Clone, ValueEnum)]
pub enum SpaceType {
    Space,
    GroupChat,
    DirectMessage,
}

impl SpaceType {
    pub fn api_value(&self) -> &'static str {
        match self {
            Self::Space => "SPACE",
            Self::GroupChat => "GROUP_CHAT",
            Self::DirectMessage => "DIRECT_MESSAGE",
        }
    }
}

#[derive(Debug, Args)]
pub struct MessagesArgs {
    pub space_id: String,

    #[arg(long)]
    pub thread: Option<String>,

    #[arg(long)]
    pub before: Option<String>,

    #[arg(long)]
    pub after: Option<String>,

    #[arg(long)]
    pub include_deleted: bool,
}

#[derive(Debug, Args)]
pub struct SendArgs {
    #[arg(long)]
    pub space: String,

    #[arg(long)]
    pub text: String,

    #[arg(long)]
    pub thread: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum DmCommand {
    Space(AccountEmailArgs),
    Send(DmSendArgs),
}

#[derive(Debug, Args)]
pub struct DmSendArgs {
    pub email: String,

    #[arg(long)]
    pub text: String,
}

#[derive(Debug, Args)]
pub struct ThreadsArgs {
    pub space_id: String,
}

#[derive(Debug, Subcommand)]
pub enum MarkCommand {
    Read(MarkReadArgs),
}

#[derive(Debug, Args)]
pub struct MarkReadArgs {
    #[arg(long)]
    pub space: Option<String>,

    #[arg(long)]
    pub remote: bool,

    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Args, Clone)]
pub struct SearchArgs {
    #[arg(value_name = "QUERY", num_args = 1..)]
    pub query: Vec<String>,

    #[arg(long)]
    pub space: Option<String>,

    #[arg(long)]
    pub sender: Option<String>,

    #[arg(long)]
    pub after: Option<String>,

    #[arg(long)]
    pub before: Option<String>,

    #[arg(long)]
    pub has_link: bool,

    #[arg(long)]
    pub attachments: bool,

    #[arg(long)]
    pub include_marked: bool,

    #[arg(long, value_enum)]
    pub view: Option<SearchView>,

    #[arg(long, value_enum, default_value = "create-time")]
    pub order: SearchOrder,
}

#[derive(Debug, Clone, ValueEnum)]
pub enum SearchView {
    Basic,
    Full,
}

impl SearchView {
    pub fn api_value(&self) -> &'static str {
        match self {
            Self::Basic => "SEARCH_MESSAGES_VIEW_BASIC",
            Self::Full => "SEARCH_MESSAGES_VIEW_FULL",
        }
    }
}

#[derive(Debug, Clone, ValueEnum)]
pub enum SearchOrder {
    CreateTime,
    Relevance,
}

impl SearchOrder {
    pub fn api_value(&self) -> &'static str {
        match self {
            Self::CreateTime => "createTime desc",
            Self::Relevance => "relevance desc",
        }
    }
}

pub fn contains_flag(args: &[String], flag: &str) -> bool {
    args.iter().skip(1).any(|arg| arg == flag)
}

pub fn help_json() -> Value {
    json!({
        "usage": "gchat <command> [options]",
        "globalFlags": [
            "--account <email>",
            "--config-dir <path>",
            "--max <n>",
            "--page-token <token>",
            "--all",
            "--verbose",
            "--progress",
            "--no-display-names",
            "--help",
            "--version"
        ],
        "commands": [
            "auth credentials <client-secret-json>",
            "auth add <email>",
            "auth list",
            "auth remove <email>",
            "chat list",
            "chat spaces list",
            "chat messages <space-id>",
            "chat send --space <space-id> --text <text>",
            "chat dm space <email>",
            "chat dm send <email> --text <text>",
            "chat threads <space-id>",
            "search <query>",
            "search unread",
            "mark read"
        ]
    })
}
