import { cpSync, existsSync, mkdirSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { createRequire } from 'node:module';
import { dirname, join, resolve } from 'node:path';
const require = createRequire(import.meta.url);
const codexRequire = createRequire(require.resolve('@openai/codex/package.json'));
const architecture = process.arch === 'arm64' ? 'arm64' : 'x64';
const triple = architecture === 'arm64' ? 'aarch64-apple-darwin' : 'x86_64-apple-darwin';
const source = join(dirname(codexRequire.resolve(`@openai/codex-darwin-${architecture}/package.json`)), 'vendor', triple);
// Verify signatures without executing Codex or altering macOS security settings.
for (const name of ['codex', 'codex-code-mode-host']) {
  const binary = join(source, 'bin', name);
  if (!existsSync(binary)) throw new Error(`Missing Codex runtime: ${binary}`);
  execFileSync('/usr/bin/codesign', ['--verify', '--strict', binary], { stdio: 'inherit' });
}
const target = resolve('src-tauri/resources/codex-runtime');
mkdirSync(target, { recursive: true });
cpSync(source, target, { recursive: true });
console.log('Signed Codex 0.154.0 runtime copied (not executed).');
