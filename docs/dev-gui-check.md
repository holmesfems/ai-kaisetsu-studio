# 開発版を GUI で確かめる

CLAUDE.md「作業の流れ」4. の手順と注意。開発版を起動し、Playwright の MCP で画面を操作・スクリーンショットして、仕様の「確かめ方」どおりに動くかを見る（許可済み）。

## 起動

WebView2 のデバッグ用ポートを開けて開発版を起動する。

- Git Bash: `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222 npm run tauri dev`
- PowerShell: `$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=9222"; npm run tauri dev`

## Playwright の MCP

- `~/.claude.json` に `cmd /c npx -y @playwright/mcp@latest --cdp-endpoint http://127.0.0.1:9222` で登録してある（CDP でアプリの画面につなぐ）
- 開発版より先に Claude Code を起動したときは、MCP の再接続が要ることがある
- 本物のアプリの画面なので `window.__TAURI_INTERNALS__.invoke("get_state")` などで状態も直接読める
- スクリーンショットは `.playwright-mcp/` に出る（git には入れない）

## 確かめる回

確かめるのは回の写し（例 `E:\tmp\an389_copy`）で。実際の回は書き換えない。

## computer-use を使うとき

開発版（`target\debug\ak-studio.exe`）だけを許可し、`open_application` は使わない。
開発版がもう 1 つ起動して `session.json` を上書きし、MCP が止めた側を指すため。

## 注意

- `tauri dev` が動いている間に `cargo test` を並行して走らせるとリンクが壊れることがある（そのときは `cargo clean -p ak-studio`）
- モニターの見た目だけなら ak-studio の MCP の `render_still` でもよい
- ブラウザで Vite（5174）を開いても Tauri の `invoke` が無いので動かない
