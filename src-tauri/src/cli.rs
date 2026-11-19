//! Locates the AI command-line tools installed on this Mac.
//!
//! GrammarBar does not bundle any CLI. Apps launched from Finder get a minimal
//! PATH, so the user's login-shell PATH is combined with common install folders.
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::process::Command;

const MARKER: &str = "__GRAMMARBAR_PATH__";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Provider {
    Codex,
    Claude,
}

impl Provider {
    pub const ALL: [Provider; 2] = [Provider::Claude, Provider::Codex];
    pub fn parse(id: &str) -> Result<Self, String> {
        match id {
            "codex" => Ok(Self::Codex),
            "claude" => Ok(Self::Claude),
            _ => Err("Unknown provider.".into()),
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
        }
    }
    pub fn binary(self) -> &'static str {
        self.id()
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Codex => "ChatGPT · Codex CLI",
            Self::Claude => "Claude · Claude Code",
        }
    }
    pub fn install_url(self) -> &'static str {
        match self {
            Self::Codex => "https://developers.openai.com/codex/cli",
            Self::Claude => "https://code.claude.com/docs/en/setup",
        }
    }
}

#[derive(Clone, Default)]
pub struct Tools {
    /// PATH passed to every child process, so Node-based installs find `node`.
    pub path_env: OsString,
    pub codex: Option<PathBuf>,
    pub claude: Option<PathBuf>,
}

impl Tools {
    pub fn get(&self, provider: Provider) -> Option<&Path> {
        match provider {
            Provider::Codex => self.codex.as_deref(),
            Provider::Claude => self.claude.as_deref(),
        }
    }
    pub fn require(&self, provider: Provider) -> Result<&Path, String> {
        self.get(provider).ok_or_else(|| {
            format!(
                "{} is not installed. Install it from {}",
                match provider {
                    Provider::Codex => "Codex CLI",
                    Provider::Claude => "Claude Code",
                },
                provider.install_url()
            )
        })
    }
    pub fn command(&self, binary: &Path) -> Command {
        let mut command = Command::new(binary);
        command
            .env("PATH", &self.path_env)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        command
    }
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

fn fallback_dirs() -> Vec<PathBuf> {
    let home = home();
    let mut dirs: Vec<PathBuf> = [
        ".local/bin",
        ".claude/local",
        ".codex/bin",
        ".bun/bin",
        ".volta/bin",
        ".npm-global/bin",
        ".cargo/bin",
    ]
    .iter()
    .map(|dir| home.join(dir))
    .collect();
    dirs.extend(
        ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"]
            .iter()
            .map(PathBuf::from),
    );
    dirs
}

/// Reads PATH from the user's interactive login shell (covers nvm, fnm, asdf…).
async fn shell_path() -> Option<String> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let output = tokio::time::timeout(
        Duration::from_secs(5),
        Command::new(shell)
            .args(["-ilc", &format!("printf '{MARKER}%s{MARKER}' \"$PATH\"")])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    let start = text.find(MARKER)? + MARKER.len();
    let end = start + text[start..].find(MARKER)?;
    Some(text[start..end].to_owned())
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

pub fn find_in(dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    dirs.iter()
        .map(|dir| dir.join(name))
        .find(|path| is_executable(path))
}

pub async fn detect() -> Tools {
    let mut dirs: Vec<PathBuf> = shell_path()
        .await
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    for dir in fallback_dirs() {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs.retain(|dir| dir.is_absolute());
    Tools {
        path_env: std::env::join_paths(&dirs).unwrap_or_default(),
        codex: find_in(&dirs, Provider::Codex.binary()),
        claude: find_in(&dirs, Provider::Claude.binary()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn providers_round_trip() {
        for provider in Provider::ALL {
            assert_eq!(Provider::parse(provider.id()).unwrap(), provider);
        }
        assert!(Provider::parse("gemini").is_err());
    }

    #[test]
    fn finds_only_executables_in_order() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("grammarbar-cli-{}", std::process::id()));
        let (first, second) = (root.join("a"), root.join("b"));
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        std::fs::write(first.join("claude"), "").unwrap();
        std::fs::write(second.join("claude"), "").unwrap();
        std::fs::set_permissions(
            second.join("claude"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let dirs = [first.clone(), second.clone()];
        assert_eq!(find_in(&dirs, "claude"), Some(second.join("claude")));
        assert_eq!(find_in(&dirs, "codex"), None);
        std::fs::remove_dir_all(root).unwrap();
    }
}
