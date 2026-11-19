<h1 align="center">GrammarBar</h1>

<p align="center">Fix your English from the macOS menu bar.<br>Keep your voice, see every change, and learn why it was made.</p>

<p align="center">
  <a href="https://github.com/TABmk/GrammarBar/issues"><img src="https://img.shields.io/github/issues/TABmk/GrammarBar?style=flat-square" alt="Issues"></a>
  <a href="https://github.com/TABmk/GrammarBar/pulls"><img src="https://img.shields.io/github/issues-pr/TABmk/GrammarBar?style=flat-square" alt="Pull requests"></a>
  <a href="#license"><img src="https://img.shields.io/badge/license-not%20specified-lightgrey?style=flat-square" alt="License: not specified"></a>
  <a href="https://github.com/TABmk/GrammarBar/stargazers"><img src="https://img.shields.io/github/stars/TABmk/GrammarBar?style=flat-square" alt="Stars"></a>
</p>

<p align="center">
  <img src="docs/grammarbar.png" width="900" alt="GrammarBar showing original English text, corrected text, red and green changes, and an explanation of a grammar correction.">
</p>

## Quick Start

1. Open the **[latest release](https://github.com/TABmk/GrammarBar/releases/latest)** and download the ZIP under **Assets**:
   - **Apple Silicon (M-series):** `GrammarBar-macos-arm64.zip`
   - **Intel:** `GrammarBar-macos-x64.zip`
2. Install at least one AI command-line tool. GrammarBar uses your subscription through it:
   - **Claude** (Pro, Max, Team or Enterprise): [Claude Code](https://code.claude.com/docs/en/setup)
   - **ChatGPT**: [Codex CLI](https://developers.openai.com/codex/cli)
3. Unzip GrammarBar, move **GrammarBar.app** to **Applications**, and open it.
4. Click the GrammarBar menu-bar icon, open **Settings**, choose a provider and press **Connect**. If the provider is not installed, Settings links to its install page. Save your instructions and preferred model.
5. Paste your text and press **Correct** or **⌘↵**. Hover over a change to see its explanation.

## Features

- **One-click access** — opens below the menu-bar icon and hides when you click away.
- **Claude or ChatGPT** — use the subscription you already have; the model list comes from your account, so new models appear automatically.
- **Your writing rules** — save custom instructions and choose a model.
- **Visible corrections** — removed text is red and struck through; additions are green.
- **Explanations on hover** — understand the reason behind each correction.
- **A compact workspace** — resize the window vertically; changes appear only when present.
- **Copy and clear** — copy the result or clear all text for your next message.

<details>
<summary><strong>Providers and privacy</strong></summary>

GrammarBar bundles no AI runtime. It looks for `claude` and `codex` in your login-shell `PATH` and in common install folders (`~/.local/bin`, Homebrew, npm, Bun, Volta). Press **Check again** in Settings after installing one.

- **Claude** runs through the official Claude Code CLI (`claude -p`). Sign-in is done by `claude auth login`, so GrammarBar never handles Claude credentials and shares the sign-in with your terminal. If the browser shows a code after sign-in, paste it into Settings. Models come from the account Claude Code is signed in to. To sign out, run `claude auth logout`.
- **ChatGPT** runs through the official Codex App Server (`codex app-server`) over local stdio. It does not automate the ChatGPT website.

Text is submitted for correction only when you press **Correct**. Corrections run in fresh, non-persistent sessions, and the editor does not save a text history. Filesystem, shell, web, MCP and other tools are disabled; unexpected tool requests are refused.

Codex stores credentials in macOS Keychain using an app-specific home. Settings live in `~/Library/Application Support/com.tabmk.grammarbar/settings.json`. Existing installations keep the legacy `local.textfixer` directory to preserve sign-in and settings.

How a correction works:

- Long texts are split at paragraph (then sentence) boundaries into parts of about 1,800 characters. Claude corrects up to three parts at once; Codex corrects them in turn. A part that fails is retried once; if it still fails, it is left unchanged and the app says so.
- The model returns the corrected text and a list of changes with explanations. GrammarBar compares the corrected text with yours word by word, so highlights cover only the words that changed. Invisible or typographic differences the model may introduce (Unicode normalization, non-breaking or zero-width spaces, curly quotes, edge whitespace) keep your original characters.
- **Speed → Fast** asks for low reasoning effort, which is enough for grammar; **Thorough** uses the model's default. With Claude, the default model is Sonnet.
- Rust checks that the result reconstructs your text exactly before displaying it. The interface renders model output as plain text, never HTML; whitespace-only edits are shown as `␣`, `¶` and `⇥`.

</details>

<details>
<summary><strong>Development</strong></summary>

Built with Rust, Tauri 2, and plain HTML/CSS/JavaScript. Requires Rust, Node.js, and Apple Command Line Tools (`xcode-select --install`). CI uses Node.js 22 and stable Rust.

```sh
git clone https://github.com/TABmk/GrammarBar.git
cd GrammarBar
npm ci
npm run dev
```

Build and open a standalone app:

```sh
npm run build
open 'src-tauri/target/release/bundle/macos/GrammarBar.app'
```

Run checks from a fresh checkout:

```sh
npm test
cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings
```

Live checks against the installed, signed-in CLIs: `cargo test --manifest-path src-tauri/Cargo.toml -- --ignored live_` (`live_codex` uses GrammarBar's Codex sign-in, or `GRAMMARBAR_CODEX_HOME`). The Codex App Server protocol is version-dependent: re-test sign-in and corrections after major Codex updates.

The transparent popover uses Tauri's `macos-private-api` feature. An App Store release would need a different implementation.

</details>

<details>
<summary><strong>Release workflow</strong></summary>

[GitHub Actions](https://github.com/TABmk/GrammarBar/actions/workflows/release.yml) builds and tests Apple Silicon and Intel versions on every pushed tag. After both succeed, it publishes the ZIP archives to GitHub Releases.

Update the version in `package.json`, `package-lock.json`, `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock`, and `src-tauri/tauri.conf.json`, then commit and push the changes before tagging. For example:

```sh
git tag v0.1.0
git push origin v0.1.0
```

The workflow uses the built-in `GITHUB_TOKEN`; no additional secrets are required. Re-running a tag workflow replaces its ZIP assets. Apple Developer signing and notarization are not configured. Never bypass a macOS malware warning; obtain a verified replacement from its official source.

</details>
