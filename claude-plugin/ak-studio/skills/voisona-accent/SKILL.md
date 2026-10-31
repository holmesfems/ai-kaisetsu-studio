---
name: voisona-accent
description: VoiSona Talk で読み上げるトーク台本（AK Studio の各回の script/*.yaml）の、アクセント（イントネーション）・読み・アクセント句の区切りを直すスキル。AK Studio の voice\ の道具で出した解析結果を一覧にして、辞書（voice_dict.json）と行ごとの手直し（_voice.json。行の id が鍵）の案を出し、ユーザーが「アクセント確認」画面（http://localhost:8765/review）で試聴・調整・採用したものだけを書き込む。「アクセント直して」「イントネーションがおかしい」「読み間違ってる」「辞書に登録して」「VoiSona の読み」「アクセントの案を出して」「アクセント確認の結果を保存しました」「採用したものを反映して」のように、台本の読み上げ音声の高低・読み・区切りの話が出たら、明示的な指定がなくても必ずこのスキルを使うこと。
---

# VoiSona のアクセント調整

VoiSona の既定のイントネーションは**かなり間違っていることが多い**。⚠ の付いた所だけでなく、不自然な所は**大胆に直す案を出してよい**（ユーザーの了承済み）。案は確認画面で一つずつ試聴して採用／不採用を決めるので、迷ったら出す側に倒す。書き込むのは、ユーザーが採用したものだけ。

## 道具（`%LOCALAPPDATA%\ak-studio\voice\`）

AK Studio のリポジトリの `voice\` と同じもの。AK Studio が埋め込んで `%LOCALAPPDATA%\ak-studio\voice\` に書き出し、中継（`voice_server.py`）を裏で動かす。以下の `python` は、このフォルダ（`<voice>` と書く）で動かす。

| ファイル | 役目 |
|---|---|
| `akscript.py` | 台本（`script\*.yaml`）と `_voice.json`・`_voice_params.json` を読む共通の部品（ほかの道具が使う） |
| `voice_report.py` | 全行の解析結果を一覧にする → `<回>\voice_report.md`（各行に `番号` と行の `id`） |
| `voice_review.py build` | Claude の案（draft.json）→ 確認画面用の `<回>\voice_review\proposal.json` |
| `accent_review.html` | 確認画面（サーバーの `/review`）。▶今／▶案の試聴、モーラのクリックで高低、単語の間のクリックで区切り、採用／不採用／保留、メモ。`result.json` に自動保存 |
| `voice_review.py apply` | `result.json` の採用分を、バックアップしてから `voice_dict.json` と `script\_voice.json`（行の id が鍵）に書き込む |
| `voice_dict.json` | 辞書（全部の回で共通）。置き場所は `%LOCALAPPDATA%\ak-studio\voice_dict.json`（環境変数 `AK_VOICE_DICT` で変えられる。道具のフォルダには無い）`{"entries":[{"surface","reading","accent"}]}` |
| `voisona_tts.py` / `voice_server.py` | 合成とサーバー（`voice_server.py` は `/` を `/review` へ転送する）。辞書の当て方は `apply_accent` |

draft.json の形と細かい仕様は `voice_review.py` の先頭の説明にある。
- 台本は `<回>\script\<ブロック>.yaml`（`title` / `notes` / `lines: [{id, sp, text, start?, memo?}]`）。**行は `id` で指す**（draft.json の lines も `{"id": "l51x9k", ...}`。`file`・`no` は画面の表示用に proposal／result へ付くだけ）。形式の詳細は studio\docs\script-format.md。
- `_voice.json` は `{"<行の id>": {"sp": 話者, "text": 読み上げ用の本文（`｜漢字《よみ》` は読みに置き換えたもの）, "analyzed_text": 解析結果のXML}}`。**今の行の話者と読み上げの文が sp・text と同じときだけ効く**（セリフを直すと、その手直しは使われなくなる）。
- 古い形式（`.md` の台本）の回は、道具がエラーで止まる。AK Studio でその回を開くと新しい形式に移るので、先に開いてもらう。

## 手順

### 0. 前の確認
- 中継（`voice_server.py`）は、AK Studio が回を開くと裏で起動する（`src-tauri/src/voice/relay.rs`）。すでに動いていればその回に切り替わる。対象の回で動いているか：`curl -s localhost:8765/api/ping` の `project`（ポートは AK Studio の設定の `voice_port`。既定 8765）。
  違う・応答しないときは、AK Studio でその回を開いてもらう（上部の VoiSona をクリックすると中継を再起動できる）。別の回で起動した古い中継や、辞書が別の古い中継が残っているとエラーになるので、閉じてもらう。
- まだ解析していない行があれば、AK Studio の台本タブの「音声を一括作成」で音声を作ってもらう（VoiSona の解析結果は音声を作るときに `voice_cache` に入る。一覧は `voice_cache` を読むだけで、自分では音声を作らない）。

### 1. 一覧を作って読む
```
cd <voice>
set PYTHONUTF8=1
python voice_report.py --project <回のフォルダ> --words
```
`<回>\voice_report.md` を**全行**読む。各行は「**番号** `id` 話者：セリフ」で、案の行はこの `id` で指す。書き方：`／`＝アクセント句の区切り、`・`＝同じ句の単語、`［ ］`＝高いモーラ、`⚠`＝読めない言葉・句の中が低→高→低でない・句の中が全部低い。末尾の単語一覧で、同じ言葉が行によって違う高低になっていないかも見る。

### 2. 案を考える（references/accent_rules.md と、あれば好みの表 `presets/accent_prefs.md` を読んでから。好みの表は AK Studio のリポジトリの直下の `presets/`（公開版に移った後は `fx-private/presets/`）にある。場所が分からなければユーザーに聞く）
- **辞書**に回すもの：何行にも出る言葉、細切れになる名前（人名・用語）、記号（A1〜A3、★6、R2D2 など）、読み間違い。辞書は全部の回で共通なので、この回だけの言い回しは入れない。登録済みの言葉（voice_dict.json）は重ねて出さない（直したいときは同じ表記で上書きの案にする）。
- **行の手直し**に回すもの：その行だけの高低・区切り。⚠ の行は全部見る。⚠ が無くても、助詞・助動詞だけの句、複合語の途中の区切り、下がった後に助詞が高い所などは直す。
- **自信がない**もの（読みが分からない、高低が2通り考えられる）は `"check": true` にして `question` と `options`（候補）を付ける。候補ボタンで選べるようにすると早い。
- `why` には「今どうなっていて、どう直すか」を短く書く（画面に出る）。

### 3. draft.json → proposal.json
`<回>\voice_review\draft.json` に書いて（Windows のパスや `\` を含むファイルは Write ツールで書く。Bash のヒアドキュメントだと `\v` などが制御文字になる）：
```
python voice_review.py build --project <回のフォルダ> <回のフォルダ>\voice_review\draft.json
```
前と案が表示されるので、案が狙いどおりか（⚠ が消えたか）を確かめる。`build` は前の result.json を退けて新しく始める。

### 4. ユーザーに確認してもらう
ブラウザで「アクセント確認」の画面（http://localhost:8765/review。ポートは上と同じ）を開くよう伝える。チャットには件数と、要確認の項目（質問）だけを短く書く。表を全部チャットに貼る必要はない。

### 5. 反映
ユーザーが「アクセント確認の結果を保存しました…反映してください」と言ったら：
1. `python voice_review.py apply --project <回のフォルダ>`（書き込まずに一覧を見る）
2. `result.json` の **note（メモ）を必ず読む**。メモは Claude への指示や感想（例：「他→タ」＝「他」をタと読んでいる）。メモから新しい直しが要るときは、案を書いてユーザーの OK をもらってから。
3. ユーザーが画面で変えた形（`changed: true`）は、その形のまま書く。好みとして好みの表（`presets/accent_prefs.md`。無ければ `fx-private/presets/accent_prefs.md`。どちらも無ければ `presets/accent_prefs.md` を新しく作る）に足す。`references/accent_rules.md` の表には足さない。
4. `python voice_review.py apply --project <回のフォルダ> --write`
5. AK Studio は `_voice.json` が変わると自動で読み込むので、タブを開き直す必要はない。「AK Studio で、変わった行を作り直し（再合成）てください」と伝える。辞書（voice_dict.json）を書き込んだときは、辞書の言葉を含む行も解析し直しになる。

## 気をつけること
- **手直し済みの行（今のセリフ・話者で _voice.json に手直しがある行）には辞書が効かない**。辞書の言葉を含む手直し済みの行は、行の案にも辞書の分を当てる（build が自動でやる）。採用すると、その行の前の手直しは上書きになる。
- 行ごとの**抑揚（intonation）と音素の長さ**は、ユーザーが AK Studio の「長さ」タブ・抑揚のつまみで決めて `script\_voice_params.json`（これも行の id が鍵）に入る。このスキルでは触らない（案にも出さない）。ピッチ曲線は VoiSona の API が受け付けないので調整できない。
- **読みを変える辞書**（R2D2 → アールツーディーツー、一見 → イチゲン など）は、解析し直さないと当たらない。その言葉を含む行の手直しは、辞書を書き込んで解析し直してから次の回で出す。
- 辞書の高低（accent）は読みのモーラ数と同じ長さ（拗音は前と1モーラ、ー・ッ・ンは1モーラ）。途中で下がる辞書なら、同じ句の後ろの助詞は自動で低くなる。
- 書き込みは必ず `voice_review.py apply --write` で（バックアップ：`_backup\claude-日時\`）。辞書を手で1件足すときも、先にバックアップを取る。
- 最後の確認はユーザーが耳でする。Claude は「良くなったはず」と言い切らない。
