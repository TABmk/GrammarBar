//! ChatGPT through the user's installed Codex CLI (`codex app-server` over stdio).
use serde_json::{json, Value};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout, Command},
    time::timeout,
};

pub struct Server {
    _child: Child,
    input: ChildStdin,
    output: Lines<BufReader<ChildStdout>>,
    sequence: u64,
}

impl Server {
    pub async fn start(
        tools: &crate::cli::Tools,
        binary: &Path,
        home: &Path,
        workspace: &Path,
    ) -> Result<Self, String> {
        let mut child = Command::new(binary)
            .env("PATH", &tools.path_env)
            .arg("app-server")
            .args([
                "-c",
                "cli_auth_credentials_store=\"keyring\"",
                "-c",
                "forced_login_method=\"chatgpt\"",
                "-c",
                "web_search=\"disabled\"",
                "-c",
                "features.shell_tool=false",
                "-c",
                "features.unified_exec=false",
                "-c",
                "features.multi_agent=false",
                "-c",
                "project_doc_max_bytes=0",
            ])
            .env("CODEX_HOME", home)
            .env_remove("OPENAI_API_KEY")
            .env_remove("OPENAI_BASE_URL")
            .env_remove("CODEX_API_KEY")
            .current_dir(workspace)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| "Could not start Codex.".to_string())?;
        let mut server = Self {
            input: child.stdin.take().unwrap(),
            output: BufReader::new(child.stdout.take().unwrap()).lines(),
            _child: child,
            sequence: 0,
        };
        server
            .request(
                "initialize",
                json!({"clientInfo":{"name":"grammarbar","title":"GrammarBar","version":"0.1.0"}}),
            )
            .await?;
        server
            .send(json!({"method":"initialized","params":{}}))
            .await?;
        Ok(server)
    }
    async fn send(&mut self, value: Value) -> Result<(), String> {
        self.input
            .write_all(format!("{value}\n").as_bytes())
            .await
            .map_err(|_| "Connection to Codex was lost.".to_string())
    }
    pub async fn next(&mut self) -> Result<Value, String> {
        loop {
            let line = self
                .output
                .next_line()
                .await
                .map_err(|_| "Connection to Codex was lost.".to_string())?
                .ok_or("Codex stopped. Please try again.")?;
            let value: Value = serde_json::from_str(&line)
                .map_err(|_| "Invalid response from Codex.".to_string())?;
            if value.get("method").is_some() && value.get("id").is_some() {
                // This text-only client never grants tool, file, or command permissions.
                self.send(json!({"id":value["id"],"error":{"code":-32601,"message":"Tools are unavailable in GrammarBar"}})).await?;
                continue;
            }
            return Ok(value);
        }
    }
    pub async fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.sequence += 1;
        let id = self.sequence;
        self.send(json!({"id":id,"method":method,"params":params}))
            .await?;
        timeout(Duration::from_secs(25), async {
            loop {
                let msg = self.next().await?;
                if msg["id"] == id {
                    if let Some(error) = msg.get("error") {
                        return Err(error["message"]
                            .as_str()
                            .unwrap_or("Codex error.")
                            .to_string());
                    }
                    return Ok(msg["result"].clone());
                }
            }
        })
        .await
        .map_err(|_| "Codex is not responding. Please try again.".to_string())?
    }
    /// Lists text models in the shared `{id, name, description}` shape.
    pub async fn models(&mut self) -> Result<Vec<Value>, String> {
        Ok(normalize_models(self.raw_models().await?))
    }
    async fn raw_models(&mut self) -> Result<Vec<Value>, String> {
        let mut models = Vec::new();
        let mut cursor = Value::Null;
        loop {
            let page = self
                .request(
                    "model/list",
                    json!({"limit":100,"includeHidden":false,"cursor":cursor}),
                )
                .await?;
            models.extend(
                page["data"]
                    .as_array()
                    .ok_or("Invalid model list.")?
                    .iter()
                    .cloned(),
            );
            let next = page["nextCursor"].clone();
            if next.is_null() {
                return Ok(models);
            }
            if next == cursor {
                return Err("Invalid model list cursor.".into());
            }
            cursor = next;
        }
    }
    pub async fn ensure_account(&mut self) -> Result<(), String> {
        let account = self.request("account/read", json!({})).await?;
        if account["account"]["type"] != "chatgpt" {
            return Err("Connect ChatGPT in Settings.".into());
        }
        Ok(())
    }
    /// Corrects one part of the text in its own ephemeral thread.
    pub async fn draft(
        &mut self,
        request: &crate::edits::Request<'_>,
        workspace: &Path,
    ) -> Result<crate::edits::Draft, String> {
        let thread = self.request("thread/start", json!({
            "model":request.model,"cwd":workspace,"ephemeral":true,"sandbox":"read-only","approvalPolicy":"never",
            "baseInstructions":crate::edits::BASE_INSTRUCTIONS,
            "developerInstructions":crate::edits::editing_instructions(request.instruction)
        })).await?;
        let thread_id = thread["thread"]["id"]
            .as_str()
            .ok_or("Could not start the request.")?
            .to_owned();
        let mut turn = json!({"threadId":thread_id,"input":[{"type":"text","text":request.text}],"outputSchema":crate::edits::schema()});
        if let Some(effort) = request.effort {
            turn["effort"] = json!(effort);
        }
        self.request("turn/start", turn).await?;
        let mut answer = String::new();
        loop {
            let msg = self.next().await?;
            let p = &msg["params"];
            if p["threadId"] != thread_id.as_str() {
                continue;
            }
            if msg["method"] == "item/completed" && p["item"]["type"] == "agentMessage" {
                answer = p["item"]["text"].as_str().unwrap_or_default().to_owned();
            }
            if msg["method"] == "turn/completed" {
                if p["turn"]["status"] != "completed" {
                    return Err(p["turn"]["error"]["message"]
                        .as_str()
                        .unwrap_or("Could not correct the text.")
                        .to_owned());
                }
                return serde_json::from_str(&answer)
                    .map_err(|_| "Could not read the edits. Please try again.".to_string());
            }
        }
    }
}

fn normalize_models(models: Vec<Value>) -> Vec<Value> {
    models
        .iter()
        .filter(|m| m["hidden"] != true)
        .filter(|m| {
            m["inputModalities"]
                .as_array()
                .is_none_or(|modes| modes.iter().any(|mode| mode == "text"))
        })
        .filter_map(|m| {
            let id = m["model"].as_str()?;
            Some(json!({
                "id": id,
                "name": m["displayName"].as_str().unwrap_or(id),
                "description": m["description"].as_str().unwrap_or_default(),
            }))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn models_keep_visible_text_models() {
        let models = super::normalize_models(vec![
            serde_json::json!({"model":"a","displayName":"A","inputModalities":["text","image"]}),
            serde_json::json!({"model":"b","hidden":true}),
            serde_json::json!({"model":"c","inputModalities":["audio"]}),
            serde_json::json!({"model":"d"}),
        ]);
        let ids: Vec<_> = models.iter().map(|m| m["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["a", "d"]);
        assert_eq!(models[0]["name"], "A");
    }

    /// Live check with GrammarBar's own Codex sign-in:
    /// `cargo test -- --ignored live_codex --nocapture`
    #[test]
    #[ignore]
    fn live_codex_draft() {
        tauri::async_runtime::block_on(async {
            let tools = crate::cli::detect().await;
            let binary = tools.require(crate::cli::Provider::Codex).unwrap();
            // GRAMMARBAR_CODEX_HOME can point at another signed-in Codex home.
            let home = std::env::var_os("GRAMMARBAR_CODEX_HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| {
                    std::path::PathBuf::from(std::env::var("HOME").unwrap())
                        .join("Library/Application Support/com.tabmk.grammarbar/session")
                });
            let workspace = std::env::temp_dir();
            let mut server = super::Server::start(&tools, binary, &home, &workspace)
                .await
                .unwrap();
            server.ensure_account().await.unwrap();
            assert!(!server.models().await.unwrap().is_empty());
            let text = "Hi\u{a0}team, me and him goes to the cafe\u{301} everyday.\n";
            let request = crate::edits::Request {
                text,
                instruction: "Fix grammar.",
                model: None,
                effort: Some("low"),
            };
            let started = std::time::Instant::now();
            let draft = server.draft(&request, &workspace).await.unwrap();
            let segments = crate::align::align(text, &draft).unwrap();
            for s in segments.iter().filter(|s| s.original != s.replacement) {
                println!("{:?} -> {:?}: {}", s.original, s.replacement, s.explanation);
            }
            println!("{:?}", started.elapsed());
            assert_eq!(
                segments
                    .iter()
                    .map(|s| s.original.as_str())
                    .collect::<String>(),
                text
            );
            assert!(segments.iter().any(|s| s.original != s.replacement));
        });
    }
}
