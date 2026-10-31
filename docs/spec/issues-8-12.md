# 声のエンジンを差し替えられるようにする（#8〜#12）

調べた日: 2026-10-31（main 8f7a5af）

## 決めたこと（2026-10-31 ユーザーに聞いた）

| 問い | 答え |
|---|---|
| VoiSona のほかに同梱するエンジン | **VOICEVOX**（この PC に入っていて、実際に声を作って確かめられる） |
| fork せずに自分のエンジンを足せるか | **足せる**。設定で「エンジンの .py ファイル」を指す（kit と同じ考え方） |
| VoiSona 以外のアクセントの手直し（A の画面・voisona-accent） | **今回は VoiSona だけ**。ほかのエンジンでは「使えません」と出す。約束には「できるエンジンだけ」の入口を残す |

## 今の形（原因）

声は Python の中継 `voice/voice_server.py`（127.0.0.1:8765）が作る。Rust と画面は中継の HTTP（`/api/synth`・`/api/lookup`・`/api/assemble`・`/api/project`・`/api/ping`・`/api/speakers`・`/api/dict`・`/audio/<id>.wav`）しか使わない。
VoiSona に固有なのは次のところ。

- `voice/voice_server.py:24` が `voisona_tts` を直に import し、`vt.synthesize`・`vt.call("GET", "/voices")`（`voice_info`、:105-112）・`vt.GLOBAL_RANGE`（:145）・`vt._auth()`（:380）を呼ぶ
- `voice/voisona_tts.py`: VoiSona の API（:21-82）・解析結果の XML（`pause_groups` :88、`apply_accent` :158）と、どのエンジンでも使う辞書（`dict_path`〜`apply_reading` :109-152）・プリセット（`load_presets` :84、`clean_params` :211、`clean_global`・`apply_speakers` :228-268）が 1 つのファイルに混ざっている。`GLOBAL_RANGE`（:228）は VoiSona の範囲
- 中継の起動（`src-tauri/src/voice/relay.rs:76-96`）は `VOISONA_USER`・`VOISONA_PASS` を渡すだけで、エンジンを選ぶ口が無い。設定は `voisona_user`・`voisona_pass`（`settings/mod.rs:75-77`）だけ
- 埋め込み（`settings/embed.rs:64-74` の `VOICE_FILES`）は VoiSona の 2 ファイル（`voisona_tts.py`・`voisona_presets.json`）を前提にしている
- 画面: 話者の声の項目（`ui/src/logic/speakervoice.ts:31-41` の `VOICE_PARAMS`）が VoiSona の範囲と項目で決め打ち。アクセントの画面（`AccentEditor.tsx`）・辞書（`DictDialog.tsx`）は VoiSona の解析結果の XML（`<word phoneme hl chain pronunciation>`）を読む。文言の「VoiSona」は画面に 21 か所・Rust に 11 か所

一方で、中継の共通の部分（回の切り替え・キャッシュ・つなぎ `assemble`・話者の調整 `_voice_speakers.json`・辞書・アクセント確認の画面の配信）はエンジンに関係ない。なので、**中継は 1 つのまま、エンジンだけをモジュールにして差し替える**。

## 約束 v1（声のエンジンのモジュール）

エンジンは Python のモジュール 1 つ。中継が `--engine` で読む。

```python
ENGINE_API = 1            # 約束の版。中継は {1} だけ受け付ける
NAME = "voicevox"         # 鍵（設定の値・キャッシュの鍵に入る）。英小文字・数字・_ だけ
LABEL = "VOICEVOX"        # 画面に出す名前
FEATURES = {"intonation"} # できること（下の表）。無いものは中継と画面が使わない
PRESETS = "voicevox_presets.json"  # 話者のプリセットのファイル名（エンジンのファイルと同じフォルダ）
RANGES = {"speed": (0.5, 2.0), "pitch": (-0.15, 0.15), "intonation": (0.0, 2.0), "volume": (0.0, 2.0)}
# 話者の声（global_parameters）の項目と範囲。画面の項目もここから作る

def check():
    """起動時に 1 回。つながらない・認証が無いときは RuntimeError(理由) を投げる（中継はそれを出して止まる）。"""

def voice_info(preset):
    """話者のプリセット 1 人分から、声の情報 {"style_names": [...], "default_style_weights": [...]} を返す。
    無ければ {}。エンジンが止まっていれば例外（中継が error に入れる）。"""

def synthesize(speaker, text, out_path, presets, analyzed_text, entries, params):
    """1 行を 16bit モノラルの WAV にして out_path に書く。
    presets: 話者の調整を重ねたプリセット（presets["speakers"][speaker]["global_parameters"]・presets["pauses"]）
    analyzed_text: FEATURES に "accent" があるときだけ渡る（無ければ None）
    entries: その文に関わる辞書の項目（読みの置き換えは中継が済ませない。エンジンが voicecommon.apply_reading を使う）
    params: clean_params の形。"durations" は FEATURES に "durations" があるときだけ残る
    戻り値: {"duration": 秒, "unread": [...], "analyzed_text": 文字列か "", "phonemes": [...]か None,
             "phoneme_durations": [...]か None, "durations_ignored": bool}"""
```

| FEATURES | 意味 | 無いときの中継・画面 |
|---|---|---|
| `accent` | 解析結果（`analyzed_text`）を返し、それを渡すと手直しどおりに作る。辞書のアクセント（`accent`）も当てる | 中継は `_voice.json` の手直しを渡さず、キャッシュの鍵にも入れない。アクセントの画面は「このエンジン（◯◯）では使えません」。辞書の「アクセントも指定する」を隠す |
| `durations` | 音素の並びと長さを返し、長さの手直しを受け付ける | 中継は `params.durations` を捨てる。アクセントの画面の「長さ」を隠す |
| `intonation` | 行ごとの抑揚（`params.intonation`）を受け付ける | 中継は `params.intonation` を捨てる。抑揚のつまみを隠す |
| `styles` | 話者の声の `style_weights`（スタイルの混ぜ具合） | 画面のスタイルの帯を隠す。中継は `style_weights` を捨てる |

- 中継は `ping` に `"engine": {"name", "label", "features": [...], "api": 1}` を足す（画面と Rust はこれでエンジンを知る）。`/api/speakers` の `range` は `RANGES`
- 読めない・`ENGINE_API` が違う・入口が欠けているエンジンは、中継が起動時に理由を stderr に出して止まる（Rust がそれを画面に出す。今の「起動できません」と同じ経路）
- キャッシュの鍵（`voice_server.py:169` の `cache_key`）: `NAME != "voisona"` のときだけ `{"engine": NAME}` を足す。**VoiSona の今の声のキャッシュは鍵が変わらず、そのまま使える**
- 共通の部品は `voice/voicecommon.py` に移す: `dict_path`・`load_dict`・`save_dict`・`kata`・`relevant`・`apply_reading`・`load_presets(path)`・`clean_params`・`clean_global(over, n_styles, ranges)`・`apply_speakers(presets, over, ranges)`・`wav_duration`。`voisona_tts.py` は `from voicecommon import *` で今の名前を残す（`voice_review.py`・`voice_report.py`・テストはそのまま動く）

### エンジンの選び方（中継の引数）

`voice_server.py --engine <値>`。値は `voisona`（既定）・`voicevox`、または `.py` のパス（自作のエンジン）。

- 名前なら同じフォルダの `<名前>_tts.py`（`voisona_tts.py`・`voicevox_tts.py`）を import する
- パスなら `importlib.util.spec_from_file_location` でそのファイルを読む。プリセットはそのファイルと同じフォルダの `PRESETS`
- プリセットのファイルが無いエンジンは起動時にエラー（「話者のプリセット（◯◯）がありません」）

## #8 声の中継を「声のエンジン」の約束 v1 に分け、VoiSona をその 1 つにする

直し方（`voice/` だけ）:

1. `voice/voicecommon.py` を作り、上の共通の部品を `voisona_tts.py` から移す。`clean_global`・`apply_speakers` は `ranges` を引数に取る（既定は呼ぶ側が渡す。`voisona_tts.apply_speakers(presets, over)` は今の形で呼べるよう、`voisona_tts.py` に `RANGES` を渡す薄い包みを残す）。`load_presets(path)` はパスを取る
2. `voisona_tts.py` に約束 v1 の入口を足す: `ENGINE_API = 1`・`NAME = "voisona"`・`LABEL = "VoiSona Talk"`・`FEATURES = {"accent", "durations", "intonation", "styles"}`・`PRESETS = "voisona_presets.json"`・`RANGES = GLOBAL_RANGE`（`GLOBAL_RANGE` の名前も残す）・`check()`（今の `_auth()` を呼ぶ。`sys.exit` ではなく `RuntimeError`）・`voice_info(preset)`（今の `voice_server.voice_info` を移す）。`synthesize` は今の引数のまま（`presets` を受け取る）
3. `voice_server.py`:
   - `--engine`（既定 `voisona`）を足し、`load_engine(value)` で読む。入口（`ENGINE_API`・`NAME`・`LABEL`・`FEATURES`・`PRESETS`・`RANGES`・`check`・`voice_info`・`synthesize`）を確かめ、欠けていれば理由を出して `sys.exit(2)`
   - `vt.` の呼び出しを、共通の部品は `voicecommon`、エンジンのものは `ENGINE.` に替える。プリセットは `ENGINE_DIR / ENGINE.PRESETS`
   - `synth`・`lookup`: FEATURES に `accent` が無ければ `analyzed_text` を None に、`durations` が無ければ `params.durations` を、`intonation` が無ければ `params.intonation` を落としてから `cache_key` と `synthesize` に渡す（落とす関数 `effective(body)` を 1 つ作り、両方で使う）。`styles` が無ければ `apply_speakers` の前に `style_weights` を落とす
   - `cache_key`: `ENGINE.NAME != "voisona"` のときだけ `{"engine": NAME}` を足す
   - `ping` に `engine`、`speakers_info` の `range` を `ENGINE.RANGES`
   - `main()` の `vt._auth()` を `ENGINE.check()` に（`RuntimeError` は stderr に出して `sys.exit(2)`）
4. 変えないもの: HTTP の窓口と返りの形（足すのは `ping.engine` だけ）、`voice_review.py`・`voice_report.py`（VoiSona のアクセントの道具のまま）

テスト（`voice/test_voice_engine.py` を足す。素の assert）:
- `load_engine("voisona")` が入口をそろえて読める。入口の欠けた偽のエンジン（一時フォルダの .py）は理由つきで止まる（`SystemExit`）。`ENGINE_API = 2` も止まる
- 偽のエンジン（`FEATURES = set()`、`synthesize` が受け取った引数を覚えて 0.5 秒の無音を書く）で `synth` を呼ぶと、`analyzed_text` は None・`params` の `durations`・`intonation` は落ちて渡る。キャッシュの鍵に `engine` が入る
- `voisona` では `cache_key` が今と同じ値（`test_voice_speakers.py` の `test_cache_key_and_file` が通ること。値を固定した 1 件も足す）
- `ping()` に `engine` が入る
- 今の `voice/test_*.py` がそのまま通る

確かめ方: 開発版で今の回を開き、声が作り直されずに（キャッシュのまま）鳴る。1 行の文を変えて作り直せる。`/api/ping` に `engine` が出る。

## #9 VOICEVOX の声のエンジンを足す

直し方（`voice/` だけ）:

- `voice/voicevox_tts.py`（約束 v1。標準ライブラリだけ）:
  - `NAME = "voicevox"`・`LABEL = "VOICEVOX"`・`FEATURES = {"intonation"}`・`PRESETS = "voicevox_presets.json"`
  - `RANGES = {"speed": (0.5, 2.0), "pitch": (-0.15, 0.15), "intonation": (0.0, 2.0), "volume": (0.0, 2.0)}`（VOICEVOX の `speedScale`・`pitchScale`・`intonationScale`・`volumeScale`）
  - 場所は環境変数 `VOICEVOX_URL`（無ければ `http://127.0.0.1:50021`）
  - `check()`: `GET /version`。つながらなければ「VOICEVOX につながりません（<URL>）。VOICEVOX を起動してください」
  - `voice_info(preset)`: 何もしない（`{}`）。スタイルは話者のプリセットで 1 つ選ぶ（`style_id`）
  - `synthesize`: `apply_reading(text, entries)` → `POST /audio_query?text=…&speaker=<style_id>` → 返りの query の `speedScale` などを `global_parameters`（と `params.intonation`）で上書き → 句読点の間: `accent_phrases[].pause_mora` があれば `vowel_length` を `pauses["、"]`（無ければ `default`）に → `POST /synthesis?speaker=<style_id>` の WAV を `out_path` に書く（一時ファイル → rename）。`duration` は WAV のコマ数から（`voicecommon.wav_duration`）。`unread` は `[]`、`analyzed_text` は `""`、`phonemes`・`phoneme_durations` は None
  - HTTP の失敗は `RuntimeError("VOICEVOX: <メソッド> <パス> → <コード> <本文>")`
- `voice/voicevox_presets.json`: 既定の話者 2 人（`トーク1` = ずんだもん ノーマル `style_id: 3`、`トーク2` = 四国めたん ノーマル `style_id: 2`）。形は `{"_説明", "pauses", "line_gap", "speakers": {話者: {"voice_name": "ずんだもん（ノーマル）", "style_id": 3, "global_parameters": {"speed": 1.0, "pitch": 0.0, "intonation": 1.0, "volume": 1.0}}}}`。クレジットの注意（VOICEVOX は声ごとに利用規約があり、動画に「VOICEVOX:ずんだもん」などの表記が要る）を `_説明` に書く

テスト（`voice/test_voicevox.py`）: `http.server` で偽の VOICEVOX（`/version`・`/audio_query`・`/synthesis`）を立て、`VOICEVOX_URL` をそこに向ける。
- `synthesize` が `speaker` に `style_id` を渡し、`speedScale`・`pitchScale`・`intonationScale`・`volumeScale` を上書きして `/synthesis` に送る。辞書の読みが text に入る。`pause_mora.vowel_length` が `pauses["、"]` になる。WAV が書かれ `duration` が合う
- `check()` が、偽のサーバーが無いポートで理由つきの `RuntimeError`
- 中継に `--engine voicevox` で読ませると `ping.engine.name == "voicevox"`

確かめ方: VOICEVOX を起動し、#10 の後に設定で VOICEVOX を選んで見本の回の声を作る（#10 の確かめ方で一緒に）。

## #10 設定で声のエンジンを選べるようにする（同梱・自作のファイル）

直し方（Rust）:

- `Settings` に `voice_engine: String`（既定 `"voisona"`）と `voicevox_url: String`（既定 `""`＝`http://127.0.0.1:50021`）を足す。`SettingsPatch` にも。`voice_engine` は `voisona`・`voicevox`、または存在する `.py` の絶対パスだけを受け付け、それ以外は「声のエンジンが見つかりません: …」で弾く（純粋な関数 `validate_voice_engine` にしてテスト）
- 中継の起動（`voice/relay.rs:76-96`）に `--engine <voice_engine>` を足す。`voicevox_url` が空でなければ環境変数 `VOICEVOX_URL`。`VOISONA_USER`・`VOISONA_PASS` は今のまま
- 動いている中継が別のエンジン（`ping.engine.name` か、パスのときは `ping.engine.path`）なら、`ensure` は止めて起動し直す（今の「別の辞書」の判定の隣に）。#8 の `ping.engine` に `path`（パスで読んだときだけ）を足すこと
- 設定を保存して `voice_engine`・`voicevox_url` が変わったら、中継を止めて `ensure` し直し、`refresh_voice(true)` で長さを取り直す（`app/commands.rs` の設定の保存のところ）
- 埋め込み（`settings/embed.rs:64-74`）に `voicecommon.py`・`voicevox_tts.py`・`voicevox_presets.json` を足す。プリセットのフォルダ（`fx-private/presets/` など）に `voicevox_presets.json` があればそれを使う（`voice_files(presets)` に `voisona_presets.json` と同じ扱いで。`settings/presets.rs` の読み込みも同じ形の確かめ）
- 文言: Rust の「VoiSona の中継」は「声の中継」に（`relay.rs`・`analysis.rs`・`core/voice.rs` の 11 か所。VoiSona に固有の文はそのまま）。`ErrorCode::VoiceDown` の doc も
- `cargo test -p ak-studio -p ak-proto` で `ui/src/gen/` を作り直してコミットに入れる

テスト: `validate_voice_engine`（名前・パス・無いパス・知らない名前）、設定の patch（`voice_engine`・`voicevox_url`）、埋め込みの表（`voice_table_matches_dir` が新しいファイルで通る、プリセットの `voicevox_presets.json` を優先）、`ensure` のエンジン違いの判定（判定を純粋な関数 `engine_matches(ping, settings)` にしてテスト）。

確かめ方（#9・#11 とまとめて GUI で）: 見本の回の写しで、設定を VOICEVOX にして保存 → 上の帯が VOICEVOX になり、「音声を一括作成」で声ができて鳴る。VoiSona に戻すと、前の VoiSona の声がキャッシュから鳴る（作り直さない）。

## #11 画面を声のエンジンに合わせる

直し方（`ui/`）:

- `ui/src/logic/voiceengine.ts`（純粋な関数）: `engineOf(info)` → `{name, label, features: Set}`（`info.engine` が無い古い中継は VoiSona とみなす）、`has(engine, feature)`、`paramFields(range)`（`/api/speakers` の `range` から話者の声の項目を作る。知っている項目は今の `VOICE_PARAMS` の名前・単位、知らない項目は項目名のまま）。`ui/test/voiceengine.test.ts` を足す
- 上の帯（`TopBar.tsx:333-350`）: 丸の名前はエンジンの `label`（つながる前は「声」）。title・知らせの「VoiSona の中継」は「声の中継」
- 設定（`TopBar.tsx:132-215`）: 「声のエンジン」の選択（VoiSona Talk / VOICEVOX / ファイルを指定…）。ファイルのときはパスの入力。VoiSona のメールアドレス・パスワードは VoiSona のときだけ、「VOICEVOX の場所（URL）」は VOICEVOX のときだけ出す。`logic/settings.ts` の `TEXT_KEYS` に `voice_engine`・`voicevox_url`
- 話者の声（`Inspector.tsx:365-428`）: 項目は `paramFields(range)` から。スタイルの帯は `styles` があるときだけ。文言の「VoiSona Talk のグローバルパラメータ」は「声のエンジン（◯◯）の話者の値」に、プリセットのパスはエンジンの `PRESETS`
- アクセントの画面（`AccentEditor.tsx`）: `accent` が無ければアクセントの段の代わりに「このエンジン（◯◯）ではアクセントの手直しは使えません。読みは辞書で直せます」。`durations` が無ければ「長さ」の段を隠す。`intonation` が無ければ抑揚のつまみを隠す
- 辞書（`DictDialog.tsx`）: `accent` が無ければ「アクセントも指定する」を隠す。「VoiSona に任せる」は「声のエンジンに任せる」
- ほかの文言（`ScriptPanel.tsx:302`・`io/player.ts:118`・`logic/errors.ts:10`・`state/index.ts:89`・`state/voice.ts:104,129-130`）の「VoiSona」を「声のエンジン」「声の中継」に。`errors.ts` の voice_down の案内は「声のエンジン（VoiSona Talk・VOICEVOX など）が起動しているか確認してください」（`ui/test/errors.test.ts` も直す）
- 文言は docs/usage.md の用語に合わせる（「声」「声のエンジン」「声の中継」）

テスト: `voiceengine.test.ts`（古い中継・features・paramFields）、`errors.test.ts`、`settings.test.ts`。`npm run build` が通る。

## #12 声のエンジンの約束を文書にする

- `docs/voice-engine.md`: 約束 v1（上の表とコード）、同梱のエンジン（VoiSona Talk・VOICEVOX）の準備、自作のエンジンの作り方（`voicevox_tts.py` を写して直す・`PRESETS` を同じフォルダに・設定でファイルを選ぶ・`/api/ping` で確かめる）、できない機能の扱い、キャッシュの鍵
- README: 「要るもの」の VoiSona を「声のエンジン（VoiSona Talk か VOICEVOX。自作もできる）」に。「初めての設定」の表に「声のエンジン」の行
- docs/usage.md・CLAUDE.md（構成の 7. 声の中継）・docs/architecture.md の声の節
- `claude-plugin/ak-studio/skills/voisona-accent/SKILL.md`: 「VoiSona Talk を選んでいるときだけ使う」と冒頭に。version を上げる
- MCP の説明（`mcp/src/main.rs`）は VoiSona の名前を出していないので変えない

## 進め方

1 つのブランチ `feat/voice-engines` に、#8 → #9 → #10 → #11 の順で 1 issue 1 コミット（#9 の後に #10 が埋め込むファイルができる。#11 は #10 の生成した型を使う）。#12 は自分で書く。
最後に `npm run test:all`、GUI（見本の回の写し・VoiSona と VOICEVOX の両方）、レビューを通して 1 つの PR にする。
