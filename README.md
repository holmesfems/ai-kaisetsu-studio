# AK Studio

[![CI](https://github.com/holmesfems/ai-kaisetsu-studio/actions/workflows/ci.yml/badge.svg)](https://github.com/holmesfems/ai-kaisetsu-studio/actions/workflows/ci.yml)

AK Studio（AI Kaisetsu Studio）は、解説動画のための編集アプリ。台本・声・演出・合成をひとつにまとめ、
Claude と一緒に作る。Premiere Pro のように「モニター＋タイムライン」で見ながら、

- 台本を書く（2 人の掛け合いなど。行ごとにメモを付けられる）
- VoiSona Talk で声を作る（読み・アクセントの手直しつき）
- Python で書いた演出（テロップ・表・図解・録画の窓）を、コードを書き換えずに手で微調整する
- 背景・録画・立ち絵・演出・字幕・声・効果音・BGM を合成して mp4 に書き出す

編集中は Claude Code とつながり、画面のチャットから台本の推敲や演出の直しを頼める。

- アプリ本体: Tauri v2（Rust）＋ React。合成・キャッシュ・再生・書き出しは Rust
- 演出: 各回の `fx/<名前>_fx.py`（fxkit）を読み込んで描く。Python のワーカーを並列で常駐させる
- 声: `voice/voice_server.py`（VoiSona Talk の中継）を裏で動かす
- Claude: MCP サーバー（`ak-studio-mcp.exe`）と Claude Code の Mod（`claude-plugin/ak-studio`）

## 要るもの

- Windows 10 / 11（WebView2 は Windows 11 には入っている）
- [Rust](https://rustup.rs/)（MSVC）と [Node.js](https://nodejs.org/)
- Python 3.10 以上（演出のワーカーが使う。`pillow numpy scipy fonttools imageio-ffmpeg`）
- [VoiSona Talk](https://voisona.com/)（声を作るとき。別に入れて、環境設定で API をオンにする）
- [Claude Code](https://claude.com/claude-code)（Claude とつなぐとき）

## 入れ方・起動

```
git clone https://github.com/holmesfems/ai-kaisetsu-studio
cd ai-kaisetsu-studio
npm install
python -m pip install -e fxkit --config-settings editable_mode=compat   # 演出のエンジン（初回だけ）
npm run tauri build -- --no-bundle
target\release\ak-studio.exe
```

開発中は `npm run tauri dev`（画面はホットリロード、Rust は保存で作り直し）。
MCP サーバーは `cargo build --release -p ak-studio-mcp`。テストは `npm run test:all`。
開発の手引きは [CLAUDE.md](CLAUDE.md)、構成は [docs/architecture.md](docs/architecture.md)。

アプリを開いたら、動画 1 本ぶんのフォルダ（「回」）を選ぶ。回のフォルダの中に `script/`（台本）・`fx/`（演出）・`studio/`（アプリの保存）ができる。
画面の見方・書き出し・字幕・効果音は [docs/usage.md](docs/usage.md)、台本の書式は [docs/script-format.md](docs/script-format.md)。

## 演出の kit

演出の見た目（テロップや図の絵・シーンの札・録画の窓の枠・効果音の音）は「kit」が決める。
**kit が無くても動く**。そのときは素の kit で描く（白い文字と細い枠だけの地味な見た目。画面には「演出の kit がありません」と情報が出る）。

自分の動画の見た目にしたいときは、kit を作って `fx-private/kit/` に置く（か、環境変数 `AK_FX_PRIVATE` で指す）。
色・字幕・声の既定もプリセットのフォルダで上書きできる。作り方は [docs/kit.md](docs/kit.md)。

## Claude Code とつなぐ

アプリの右の「Claude」タブ（か ⚙ 設定）で「Claude Code に登録」を 1 回押すと、MCP サーバー `ak-studio` と Mod `ak-studio` が入る。
「Claude を起動」でその回のフォルダに Claude Code が開き、画面のチャットで送ったメッセージがそのセッションに届く。
台本は Claude の「推敲案」として出て、差分を見て 1 件ずつ採用する。メッセージの届け方・モニターの印・取り消しは [docs/usage.md](docs/usage.md#claude-code-とつなぐ)。

## ライセンス

[MIT](LICENSE)。依存のライセンスは [THIRD_PARTY.md](THIRD_PARTY.md)（`python tools/third_party.py` で作り直す）。
