# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

AK Studio（AI Kaisetsu Studio）— 解説動画の台本・声（VoiSona）・演出（fxkit / Python）・合成をまとめた編集アプリ。Tauri v2（Rust）＋ React。
入り方・要るものは [README.md](README.md)、画面の操作・書き出し・Claude とのつなぎ方の詳しい説明は [docs/usage.md](docs/usage.md)、台本の書式は [docs/script-format.md](docs/script-format.md)、演出の kit は [docs/kit.md](docs/kit.md)。

## コマンド

```
npm install                               # 初回だけ
npm run tauri dev                         # 開発（画面はホットリロード、Rust は保存で作り直し。Vite は 5174 固定）
npm run tauri build -- --no-bundle        # リリースのビルド → target\release\ak-studio.exe
cargo build --release -p ak-studio-mcp    # MCP サーバー → target\release\ak-studio-mcp.exe
npm run build                             # 画面の型チェック（tsc --noEmit -p ui）＋ vite build
```

テスト:

```
npm run test:all                                  # 全部（npm test → npm run test:py → cargo test -p ak-studio -p ak-studio-mcp -p ak-proto。1 つ落ちたらそこで止まる）
npm test                                          # ui/test/*.test.ts と claude-plugin のフック（node --test。.ts をそのまま実行）
node --test ui/test/merge.test.ts                 # 1 ファイルだけ
node --test --test-name-pattern "メモ" ui/test/merge.test.ts   # 名前で絞る
cargo test -p ak-studio                           # Rust（各モジュールの #[cfg(test)]）
cargo test -p ak-studio rekey::                   # モジュールで絞る（攻略動画は guide::）
cargo test -p ak-studio-mcp
cargo test -p ak-proto                          # 入口をまたぐ型（ui/src/gen/ の型もここと cargo test -p ak-studio で作り直される）
npm run test:py                                   # Python のテスト全部（engine/・fxkit/・voice/ の test_*.py を 1 本ずつ回して OK/NG と件数を出す。素の assert）
python tools/test_py.py fxkit                     # フォルダで絞る
python tools/test_py.py voice/test_voice_keys.py  # ファイルで絞る
```

Python の準備（初回だけ。設定の python と同じ Python で）:

```
python -m pip install -e fxkit --config-settings editable_mode=compat   # compat が要る（fxkit/README.md）
```

- Rust と同じ見本 `src-tauri/tests/fixtures/` を読む Python のテストは `fxkit/test_timeline.py`・`fxkit/test_motion.py`・`voice/test_voice_keys.py`。`script_cues.json`・`ease.json`・`trans_curves.json`・`glitch.json` は Rust が作る（式を変えたら `AK_UPDATE_FIXTURES=1 cargo test -p ak-studio`）。`voice_keys.json` は手で書く表で、TS（`ui/test/voicekeys.test.ts`）も読む
- 実際の回の台本を読むテストは `src-tauri/src/voice/layout.rs`（`AK_TEST_EPISODE` に回のフォルダを入れたときだけ動く。無ければ飛ばす）。読むだけで書き換えない。セリフの並べ方・声の解析・音声の署名は Rust が持つ（`core/voice.rs` の `refresh_voice`）
- UI のテストはアプリを起動せずに動く純粋な関数（`ui/src/logic/*.ts`）を対象にしている。ロジックはコンポーネントではなくそちらに置くとテストできる
- 画面の層: `logic/`（純粋な関数）・`io/`（`api`・`player`・`notify`）・`state/`（zustand。`useS` は `state/store.ts` の 1 つで、`history`・`script`・`voice`・`project`・`chat` の slice に分ける。部品は `state/index.ts` から import）。`logic/` は `state/`・`io/` を import しない（`ui/test/layers.test.ts`）
- import は `.ts` 拡張子つき（`allowImportingTsExtensions`。node --test がそのまま読むため）
- `ui/src/gen/` は ts-rs が書き出す TypeScript の型（Rust の `#[derive(TS)] #[ts(export)]`）。**手で直さない**。Rust の型を変えたら `cargo test -p ak-studio -p ak-proto` で作り直し、生成物（`git status ui/src/gen`）も一緒にコミットする。出力先は `.cargo/config.toml` の `TS_RS_EXPORT_DIR`

## 全体の構成

モジュールの依存・状態の持ち主・目標の置き場所と移す順番は [docs/architecture.md](docs/architecture.md)。

4 つのプロセス／部品と、Rust の共有クレート 1 つ、Python のライブラリ 2 つがある:

1. **Rust 本体（`src-tauri/`）** — `core/` の `Core`（`mod.rs` に定義、微調整 `overrides.rs`・見張り `watch.rs`・合成と先読み `compose.rs`・推敲案の採否 `proposals.rs`）が回（エピソード）の状態をすべて持つ。台本・微調整・合成・先読み・ファイルの見張り（notify）。
   - 画面とは 2 経路: Tauri の `invoke`（`app/commands.rs`、画面側は `ui/src/io/api.ts` の `call`）と、カスタム URI スキーム `akf://`（Windows では `http://akf.localhost/…`、`lib.rs`）でコマの JPEG・クリップの PNG・音声・素材ファイルを返す。変更の通知は Tauri のイベント（`on(...)`）
   - Claude 用のローカル HTTP API（axum、`app/api.rs` / `agent.rs`）を `127.0.0.1:47815`（使用中なら空き番号）で立てる。ポートと合言葉は `%LOCALAPPDATA%\ak-studio\session.json`
2. **Python ワーカー（`engine/akfx_worker.py`）** — 各回の `fx/<名前>_fx.py`（fxkit / fxparts）を読み込んで演出のコマを描く。シーン札・画面から足す演出・効果音の割り当ては kit（`ak_kit` / `ak_kit_basic`）経由。字幕の絵（`subtitle`）とフォント一覧（`fonts`）もここ。
   - Rust に `include_str!` で埋め込まれ、起動時に `%LOCALAPPDATA%\ak-studio\engine\` に書き出して使う（`settings/embed.rs`）。**直したら Rust を作り直さないと反映されない**
   - `render/engine.rs` がワーカーのプール（急ぎ・後回しの 2 列、固まったら止める・落ちたら作り直す）
3. **MCP サーバー（`mcp/src/main.rs`）** — stdio の MCP。中身は session.json を読んで本体の HTTP API を叩くだけ。`--wait` / `--context` はフック方式用
4. **Claude Code の Mod（`claude-plugin/ak-studio/`）** — プロンプトの上の状況の帯と、画面のチャットを `$.prompt.submit` でセッションに届ける。`skills/` は voisona-accent（声の手直し）だけ。演出・企画などのスキルは、非公開の kit の中の別の Mod（`fx-private/claude-plugin/ak-studio-fx/`）。直したら `plugin.json` の version を上げて入れ直す
5. **入口をまたぐ型（`crates/ak-proto/`）** — 画面と HTTP で共通の型（`Scene`・`MediaProbe`・`SeBankUpdate`）と、HTTP の要求の型（`http.rs`）。ts-rs で `ui/src/gen/` の TypeScript の型に、schemars で MCP の `inputSchema` になる（`mcp/src/main.rs` の `tools()` が使う。説明文は各項目の `///` の doc コメント）。依存は軽く保つ。画面に返すだけの形（`Snapshot` など）は `src-tauri/src/app/dto.rs`。コマンド・イベントの名前と型の表は `ui/src/io/commands.ts`（`call` / `on` はこの表で型づけされる。漏れは `cargo test -p ak-studio` の `app::tests` が見つける）
6. **fxkit（`fxkit/`）** — 演出の芯（`fxcore.py`。El・Sprite・Timeline・入り抜け・`RecWindowBase`）と、見た目つきの入口（`fxkit.py` = `from fxcore import *` ＋色・パネルなど）・部品（`fxparts.py`）・テンプレート・道具（`align_sync` など）。ワーカーは部品を直接 import せず、kit の入口 `ak_kit.py`（無ければ公開の素の kit `ak_kit_basic.py`。約束は [docs/kit.md](docs/kit.md)）を通す。探す順は「回の `fx/` のコピー → `pip install -e` した `fxkit/` → アプリに埋め込んだ版（`%LOCALAPPDATA%\ak-studio\fxkit\`、`AK_FXKIT_FALLBACK`）」。回を今の版に固定するのは `python -m fxfreeze <回>`。埋め込みの表 `FXKIT_FILES` は `src-tauri/build.rs` が作る（公開の `fxkit/` ＋あれば非公開の kit のフォルダ。場所は環境変数 `AK_FX_PRIVATE`、無ければ `fx-private/kit/`（gitignore）。同じ名前のファイルが 2 つあればビルドが止まる。表を作る関数は `settings/embed_table.rs`）。kit が無くてもビルド・起動できる（素の kit で描く）。帯には使っている kit（`kit: private 1a2b3c (API 1)`）を出す
7. **声の中継（`voice/`）** — `voice_server.py`（VoiSona Talk の中継）と、voisona-accent スキルの道具（`voice_review.py` など）。アプリは埋め込んだ版を `%LOCALAPPDATA%\ak-studio\voice\` に書き出して動かす（`VOICE_FILES`）。辞書は `%LOCALAPPDATA%\ak-studio\voice_dict.json`（`AK_VOICE_DICT`）

### 演出クリップのモデル（複数ファイルにまたがる要点）

- `fx/<名前>_fx.py` の `build()` 内の `add(...)` 1 回 = 1 クリップ。キーは `部品名@c<cue>#<順番>`（例 `telop@c15#1`）で、コードの行が動いても変わらない
- 手での調整はコードを書き換えず `fx/studio_overrides.json` に、画面から足した簡単な演出は `fx/studio_clips.json` に、RecWindow に差し込んだ録画は `studio/project.json` の `nests` に入る
- 台本の行の追加・削除で cue 番号がずれると、`script/rekey.rs` が前の `script.srt` と今のセリフを `[話者] 文` で突き合わせ、overrides と nests のキーを付け替える（書き換え前に控えを保存）
- コマのキャッシュ（`render/cache.rs`、LZ4）の鍵は「そのコマに出ているクリップの中身の指紋＋調整」。1 つ直してもほかの場面のキャッシュは残る
- `fx/*.py`・`files/script.srt`・`files/script_cues.json`・`sync_points.json` の変更は自動で読み直す。同期点の手直しは `syncman.rs`（`sync_manual.json`）
- シーン番号は Rust だけが決める（`render/scenes.rs`。ワーカーは札の絵を描くだけ）。字幕の見た目も Rust が決める（`render/subs.rs`）
- 入り・抜けの動き（緩急・Sprite の入りの曲線・グリッチの画素）は Rust（`render/ease.rs`・`trans.rs`）が正で、fxkit は見本（`src-tauri/tests/fixtures/` の `ease.json`・`trans_curves.json`・`glitch.json`。`AK_UPDATE_FIXTURES=1 cargo test -p ak-studio` で作り直す）と同じになるようにテストで縛ってある。式を変えたら見本を作り直して `fxkit/test_motion.py` も通す。ワイプの走査線の見た目は似せただけ。RecWindow の入り・抜けは RecWindow が持ち、ワーカーが desc の rec の `fade` で Rust に渡す

### 攻略動画（`guide/`・`core/guide.rs`）

- 台本の無い回は `studio/guide.json`（型は `crates/ak-proto/src/guide.rs`）が正。素材の解析 `studio/guide_analysis.json`（`fxkit/guide_scan.py`。画面の「攻略」タブの「解析」が Python で動かす）を読み、`guide/plan.rs`（純粋な関数）がアンカー・固定値・カットからパートを組み立てる。出力の t → 素材のコマの対応表は `studio/guide_map.json`（組み立てるたびに書く）
- 素材は可変 fps なので、コマ番号で読む（`render/media.rs` の `VideoSource::with_pts`・`MediaPool::video_pts`）。戦闘画面のレイヤー（70%・HUD の切り抜き・手動パートは 75% で左上）の座標は `guide/layer.rs` の `geometry` に 1 か所、合成は `core/compose.rs` の演出の手前（`core/guide.rs` の `draw_guide`）
- 固定値は 回の guide.json の `fixed` → 設定の `guide_fixed` → 既定（`plan::effective_fixed`）。画面の表の行は `ui/src/logic/guide.ts`
- 実際の素材での手動確認（`#[ignore]`）: `AK_TEST_GUIDE_VIDEO`・`AK_TEST_GUIDE_ANALYSIS`（・`AK_TEST_OUT`）を指定して `cargo test -p ak-studio real_ -- --ignored`

### 台本と声

- 台本は回の `script/<名前>.yaml`（ブロックごと、行は `id`・`sp`・`start`・`text`・`memo`）。読み書き・古い `.md` からの移行は `script/`（書式 `format.rs`・読み書き `store.rs`）、推敲案の適用は `script/ops.rs`
- 声のアクセントの手直し（`_voice.json`・`_voice_params.json`）は行の `id` が鍵。文や話者を直すと使われなくなる
- 声は `voice/voice_server.py`（VoiSona Talk の中継）を裏で動かして使う（`voice/relay.rs`。セリフの並び `VoiceItem` は `voice/layout.rs`）。**直したら Rust を作り直さないとアプリには反映されない**（埋め込み）
- 台本の自動保存は、Claude・voisona-accent・外のエディタの変更をファイルごとに見分けて上書きしない作り（`ui/src/logic/history.ts` の merge など）。ここを触るときは外の変更を黙って消さないこと

### 回のフォルダとの関係

編集対象はこのリポジトリの外の各回のフォルダで、回の中に `studio/`（`project.json`・チャット・推敲案・確認画像・`export/`）ができる。
回のフォルダが並んでいる所（設定の `an_root`）と、一覧から外す「ひな形のフォルダ」（`template_dir`）の既定は空で、画面の「回を開く」・設定で選ぶ。
今の動画の見た目・声・背景の既定はコードに書かず、非公開の `fx-private/presets/`（`subtitle.json`・`project.json`・`voisona_presets.json`・`ui/` など）をビルドで埋め込んで既定にする（無ければ素の既定）（`settings/presets.rs`、探す順は `build.rs`）。優先は「回の設定 → ユーザーの設定 → プリセット → 素の既定」。読めないプリセットは起動時の知らせに出して素の既定で動く。

手元の回の題材・Claude に補足する説明・好みの表など、題材に固有の情報は非公開の `fx-private/presets/` に置く。非公開の kit があるときは、まず `fx-private/README.md` を読む。

## 作業の流れ（issue ・機能の追加）

設計を先に固め、実装はサブエージェントに任せる。小さな直し（1〜2 ファイル・原因が明らか）は設計の文書を省き、自分で直してよい。

1. **調べて設計を書く（自分で）** — 原因を突き止め、`docs/spec/issues-<番号>.md`（まとめて扱うなら `issues-63-71.md` のように）に、issue ごとに
   「原因（`ファイル:行` つき）」「直し方」「テスト」「確かめ方」を書く。冒頭に調べた日と main のコミットを残す。
   決めるべきことはユーザーに聞き、答えた日と内容を仕様に反映してから実装に入る。
   汎用の機能の設計は公開してよい。kit に固有の設計は `fx-private/docs/` に書く
2. **実装は Sonnet のサブエージェントに任せる** — main から `feat/<内容>`・`fix/<issue>-<内容>` のブランチを切り、
   `Agent` で `model: "sonnet"` に、仕様のその issue の節（直し方・テスト）を渡して実装させる。1 issue ≒ 1 エージェント ≒ 1 コミット。
   触るファイルが重ならない issue は並行に出してよい（重なりうるなら `isolation: "worktree"` にするか、順に出す）。
   返ってきたら差分を自分で読み、仕様とずれていれば直させる
3. **テスト** — マージの前に `npm run test:all`（`npm test`・`npm run test:py`・`cargo test`）を通す。CI（GitHub Actions。kit なしの `npm run test:all`）もあるが、kit を含めた確かめは手元でしかできないので、マージの前に手元でも回す。ロジックは `ui/src/logic/*.ts` の純粋な関数に寄せて `ui/test/` にテストを足す
4. **GUI で確かめる（許可済み）** — 開発版を WebView2 のデバッグ用ポートを開けて起動し、Playwright の MCP（CDP でアプリの画面につなぐ）で操作・スクリーンショットして、仕様の「確かめ方」どおりに動くかを見る。
   確かめるのは回の写しで。実際の回は書き換えない。起動のコマンド・MCP の登録・computer-use・`cargo test` との並行など、手順と注意は [docs/dev-gui-check.md](docs/dev-gui-check.md)
5. **レビュー** — `/code-review` か pr-review-toolkit のエージェント（code-reviewer・silent-failure-hunter など）で差分を見る。
   直した分は「〜（レビュー）」のコミットにする
6. **PR とマージ（Claude がしてよい）** — PR を作り、3.〜5. がすべて通ったら（テストが緑・GUI で確かめ済み・レビューの指摘が直っている）
   ユーザーに聞かずに `gh pr merge --merge` でマージしてよい（履歴は「Merge pull request」のマージコミットの形）。
   テストが落ちている・GUI で確かめられなかった・ユーザーに聞くことが残っているときはマージせず、その理由を伝える

## 決まりごと

- コード中のコメント・コミットメッセージ・UI の文言は日本語。コミットは `#<issue>: 〜を直す` / `Fix #<issue>: …` の形、ブランチは `feat/…`・`fix/<issue>-…`
- UI の文言は次の方針と用語集に従う: ボタンは動詞で短く、ラベルは名詞、説明・通知・エラーはです・ます体、エラーは「〜できませんでした: 理由」。用語は統一（「効果音」であって「SE」ではない、「書き出し」「素材」「調整」「回」など）。台本・cue・トーク・演出などの制作用語は言い換えない
- ファイルの保存は原子的に（一時ファイル → rename）。読めないファイルを既定値で上書きせず、退避して知らせる（過去の監査で直した類のバグ）
- 失敗を黙らない: ワーカー・ffmpeg・VoiSona・HTTP API の失敗は画面に出す。書き出しで飛ばしたものは警告に出す
- 改行は LF（`.gitattributes`）
