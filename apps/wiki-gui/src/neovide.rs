//! External editor integration: find or start a Neovim server, attach Neovide to it,
//! open the active Wiki Content with `:WikiOpen`, and focus the Neovide window through
//! niri when niri is available.
//!
//! This deliberately keeps the editor out of the GUI process. The GUI only talks to the
//! server through `nvim --server … --remote-send` and to the compositor through
//! `niri msg`. Every failure is returned as text so the toolbar and the inspector can
//! show it instead of silently doing nothing.

use std::env;
use std::fs;
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

const DEFAULT_SOCKET_NAME: &str = "harmion-wiki-nvim.sock";
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(4);

#[derive(Clone, Debug)]
pub struct OpenOutcome {
    pub message: String,
    pub warning: Option<String>,
}

#[derive(Debug)]
pub struct Manager {
    socket: PathBuf,
    launched: bool,
}

impl Manager {
    /// Find an existing Neovim server, or choose the path a new one will listen on.
    pub fn detect() -> Self {
        let socket = env::var_os("WIKI_NVIM_SERVER")
            .map(PathBuf::from)
            .or_else(|| {
                env::var_os("NVIM_LISTEN_ADDRESS")
                    .map(PathBuf::from)
                    .filter(|path| is_socket(path))
            })
            .or_else(find_existing_socket)
            .unwrap_or_else(default_socket);
        Self {
            socket,
            launched: false,
        }
    }

    pub fn has_server(&self) -> bool {
        is_socket(&self.socket)
    }

    /// Open `id` in the editor, starting the editor when it is not running yet.
    pub fn open(&mut self, id: &str, title: &str) -> Result<OpenOutcome, String> {
        self.ensure_server()?;
        self.ensure_window()?;

        let mut remote = Command::new("nvim");
        remote.args([
            "--server",
            &self.socket.display().to_string(),
            "--remote-send",
            &format!(":WikiOpen {id}<CR>"),
        ]);
        sanitize_editor_env(&mut remote, &self.socket);
        let output = remote
            .output()
            .map_err(|error| format!("nvim --server failed: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "nvim --server exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        let remote_stderr = String::from_utf8_lossy(&output.stderr);
        if let Some(error) = editor_error(&remote_stderr) {
            return Err(format!(
                "Neovim refused the buffer ({error}): {}",
                remote_stderr.trim()
            ));
        }

        let warning = self.focus_window(title).err();
        Ok(OpenOutcome {
            message: format!("Neovide · {title}"),
            warning,
        })
    }

    /// Focus the Neovide window that belongs to this Wiki session.
    pub fn focus_window(&self, title: &str) -> Result<(), String> {
        let windows = niri_windows()?;
        let wanted = windows.iter().find_map(|window| {
            let app = window
                .get("app_id")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            let window_title = window
                .get("title")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            let is_editor = app.contains("neovide") || app.contains("neovim");
            let mentions_page = !title.is_empty() && window_title.contains(title);
            (is_editor || mentions_page)
                .then(|| window.get("id").and_then(|value| value.as_i64()))
                .flatten()
        });
        let Some(id) = wanted else {
            return Err("niri: Neovide のウィンドウが見つかりません".to_owned());
        };
        let output = Command::new("niri")
            .args(["msg", "action", "focus-window", "--id", &id.to_string()])
            .output()
            .map_err(|error| format!("niri msg: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "niri focus-window failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(())
    }

    fn ensure_server(&mut self) -> Result<(), String> {
        if self.has_server() {
            return Ok(());
        }
        self.launch()?;
        let deadline = std::time::Instant::now() + LAUNCH_TIMEOUT;
        while std::time::Instant::now() < deadline {
            if self.has_server() {
                self.launched = true;
                return Ok(());
            }
            thread::sleep(Duration::from_millis(50));
        }
        Err(format!(
            "エディタのサーバーが {} に現れません（Neovide の起動に失敗した可能性）",
            self.socket.display()
        ))
    }

    /// Attach a Neovide window to a server that is already listening but has no window.
    fn ensure_window(&self) -> Result<(), String> {
        if let Ok(windows) = niri_windows()
            && windows.iter().any(|window| {
                window
                    .get("app_id")
                    .and_then(|value| value.as_str())
                    .is_some_and(|app| app.to_ascii_lowercase().contains("neovide"))
            })
        {
            return Ok(());
        }
        // No window yet: attach one to the server. `--fork` keeps the GUI responsive.
        let mut command = Command::new("neovide");
        command.args(["--fork", "--server", &self.socket.display().to_string()]);
        command.stdout(Stdio::null()).stderr(Stdio::null());
        sanitize_editor_env(&mut command, &self.socket);
        match command.spawn() {
            Ok(_child) => {
                let deadline = std::time::Instant::now() + LAUNCH_TIMEOUT;
                while std::time::Instant::now() < deadline {
                    if self.focus_window("").is_ok() {
                        return Ok(());
                    }
                    thread::sleep(Duration::from_millis(100));
                }
                Ok(())
            }
            Err(error) => Err(format!(
                "Neovide を起動できません（{}）。`neovide` を PATH に置くか、既存の \
                 Neovim サーバーへ接続してください",
                error
            )),
        }
    }

    fn launch(&mut self) -> Result<(), String> {
        if let Some(parent) = self.socket.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::remove_file(&self.socket);
        let mut args: Vec<String> = vec!["--fork".to_owned(), "--".to_owned()];
        args.push("--listen".to_owned());
        args.push(self.socket.display().to_string());
        args.push("--cmd".to_owned());
        args.push("set termguicolors background=dark".to_owned());
        if let Some(plugin) = plugin_path() {
            args.push("--cmd".to_owned());
            args.push(format!("set rtp+={}", plugin.display()));
        }
        let mut command = Command::new("neovide");
        command.args(&args);
        command.stdout(Stdio::null()).stderr(Stdio::null());
        sanitize_editor_env(&mut command, &self.socket);
        match command.spawn() {
            Ok(_child) => {
                self.launched = true;
                Ok(())
            }
            Err(error) => {
                // No Neovide: fall back to a terminal Neovim with the same server socket.
                self.launch_terminal()
                    .map_err(|terminal| {
                        format!(
                            "Neovide を起動できません（{error}）。ターミナルの Neovim も起動できません（{terminal}）"
                        )
                    })
            }
        }
    }

    fn launch_terminal(&mut self) -> Result<(), String> {
        let plugin = plugin_path();
        let mut nvim_args = vec![
            "--listen".to_owned(),
            self.socket.display().to_string(),
            "--cmd".to_owned(),
            "set termguicolors background=dark".to_owned(),
        ];
        if let Some(plugin) = plugin {
            nvim_args.push("--cmd".to_owned());
            nvim_args.push(format!("set rtp+={}", plugin.display()));
        }
        let terminals: [(&str, &[&str]); 4] = [
            ("alacritty", &["-e", "nvim"]),
            ("foot", &["nvim"]),
            ("kitty", &["nvim"]),
            ("xterm", &["-e", "nvim"]),
        ];
        for (program, prefix) in terminals {
            let mut args = prefix
                .iter()
                .map(|arg| (*arg).to_owned())
                .collect::<Vec<_>>();
            args.extend(nvim_args.iter().cloned());
            let mut command = Command::new(program);
            command.args(&args);
            command.stdout(Stdio::null()).stderr(Stdio::null());
            sanitize_editor_env(&mut command, &self.socket);
            match command.spawn() {
                Ok(_child) => {
                    self.launched = true;
                    return Ok(());
                }
                Err(_) => continue,
            }
        }
        Err("neovide / alacritty / foot / kitty / xterm のいずれも見つかりません".to_owned())
    }

    /// A short label for status bars.
    pub fn status_label(&self) -> String {
        if self.has_server() {
            if self.launched {
                "Neovide · 起動済み".to_owned()
            } else {
                "Neovide · 接続済み".to_owned()
            }
        } else {
            "Neovide · 未起動".to_owned()
        }
    }
}

fn sanitize_editor_env(command: &mut Command, editor_socket: &Path) {
    for key in [
        "XDG_CACHE_HOME",
        "XDG_STATE_HOME",
        "XDG_DATA_HOME",
        "XDG_CONFIG_HOME",
    ] {
        if let Some(value) = env::var_os(key) {
            let value = value.to_string_lossy();
            if value.contains("/target/nix-") || value.contains("/target/cargo-home") {
                command.env_remove(key);
            }
        }
    }
    if let Ok(value) = env::var("WIKI_SOCKET") {
        let path = PathBuf::from(value);
        if path.is_relative() {
            if let Ok(cwd) = env::current_dir() {
                command.env("WIKI_SOCKET", cwd.join(path));
            }
        } else {
            command.env("WIKI_SOCKET", path);
        }
    }
    command.env("WIKI_NVIM_SERVER", editor_socket);
    if let Some(lsp) = lsp_path() {
        command.env("WIKI_LSP_CMD", lsp);
    }
}

fn lsp_path() -> Option<PathBuf> {
    if let Some(value) = env::var_os("WIKI_LSP_CMD") {
        return Some(PathBuf::from(value));
    }
    let sibling = env::current_exe().ok()?.parent()?.join("wiki-lsp");
    sibling.exists().then_some(sibling)
}

fn editor_error(stderr: &str) -> Option<String> {
    stderr
        .split_whitespace()
        .find(|word| {
            let mut chars = word.chars();
            chars.next() == Some('E') && chars.next().is_some_and(|digit| digit.is_ascii_digit())
        })
        .map(str::to_owned)
}

fn plugin_path() -> Option<PathBuf> {
    if let Some(path) = env::var_os("WIKI_NVIM_RTP") {
        let path = PathBuf::from(path);
        if path.exists() {
            return Some(path);
        }
    }
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../wiki/nvim");
    path.canonicalize().ok().filter(|path| path.exists())
}

fn default_socket() -> PathBuf {
    env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(DEFAULT_SOCKET_NAME)
}

fn is_socket(path: &Path) -> bool {
    fs::metadata(path)
        .map(|metadata| metadata.file_type().is_socket())
        .unwrap_or(false)
}

fn find_existing_socket() -> Option<PathBuf> {
    let uid = nix_uid();
    let mut candidates = Vec::new();
    if let Some(runtime) = env::var_os("XDG_RUNTIME_DIR") {
        let runtime = PathBuf::from(runtime);
        candidates.push(runtime.join("nvim.sock"));
        if let Ok(entries) = fs::read_dir(&runtime) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with("nvim.") && name.ends_with(".0") {
                    candidates.push(entry.path());
                }
            }
        }
    }
    candidates.push(PathBuf::from(format!("/tmp/nvim.{uid}.0")));
    candidates.push(PathBuf::from("/tmp/nvim.sock"));
    candidates.push(PathBuf::from("/tmp/harmion-wiki-nvim.sock"));
    if let Ok(dir) = env::current_dir() {
        candidates.push(dir.join("target/nvim.sock"));
    }
    for path in candidates {
        if is_socket(&path) {
            return Some(path);
        }
    }
    // Neovim names the default server after the uid inside the runtime directory.
    if let Some(runtime) = env::var_os("XDG_RUNTIME_DIR") {
        let entries = fs::read_dir(PathBuf::from(runtime).join("nvim")).ok();
        if let Some(entries) = entries {
            for entry in entries.flatten() {
                if is_socket(&entry.path()) {
                    return Some(entry.path());
                }
            }
        }
    }
    None
}

fn nix_uid() -> u32 {
    // Reading /proc avoids adding a libc dependency just for getuid.
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find_map(|line| line.strip_prefix("Uid:"))
                .and_then(|rest| rest.split_whitespace().next())
                .and_then(|uid| uid.parse().ok())
        })
        .unwrap_or(1000)
}

fn niri_windows() -> Result<Vec<serde_json::Value>, String> {
    let output = Command::new("niri")
        .args(["msg", "--json", "windows"])
        .output()
        .map_err(|error| format!("niri msg: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "niri msg windows failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("niri msg windows returned invalid JSON: {error}"))?;
    value
        .as_array()
        .cloned()
        .ok_or_else(|| "niri msg windows did not return an array".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "opens a real Neovide window"]
    fn launches_and_opens_content() {
        let mut manager = Manager::detect();
        let outcome = manager.open("p-home", "家計の記録").expect("editor launch");
        assert!(outcome.message.contains("Neovide"));
        assert!(manager.has_server(), "editor socket should exist");

        let output = Command::new("timeout")
            .args([
                "5s",
                "nvim",
                "--server",
                &manager.socket.display().to_string(),
                "--remote-expr",
                "stdpath('cache')",
            ])
            .output()
            .expect("query editor cache path");
        let cache = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        eprintln!("editor cache: {cache}");
        assert!(
            !cache.contains("/target/nix-cache"),
            "editor inherited the sandbox cache: {cache}"
        );
        assert!(cache.ends_with("/nvim"), "unexpected cache path: {cache}");
    }
}
