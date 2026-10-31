# 声のエンジン

AK Studio は、声を作るソフト（声のエンジン）を差し替えられる。同梱しているのは **VoiSona Talk** と **VOICEVOX**。
約束（下の「約束 v1」）どおりの Python のファイルを 1 つ書けば、fork せずにほかのエンジンも使える。

## しくみ

アプリは声を直接作らず、裏で動かす **声の中継**（`voice/voice_server.py`、`127.0.0.1:8765`）に頼む。
中継は、どのエンジンでも同じ仕事（回の切り替え・声のキャッシュ `voice_cache/`・声のつなぎ `voice_out/`・話者ごとの調整 `_voice_speakers.json`・読みの辞書）を受け持ち、
1 行の声を作るところだけを **エンジンのモジュール** に任せる。

```
画面・Rust ──HTTP──▶ 声の中継（voice_server.py）──▶ エンジンのモジュール ──▶ VoiSona Talk / VOICEVOX / 自作
                        共通の部品は voicecommon.py
```

使うエンジンは ⚙ 設定の「声のエンジン」で選ぶ。保存すると中継を起動し直し、声の長さを取り直す。
上の帯の丸には、使っているエンジンの名前（VoiSona Talk・VOICEVOX など）が出る。

## 同梱のエンジン

| エンジン | 準備 | 使える機能 |
|---|---|---|
| VoiSona Talk（`voisona`） | VoiSona Talk を起動し、環境設定で API をオンにする。設定に VoiSona のメールアドレスと API パスワード | すべて（アクセントの手直し・音素の長さ・行ごとの抑揚・スタイルの混ぜ具合） |
| VOICEVOX（`voicevox`） | VOICEVOX を起動する（エンジンは `http://127.0.0.1:50021`。違う場所なら設定の「VOICEVOX の場所（URL）」） | 読みの辞書・速さ・高さ・抑揚・音量、行ごとの抑揚 |

- 話者（トーク1・トーク2）にどの声を当てるかは、エンジンごとのプリセット（`voisona_presets.json`・`voicevox_presets.json`）で決める。
  VOICEVOX の既定は トーク1＝ずんだもん（ノーマル、`style_id: 3`）・トーク2＝四国めたん（ノーマル、`style_id: 2`）
- **VOICEVOX は声ごとに利用規約がある**。動画には「VOICEVOX:ずんだもん」のようなクレジットが要る。使う声の規約を確かめること
- エンジンを切り替えると、声はそのエンジンで作り直しになる。エンジンごとにキャッシュは別なので、元のエンジンに戻せば前の声がそのまま鳴る

## 約束 v1

エンジンは Python のモジュール 1 つ。次の名前を持つ。

```python
ENGINE_API = 1            # 約束の版。中継は 1 だけ受け付ける
NAME = "myengine"         # 鍵（キャッシュの鍵に入る）。英小文字・数字・_ だけ
LABEL = "My Engine"       # 画面に出す名前
FEATURES = {"intonation"} # できること（下の表）。空でもよい
PRESETS = "myengine_presets.json"  # 話者のプリセット（このファイルと同じフォルダ）
RANGES = {"speed": (0.5, 2.0), "pitch": (-0.15, 0.15), "intonation": (0.0, 2.0), "volume": (0.0, 2.0)}
# 話者の声の項目と範囲。画面の「話者の声」（トラックの ⚙）の項目はここから作る

def check():
    """中継の起動時に 1 回。つながらない・準備が足りないときは RuntimeError(理由) を投げる。"""

def voice_info(preset):
    """話者のプリセット 1 人分から {"style_names": [...], "default_style_weights": [...]} を返す。無ければ {}。"""

def synthesize(speaker, text, out_path, presets, analyzed_text, entries, params):
    """1 行を 16bit モノラルの WAV にして out_path に書き、次を返す:
    {"duration": 秒, "unread": [読めなかった言葉], "analyzed_text": 文字列か "",
     "phonemes": [...] か None, "phoneme_durations": [...] か None, "durations_ignored": bool}"""
```

| FEATURES | 意味 | 無いとき |
|---|---|---|
| `accent` | 解析結果（`analyzed_text`）を返し、渡すと手直しどおりに作る。辞書のアクセントも当てる | アクセントの手直しを渡さない。アクセントの画面は「使えません」と出し、辞書の「アクセントも指定する」を隠す |
| `durations` | 音素の並びと長さを返し、長さの手直しを受け付ける | 長さの手直しを渡さない。アクセントの画面の「長さ」を隠す |
| `intonation` | 行ごとの抑揚（`params["intonation"]`）を受け付ける | 行ごとの抑揚を渡さない。抑揚のつまみを隠す |
| `styles` | 話者の声のスタイルの混ぜ具合（`style_weights`） | スタイルの帯を隠し、`style_weights` を渡さない |

- `presets["speakers"][speaker]` は `{"voice_name": 画面に出す名前, "global_parameters": {RANGES の項目: 値}, …}`。ほかの項目（VOICEVOX の `style_id` など）はエンジンが自由に足してよい。`presets["pauses"]` は句読点の間（秒）
- `entries` はその文に関わる読みの辞書の項目（`{"surface", "reading", "accent"?}`）。読みの置き換えは `voicecommon.apply_reading(text, entries)` で
- 読めない・入口が欠けている・版が違うエンジンは、中継が起動時に理由を出して止まる（画面に出る）
- 中継の `/api/ping` の `engine`（`name`・`label`・`features`・`api`、ファイルで読んだときは `path`）で、読み込まれたエンジンを確かめられる

## 自作のエンジンを足す

1. `voice/voicevox_tts.py` を写して、`NAME`・`LABEL`・`PRESETS` と `check`・`synthesize` を自分のエンジンに合わせて書き換える（標準ライブラリだけで書くと、Python に何も足さずに動く）
2. 同じフォルダに話者のプリセット（`PRESETS` の名前の JSON。形は `voice/voicevox_presets.json`）を置く
3. ⚙ 設定の「声のエンジン」で「ファイルを指定…」を選び、その `.py` の絶対パスを入れて保存する
4. 上の帯の丸がそのエンジンの `LABEL` になり、緑になれば使える。赤なら押すと理由が出る

共通の部品（`voicecommon`）は、中継が読めるようにしてあるので、自作のエンジンからそのまま `from voicecommon import apply_reading, relevant, clean_params, wav_duration` で使える。

試すときは、中継を手で起動すると早い:

```
python voice/voice_server.py --project <回のフォルダ> --engine <自作の .py>
```

## アクセントの手直しについて

アクセントの画面（台本の行の A）と voisona-accent スキルは、VoiSona Talk の解析結果の形（`<word phoneme hl chain …>`）を前提にしている。
今は VoiSona Talk でだけ使える。ほかのエンジンでは、読みの辞書で読みを直す。

## キャッシュの鍵

声のキャッシュ（`voice_cache/<鍵>.wav`）の鍵は、話者・文・手直し・話者の声・句読点の間・関わる辞書の項目と、VoiSona 以外ではエンジンの `NAME` から決まる。
VoiSona の鍵にはエンジンの名前を入れていない（エンジンを差し替えられるようにする前に作った声も、そのまま使える）。
