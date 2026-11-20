use crate::cli::{Cli, HomeCommand};
use crate::config::ConfigStore;
use crate::error::AppError;
use crate::output::SuccessEnvelope;
use serde_json::json;

const COMMAND: &str = "home.unread";
const ORIGIN: &str = "chrome-extension://ancpoagagdlcombppddgmdkgfilnfijc/";

pub fn is_chrome_host(argument: &str) -> bool {
    argument.trim_end_matches('/') == ORIGIN.trim_end_matches('/')
}

pub fn run(
    store: &ConfigStore,
    cli: &Cli,
    command: &HomeCommand,
) -> Result<SuccessEnvelope, AppError> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        match command {
            HomeCommand::Unread => platform::unread(store, cli),
            HomeCommand::Setup => platform::setup(),
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (store, cli, command);
        Err(unsupported())
    }
}

pub fn serve() -> Result<(), AppError> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        platform::serve().map_err(|error| platform::io_error("home.bridge", error))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Err(unsupported())
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn unsupported() -> AppError {
    AppError::usage(
        COMMAND,
        "Google Chat Home requires Chrome on macOS or Linux",
        json!({}),
    )
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod platform {
    use super::*;
    use crate::config::normalize_email;
    use crate::output::success;
    use serde_json::Value;
    use std::fs::{self, DirBuilder, File};
    use std::io::{self, Read, Write};
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::time::Duration;

    const HOST: &str = "com.gchat.home";
    const FRAME_LIMIT: usize = 4 * 1024 * 1024;
    const BROWSER_TIMEOUT: Duration = Duration::from_secs(30);
    const CLIENT_TIMEOUT: Duration = Duration::from_secs(35);

    pub fn setup() -> Result<SuccessEnvelope, AppError> {
        let command = "home.setup";
        let home = home_dir().map_err(|e| io_error(command, e))?;
        #[cfg(target_os = "macos")]
        let path = home.join("Library/Application Support/Google/Chrome/NativeMessagingHosts");
        #[cfg(target_os = "linux")]
        let path = std::env::var_os("CHROME_CONFIG_HOME")
            .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"))
            .join("google-chrome/NativeMessagingHosts");

        fs::create_dir_all(&path).map_err(|e| io_error(command, e))?;
        let manifest_path = path.join(format!("{HOST}.json"));
        let executable = std::env::current_exe()
            .and_then(fs::canonicalize)
            .map_err(|e| io_error(command, e))?;
        let manifest = json!({
            "name": HOST,
            "description": "Google Chat Home for gchat",
            "path": executable,
            "type": "stdio",
            "allowed_origins": [ORIGIN],
        });
        let pending = path.join(format!(".{HOST}.{}.tmp", std::process::id()));
        fs::write(&pending, serde_json::to_vec_pretty(&manifest)?)
            .and_then(|()| fs::rename(&pending, &manifest_path))
            .map_err(|e| io_error(command, e))?;
        Ok(success(
            command,
            None,
            json!({"manifest": manifest_path}),
            json!({}),
        ))
    }

    pub fn unread(store: &ConfigStore, cli: &Cli) -> Result<SuccessEnvelope, AppError> {
        if cli.page_token.is_some() {
            return Err(AppError::usage(
                COMMAND,
                "Home does not use page tokens",
                json!({}),
            ));
        }
        let expected = cli
            .account
            .as_deref()
            .map(normalize_email)
            .or(store.load_config()?.default_account);

        let mut stream = private_socket_dir().and_then(|dir| UnixStream::connect(dir.join("home.sock")))
            .map_err(|e| AppError::local_io(
                COMMAND,
                "Chrome is not connected to gchat Home",
                json!({
                    "ioError": e.to_string(),
                    "fix": "Set up the gchat Home extension, then open Chrome or click its Reconnect gchat Home button."
                }),
            ))?;
        stream
            .set_read_timeout(Some(CLIENT_TIMEOUT))
            .and_then(|()| stream.set_write_timeout(Some(CLIENT_TIMEOUT)))
            .and_then(|()| write_frame(&mut stream, &json!({"account": expected})))
            .map_err(|e| io_error(COMMAND, e))?;
        let result = read_frame(&mut stream).map_err(|e| io_error(COMMAND, e))?;
        if let Some(error) = result.get("error").and_then(Value::as_str) {
            return Err(AppError::local_io(COMMAND, error, json!({})));
        }

        let account = result["account"].as_str().ok_or_else(invalid_response)?;
        if expected.as_deref().is_some_and(|email| email != account) {
            return Err(invalid_response());
        }
        let items = result["items"].as_array().ok_or_else(invalid_response)?;
        let count = cli.max.unwrap_or(items.len()).min(items.len());
        Ok(success(
            COMMAND,
            Some(account.to_string()),
            json!({"items": &items[..count]}),
            json!({"count": count, "truncated": count < items.len(), "source": "google_chat_home"}),
        ))
    }

    fn invalid_response() -> AppError {
        AppError::local_io(
            COMMAND,
            "Chrome returned an invalid Home response",
            json!({}),
        )
    }

    pub fn serve() -> io::Result<()> {
        let dir = socket_dir()?;
        let _ = DirBuilder::new().mode(0o700).create(&dir);
        let dir = private_socket_dir()?;
        let lock = File::options()
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join("lock"))?;
        lock.try_lock().map_err(io::Error::other)?;
        let path = dir.join("home.sock");
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let listener = UnixListener::bind(&path)?;
        let result = relay(listener, path.clone());
        let _ = fs::remove_file(&path);
        result
    }

    fn relay(listener: UnixListener, socket: PathBuf) -> io::Result<()> {
        let (sender, receiver) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let mut input = io::stdin().lock();
            while let Ok(message) = read_frame(&mut input) {
                if sender.send(message).is_err() {
                    return;
                }
            }
            // Chrome closed the native port while the main thread may still be in accept.
            let _ = fs::remove_file(socket);
            std::process::exit(0);
        });

        let mut output = io::stdout().lock();
        for incoming in listener.incoming() {
            let mut stream = incoming?;
            stream.set_read_timeout(Some(Duration::from_secs(2)))?;
            stream.set_write_timeout(Some(Duration::from_secs(2)))?;
            let Ok(request) = read_frame(&mut stream) else {
                continue;
            };
            write_frame(&mut output, &request)?;
            let response = receiver
                .recv_timeout(BROWSER_TIMEOUT)
                .map_err(io::Error::other)?;
            let _ = write_frame(&mut stream, &response);
        }
        Ok(())
    }

    fn read_frame(reader: &mut impl Read) -> io::Result<Value> {
        let mut header = [0; 4];
        reader.read_exact(&mut header)?;
        let size = u32::from_ne_bytes(header) as usize;
        if size == 0 || size > FRAME_LIMIT {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid Home message length",
            ));
        }
        let mut buffer = vec![0; size];
        reader.read_exact(&mut buffer)?;
        serde_json::from_slice(&buffer).map_err(Into::into)
    }

    fn write_frame(writer: &mut impl Write, value: &Value) -> io::Result<()> {
        let bytes = serde_json::to_vec(value)?;
        if bytes.len() > FRAME_LIMIT {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Home message is too large",
            ));
        }
        writer.write_all(&(bytes.len() as u32).to_ne_bytes())?;
        writer.write_all(&bytes)?;
        writer.flush()
    }

    fn private_socket_dir() -> io::Result<PathBuf> {
        let dir = socket_dir()?;
        let metadata = fs::symlink_metadata(&dir)?;
        if !metadata.is_dir() || metadata.permissions().mode() & 0o077 != 0 {
            return Err(io::Error::other(
                "gchat Home socket directory must be private",
            ));
        }
        Ok(dir)
    }

    fn socket_dir() -> io::Result<PathBuf> {
        Ok(home_dir()?.join(".gchat-home"))
    }

    fn home_dir() -> io::Result<PathBuf> {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))
    }

    pub fn io_error(command: &str, error: io::Error) -> AppError {
        AppError::local_io(
            command,
            "Google Chat Home connection failed",
            json!({"ioError": error.to_string()}),
        )
    }
}
