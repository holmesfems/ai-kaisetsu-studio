# 演出の kit（自分の見た目を足す）

AK Studio は、演出の「仕組み」と「見た目」を分けている。

- 仕組み（このリポジトリの `fxkit/`）: タイムライン・要素（`El`・`Sprite`）・入りと抜けの動き・効果音を付ける仕組み・同期点の道具。
  芯は `fxcore.py`、効果音の仕組みは `fxse.py`
- 見た目（kit）: テロップや図の絵の描き方・色・シーンの札・録画の窓の枠・効果音の音と当て方。**kit が無くても動く**。
  そのときは素の kit（`fxkit/ak_kit_basic.py`。白い文字と細い枠だけの地味な見た目）で描く

自分の動画の見た目にしたいときは、kit を 1 つ作る。

## kit の入口: `ak_kit.py`

kit のフォルダに `ak_kit.py` を置く。ワーカー（`engine/akfx_worker.py`）は `import ak_kit` を試し、無ければ `ak_kit_basic` を使う。
`ak_kit.py` が持つ名前は次のとおり（形の見本は `fxkit/ak_kit_basic.py`）。

| 名前 | 形 | 使うところ | 素の kit |
|---|---|---|---|
| `KIT_API` | `int`（今は 1） | 版を確かめる | 1 |
| `NAME` | `str` | 画面の表示・ログ | `"basic"` |
| `scene_label(sid: str)` | RGBA の `Image` を返す | シーン番号の札 | 白い文字に黒の帯 |
| `studio_clip(kind, t0, t1, params)` | `El` のリストを返す。kind は `telop`・`note`・`stamp`・`chips` | 画面から足す簡単な演出 | 白い文字だけ（`stamp` は赤い枠） |
| `RecWindow` | `fxcore.RecWindowBase` を継いだクラス | 録画の窓。回の `fx/` から使う | 土台のまま（細い四角の枠） |
| `se_collect(els)` | 効果音の割り当てのリストを返す | 演出の動きから効果音を付ける | `[]`（自動の効果音なし） |
| `se_labels()` | 効果音の種類 → 名前の `dict` | 効果音の設定の画面 | `{}` |
| `SE_DIR` | `str` か `None` | 効果音の既定の棚（wav と `bank.json`）の場所 | `None` |
| `FEATURES` | `set[str]` | kit が持つ追加の機能の名前（攻略動画の解析・決まった演出・サムネ） | 空 |
| `COLORS`（任意） | 色の名前 → `(r, g, b)` | 画面から足す演出の色の名前 | 素の色 |

画面から足す 4 種類の演出（`telop`・`note`・`stamp`・`chips`）の種類と引数はアプリが決めていて、kit は絵だけを描く。
そのため、kit を変えても `fx/studio_clips.json` はそのまま使える。

### 録画の窓（RecWindowBase）

窓の位置と大きさ（`box`）・区切り（`segs`）・入りと抜け・素材が入っていない区間の仮表示は、土台の `RecWindowBase` が持つ。
kit の `RecWindow` は、次の 3 つを上書きして見た目だけを決める。

- `frame_image(w, h)`: 窓の枠の絵
- `label_image(idx, rid)`: 窓の名札の絵
- `overlay(fr, t, a)`: 窓の上に重ねるもの

### 版の確かめ方

アプリが読める版は `SUPPORTED_KIT_API`（今は 1）。合わないときは素の kit で描き、画面に警告を出す。

| 警告 | いつ |
|---|---|
| `kit-missing`（情報） | `ak_kit` が無いとき。kit なしのふつうの状態 |
| `kit-load` | `ak_kit` の読み込みで例外が出たとき |
| `kit-api` | `KIT_API` が無い、またはアプリが読めない版のとき |
| `kit-entry:<名前>` | 入口のうち、その名前だけが無いとき（その入口だけ素の kit で描く） |

約束を変えるときの決まり: 入口を足すだけなら版は上げない（古い kit は、足した入口だけ素の kit で動く）。
形を変える・消すときは版を上げ、アプリは 1 つ前の版も読めるようにして `SUPPORTED_KIT_API` に残す。

## kit を置く場所

次のどちらかで、kit のフォルダ（`ak_kit.py` があるフォルダ）をアプリに見つけさせる。

1. リポジトリの直下に `fx-private/kit/` を置く（`fx-private/` は `.gitignore` に入っている。別のリポジトリを clone してもよい）
2. 環境変数 `AK_FX_PRIVATE` に、kit のフォルダを指す

ビルド時に、公開の `fxkit/` とこの kit のフォルダを 1 つのフォルダに平らに並べてアプリに埋め込む（`%LOCALAPPDATA%\ak-studio\fxkit\` に書き出して使う）。
そのため、公開側と kit でモジュールの名前を重ねないこと。

演出のコードが fxkit を探す順は、次のとおり。

1. 回の `fx/` にある `fxkit.py` など（`python -m fxfreeze <回のフォルダ>` で、今の版を回に写して固定できる）
2. `python -m pip install -e fxkit --config-settings editable_mode=compat` で入れたフォルダ（kit も同じように入れられる）
3. アプリに埋め込んだ版

回の `fx/*.py` は `from fxcore import *` で芯を使う。kit に互換の入口 `fxkit.py`（`from fxcore import *` に自分の部品を足したもの）を置けば、`from fxkit import *` とも書ける。

## 見た目・声・字幕の既定（presets）

画面の色・帯の文言・字幕の既定・声の値は、kit とは別に「プリセット」のフォルダで上書きできる。
探す順は、次のうち最初に見つかったもの。

1. `AK_FX_PRIVATE` の親のフォルダの `presets/`
2. `fx-private/presets/`
3. リポジトリ直下の `presets/`

| ファイル | 中身 |
|---|---|
| `subtitle.json` | 字幕の見た目（一部の項目だけでもよい） |
| `project.json` | 既定のプロジェクトの値（背景ループの mp4・ひな形のフォルダ） |
| `voisona_presets.json` | 声の値（話者ごとの設定） |
| `ui/theme.css` | 画面の色。`:root` の値を上書きする（公開の既定はグレーと青） |
| `ui/branding.json` | 画面の上の帯の文言（`title`・`sub`） |

無い項目は素の既定で動く。読めないプリセットは捨てて、理由を起動時の知らせに出す。
優先の順は「回の設定 → ユーザーの設定 → プリセット → 素の既定」。

画面の上書き（`ui/`）は開発サーバー・ビルドのときに読む。ほかのプリセットはビルド時にアプリへ埋め込むので、直したらビルドし直す。

## 作り始めるには

1. `fxkit/ak_kit_basic.py` を `ak_kit.py` の名前でコピーして、`KIT_API = 1` のまま `NAME` を決める
2. `scene_label` と `studio_clip` から自分の絵に変える（`render_still` の確認画像や画面のモニターで見る）
3. 録画の窓を飾るなら `RecWindow` の 3 つの関数を上書きする
4. 効果音を付けるなら `se_collect`・`se_labels`・`SE_DIR` を足す
5. 画面に警告が出ていないこと（特に `kit-api`・`kit-load`）を確かめる
