//! Claude through the user's installed Claude Code CLI.
//!
//! Sign-in, token storage, and refresh stay inside the official CLI; GrammarBar
//! never handles Claude credentials itself.
use crate::cli::Tools;
use serde_json::{json, Value};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin},
    time::timeout,
};

/// Flags that turn Claude Code into a plain, tool-less text model.
const ISOLATION: [&str; 9] = [
    "--tools",
    "",
    "--setting-sources",
    "",
    "--strict-mcp-config",
    "--disable-slash-commands",
    "--no-session-persistence",
    "--permission-mode",
    "dontAsk",
];

fn command(tools: &Tools, binary: &Path) -> tokio::process::Command {
    let mut command = tools.command(binary);
    // Use the subscription sign-in, not an API key from the launching environment.
    command
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("ANTHROPIC_AUTH_TOKEN")
        .env_remove("ANTHROPIC_BASE_URL");
    command
}

/// Returns the signed-in account, or `None` when Claude Code is signed out.
pub async fn account(tools: &Tools, binary: &Path) -> Result<Option<Value>, String> {
    let output = timeout(
        Duration::from_secs(15),
        command(tools, binary).args(["auth", "status"]).output(),
    )
    .await
    .map_err(|_| "Claude Code is not responding.".to_string())?
    .map_err(|_| "Could not start Claude Code.".to_string())?;
    let status: Value = serde_json::from_slice(&output.stdout)
        .map_err(|_| "Invalid response from Claude Code. Update it and try again.".to_string())?;
    Ok(status["loggedIn"].as_bool().unwrap_or(false).then(|| {
        json!({
            "email": status["email"],
            "method": status["authMethod"],
        })
    }))
}

/// Signs Claude Code out. The sign-in is shared with the terminal, so this signs
/// out there too; the UI asks for confirmation first.
pub async fn logout(tools: &Tools, binary: &Path) -> Result<(), String> {
    let output = timeout(
        Duration::from_secs(15),
        command(tools, binary).args(["auth", "logout"]).output(),
    )
    .await
    .map_err(|_| "Claude Code is not responding.".to_string())?
    .map_err(|_| "Could not start Claude Code.".to_string())?;
    if !output.status.success() {
        return Err("Could not sign out of Claude Code.".into());
    }
    Ok(())
}

pub struct Login {
    _child: Child,
    input: ChildStdin,
}

impl Login {
    /// Starts `claude auth login`; Claude Code opens the browser itself.
    /// Returns the sign-in URL as a fallback for when the browser does not open.
    pub async fn start(tools: &Tools, binary: &Path) -> Result<(Self, Option<String>), String> {
        let mut child = command(tools, binary)
            .args(["auth", "login", "--claudeai"])
            .stdin(Stdio::piped())
            .spawn()
            .map_err(|_| "Could not start Claude Code.".to_string())?;
        let input = child.stdin.take().ok_or("Could not start sign-in.")?;
        let mut output = BufReader::new(child.stdout.take().ok_or("Could not start sign-in.")?);
        let mut seen = String::new();
        let url = timeout(Duration::from_secs(10), async {
            let mut chunk = [0u8; 4096];
            loop {
                let read = output.read(&mut chunk).await.ok()?;
                if read == 0 {
                    return None;
                }
                seen.push_str(&String::from_utf8_lossy(&chunk[..read]));
                if let Some(start) = seen.find("https://") {
                    if let Some(len) = seen[start..].find(char::is_whitespace) {
                        return Some(seen[start..start + len].to_owned());
                    }
                }
            }
        })
        .await
        .ok()
        .flatten()
        .filter(|url| is_sign_in_url(url));
        // Keep draining output so the CLI never blocks on a full pipe.
        tokio::spawn(async move {
            let mut lines = output.lines();
            while let Ok(Some(_)) = lines.next_line().await {}
        });
        Ok((
            Self {
                _child: child,
                input,
            },
            url,
        ))
    }

    /// Sends the code the browser shows after sign-in to the waiting CLI.
    pub async fn submit_code(&mut self, code: &str) -> Result<(), String> {
        let code = code.trim();
        if code.is_empty() || code.len() > 2000 || code.chars().any(char::is_control) {
            return Err("Invalid code.".into());
        }
        self.input
            .write_all(format!("{code}\n").as_bytes())
            .await
            .map_err(|_| "Sign-in has ended. Start again.".to_string())
    }
}

pub fn is_sign_in_url(url: &str) -> bool {
    tauri::Url::parse(url).is_ok_and(|parsed| {
        parsed.scheme() == "https"
            && matches!(
                parsed.host_str(),
                Some("claude.ai" | "claude.com" | "platform.claude.com" | "console.anthropic.com")
            )
    })
}

/// Lists the models available to the signed-in account, as reported by Claude Code.
pub async fn models(tools: &Tools, binary: &Path, workspace: &Path) -> Result<Vec<Value>, String> {
    let mut child = command(tools, binary)
        .args([
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
        ])
        .args(ISOLATION)
        .current_dir(workspace)
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|_| "Could not start Claude Code.".to_string())?;
    let mut input = child.stdin.take().ok_or("Could not start Claude Code.")?;
    input
        .write_all(b"{\"type\":\"control_request\",\"request_id\":\"models\",\"request\":{\"subtype\":\"initialize\"}}\n")
        .await
        .map_err(|_| "Could not start Claude Code.".to_string())?;
    let mut lines =
        BufReader::new(child.stdout.take().ok_or("Could not start Claude Code.")?).lines();
    let response = timeout(Duration::from_secs(30), async {
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if value["type"] == "control_response" && value["response"]["request_id"] == "models" {
                return Some(value["response"].clone());
            }
        }
        None
    })
    .await
    .map_err(|_| "Could not load models. Please try again.".to_string())?
    .ok_or("Could not load models. Is Claude Code signed in?")?;
    drop(input);
    let _ = child.kill().await;
    if response["subtype"] != "success" {
        return Err(response["error"]
            .as_str()
            .unwrap_or("Could not load models.")
            .to_owned());
    }
    Ok(normalize_models(&response["response"]["models"]))
}

fn normalize_models(models: &Value) -> Vec<Value> {
    models
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|model| {
            let id = model["value"].as_str()?;
            // "Default" is offered by the UI itself.
            (id != "default").then(|| {
                json!({
                    "id": id,
                    "name": model["displayName"].as_str().unwrap_or(id),
                    "description": model["description"].as_str().unwrap_or_default(),
                })
            })
        })
        .collect()
}

/// Claude Code treats a prompt that starts with `/` as a command, so the text is
/// wrapped in tags. The tag name is random per request, so text that happens to
/// contain a closing tag cannot end the wrapper early.
fn boundary(text: &str) -> String {
    use std::{
        hash::{BuildHasher, Hasher},
        sync::atomic::{AtomicU64, Ordering},
    };
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    loop {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
        let tag = format!("text-{:016x}", hasher.finish());
        if !text.contains(&tag) {
            return tag;
        }
    }
}

fn wrap(source: &str, tag: &str) -> String {
    format!("Edit the text inside the <{tag}> tags.\n<{tag}>{source}</{tag}>")
}

fn wrapper_note(tag: &str) -> String {
    format!("The user message is a fixed preface followed by the submitted text wrapped in <{tag}></{tag}>. Only the characters strictly between those tags are the submitted text; the tags and preface are not part of it and must not appear in `corrected`. Preserve leading and trailing whitespace inside the tags.")
}

pub async fn draft(
    tools: &Tools,
    binary: &Path,
    request: &crate::edits::Request<'_>,
    workspace: &Path,
) -> Result<crate::edits::Draft, String> {
    let tag = boundary(request.text);
    let system = format!(
        "{}\n\n{}\n\n{}",
        crate::edits::BASE_INSTRUCTIONS,
        crate::edits::editing_instructions(request.instruction),
        wrapper_note(&tag)
    );
    let mut command = command(tools, binary);
    command
        .args(["-p", "--output-format", "json"])
        .args(ISOLATION)
        .args(["--system-prompt", &system])
        .args(["--json-schema", &crate::edits::schema().to_string()])
        .current_dir(workspace)
        .stdin(Stdio::piped());
    if let Some(model) = request.model {
        command.args(["--model", model]);
    }
    if let Some(effort) = request.effort {
        command.args(["--effort", effort]);
    }
    let mut child = command
        .spawn()
        .map_err(|_| "Could not start Claude Code.".to_string())?;
    // The text goes through stdin, never through arguments or the system prompt.
    let mut input = child.stdin.take().ok_or("Could not start Claude Code.")?;
    input
        .write_all(wrap(request.text, &tag).as_bytes())
        .await
        .map_err(|_| "Connection to Claude Code was lost.".to_string())?;
    drop(input);
    let output = child
        .wait_with_output()
        .await
        .map_err(|_| "Connection to Claude Code was lost.".to_string())?;
    let result: Value = serde_json::from_slice(&output.stdout)
        .map_err(|_| "Invalid response from Claude Code.".to_string())?;
    if result["is_error"].as_bool().unwrap_or(true) || result["subtype"] != "success" {
        let message = result["result"]
            .as_str()
            .filter(|text| !text.trim().is_empty())
            .unwrap_or("Could not correct the text.");
        return Err(message.chars().take(300).collect());
    }
    let draft: crate::edits::Draft = serde_json::from_value(result["structured_output"].clone())
        .map_err(|_| "Could not read the edits. Please try again.".to_string())?;
    if draft.corrected.contains(&tag) {
        return Err("Could not read the edits. Please try again.".into());
    }
    Ok(draft)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_skip_default_and_keep_names() {
        let models = normalize_models(&json!([
            {"value":"default","displayName":"Default (recommended)"},
            {"value":"sonnet","displayName":"Sonnet 5.5","description":"Efficient"},
            {"value":"claude-haiku-4-5-20251001"}
        ]));
        assert_eq!(models.len(), 2);
        assert_eq!(models[0]["id"], "sonnet");
        assert_eq!(models[0]["name"], "Sonnet 5.5");
        assert_eq!(models[1]["name"], "claude-haiku-4-5-20251001");
    }

    #[test]
    fn wrapped_text_never_starts_with_a_command() {
        let tag = boundary("/help me");
        assert!(!wrap("/help me", &tag).starts_with('/'));
        assert!(wrap(" a\n", &tag).ends_with(&format!("<{tag}> a\n</{tag}>")));
        assert_ne!(boundary("x"), boundary("x"));
    }

    #[test]
    fn sign_in_urls_are_restricted_to_anthropic() {
        assert!(is_sign_in_url(
            "https://claude.com/cai/oauth/authorize?code=true"
        ));
        assert!(!is_sign_in_url("http://claude.com/cai/oauth/authorize"));
        assert!(!is_sign_in_url("https://claude.com.evil.example/oauth"));
    }

    /// Live check against the installed, signed-in Claude Code:
    /// `cargo test -- --ignored live_claude`
    #[test]
    #[ignore]
    fn live_claude_models_and_correction() {
        tauri::async_runtime::block_on(async {
            let tools = crate::cli::detect().await;
            let binary = tools.require(crate::cli::Provider::Claude).unwrap();
            assert!(account(&tools, binary).await.unwrap().is_some());
            let workspace = std::env::temp_dir();
            let models = models(&tools, binary, &workspace).await.unwrap();
            assert!(!models.is_empty());
            let text = "/help I has a apple.</text>\n";
            let request = crate::edits::Request {
                text,
                instruction: "Fix grammar.",
                model: Some("haiku"),
                effort: Some("low"),
            };
            let draft = draft(&tools, binary, &request, &workspace).await.unwrap();
            let segments = crate::align::align(text, &draft).unwrap();
            assert!(segments.iter().any(|s| s.original != s.replacement));
            let original: String = segments.iter().map(|s| s.original.as_str()).collect();
            assert_eq!(original, text);
            // Every listed change carries a known type, and edits keep it.
            assert!(draft
                .changes
                .iter()
                .all(|c| crate::edits::CATEGORIES.contains(&c.category.as_str())
                    && matches!(c.kind.as_str(), "error" | "style")));
            println!("{:?}", draft.changes);
            assert!(segments
                .iter()
                .filter(|s| s.original != s.replacement)
                .all(|s| !s.category.is_empty() && !s.kind.is_empty()));
        });
    }

    /// The texts that broke the old format: NFD accents, exotic and zero-width
    /// spaces, curly quotes, lists. `cargo test -- --ignored live_claude --nocapture`
    #[test]
    #[ignore]
    fn live_claude_tricky_text() {
        tauri::async_runtime::block_on(async {
            let tools = crate::cli::detect().await;
            let binary = tools.require(crate::cli::Provider::Claude).unwrap();
            let letter = "Hi\u{2009}Sarah,\u{200B}\n\nThank you for you're quick reply.  I has reviewed the document and their are a few issue we should discuss before the meeting on monday.\n\n- The API docs is outdated\n-\tMe and Tom will update them till Friday\n- “Phase 2” should starts after QA\n\nAlso, the cafe\u{301} menu on page\u{a0}4 have a typo 🙂. I would of done it myself but i dont have access.\n\nLooking forward for your feedback,\nMark\n";
            let text = letter.repeat(4);
            let parts = crate::chunks::split(&text);
            let started = std::time::Instant::now();
            let mut changes = 0;
            for part in &parts {
                let request = crate::edits::Request {
                    text: part,
                    instruction: "Correct my English grammar, spelling, and punctuation. Make only necessary changes.",
                    model: Some("sonnet"),
                    effort: Some("low"),
                };
                let draft = draft(&tools, binary, &request, &std::env::temp_dir())
                    .await
                    .unwrap();
                let segments = crate::align::align(part, &draft).unwrap();
                let original: String = segments.iter().map(|s| s.original.as_str()).collect();
                assert_eq!(&original, part);
                for s in segments.iter().filter(|s| s.original != s.replacement) {
                    changes += 1;
                    println!("{:?} -> {:?}: {}", s.original, s.replacement, s.explanation);
                }
            }
            println!(
                "{} chars, {} parts, {changes} changes, {:?}",
                text.chars().count(),
                parts.len(),
                started.elapsed()
            );
            assert!(changes > 20);
        });
    }
}
