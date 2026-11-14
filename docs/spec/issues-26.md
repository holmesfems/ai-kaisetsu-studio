# RecWindow に差し込んだ素材を、その RecWindow の z の位置で合成する（#26）

調べた日: 2026-11-14（main 2705c0e）

非公開のレポジトリ（ak-studio#191）に立てたものを移した issue。

## 原因

- 演出のコマは、ワーカーが生きている要素を z の順に 1 枚の RGBA に描いて返す（`engine/akfx_worker.py:719` の `frame`、`frame_bytes` :799、`op == "frame"` :1327）。Rust の側も 1 枚として受け取る（`src-tauri/src/render/engine.rs:179` の `OverlayFrame`、`parse_frame` :816、`render/cache.rs:12` の `CachedOverlay`）
- 合成（`src-tauri/src/core/compose.rs:125` の `compose_with`）は、演出のトラックの位置で `Layer::GuideAndNests` → `Layer::Fx` の順に描く（`render/layers.rs:37` の `layer_order`）。ネストの素材は `GuideAndNests` の中の `draw_nests`（compose.rs:145・:243）で、**演出の 1 枚の下**に敷かれる
- なので、RecWindow の z をどれだけ上げても、差し込んだ素材は演出のすべて（z の低いパネルも含む）の下になる。z が効くのは、演出の 1 枚の中の枠・ラベルだけ

## 直し方

### 1. ワーカー: RecWindow のところで演出のコマを分ける

- `frame(t, hide, solo, scale)` は、z の順に描いていく途中で **RecWindow の要素（`_is_rec(e)`）を描く直前** に新しい層を始める。返すのは「いちばん下の層」と「RecWindow ごとの層の列 `[(その RecWindow のクリップのキー g.key, 層の絵), …]`」
  - 同じ z の要素は今の並び（安定ソート）のまま。RecWindow より前に描かれる要素は下の層、RecWindow 自身とそのあとの要素は上の層
  - 微調整の移動（`_transform` があるクリップ）は、今どおりクリップごとまとめて 1 回で描く。そのクリップに RecWindow が入っていれば、そのまとめて描く直前で層を分ける
  - 同じクリップ（キー）で層を分けるのは 1 回だけ（1 つのクリップに RecWindow が 2 つあっても、最初の 1 つの所で分ける）
  - `solo` があるとき（クリップ 1 つのドラッグ用の見本）は分けない（今どおり 1 枚）
  - 倍率（scale）は層ごとに今と同じ縮め方をする
- 返事のヘッダ: 今の `w`・`h`・`rect`（いちばん下の層）に、`layers: [{"key": "<キー>", "rect": [x, y, 幅, 高さ] か null}, …]` を足す（RecWindow が無いコマでは付けない）。データは「いちばん下の層の切り抜き → layers の順に各層の切り抜き」をつなげたもの。各層の長さは rect から求まる（幅×高さ×4）
- `ENGINE_VERSION` はそのまま（指紋＝絵の中身は変わらない。コマのキャッシュはメモリの中だけ）

### 2. Rust: 層つきのコマを受け取り、間にネストを描く

- `OverlayFrame` に `layers: Vec<OverlayLayer>`（`key: String`・`rect: Option<[u32; 4]>`・`rgba: Vec<u8>`）を足す。`parse_frame` はヘッダの `layers` を読み、データを順に切り分ける。今の検査（rect がコマからはみ出す・長さが合わない・rect が無いのにデータがある）を層ごとにも行い、合わなければ「演出のコマの返事が壊れています（…）」で失敗にする
- `CachedOverlay` も層を持つ（各層を LZ4 で縮める）。`bytes()` は全部の層の合計
- `compose_with`:
  - 演出のトラックが見えていて `Layer::Fx` があるときは、`GuideAndNests` のところで演出のコマ（`fx_frame`）を先に 1 回だけ取っておく（取るのは 1 回。`fx_err` の扱いは今どおり）
  - `GuideAndNests` では、攻略動画のレイヤーを描いたあと、**演出のコマの層に出てこないキー** のネストだけを描く（RecWindow が隠されている・`with_fx` が false・演出のコマが失敗した・演出のトラックが隠れている、などのときは今と同じ位置に出る）
  - `Layer::Fx` では、いちばん下の層 → （層ごとに）そのキーのネストを描く → その層、の順に重ねる
  - ネストを描く処理は、キー 1 つぶんを描く関数に切り出し、`draw_nests`（全部、今の動き）と上の 2 か所から使う。`export/video.rs:508` の録画トラックの書き出しは今どおり `draw_nests`
- `clip_png` は `solo` なので層は無い（念のため、層があったら下から順に重ねて 1 枚にする）
- コメント「RecWindow に差し込んだ録画は演出の下」（compose.rs:144）を新しい動きに合わせる

### 3. クリップのキー: `add(rw)` でも RecWindow のキーにする

- ワーカーの `_label`（akfx_worker.py:193）は `add(部品名(` の形しか部品名と見ないので、`rw = RecWindow(...); add(rw)` は関数の名前（`sc_intro`）になり、キーが `sc_intro@c6#1` になる
- 直し方: `add` の中で（`ak_label` の処理の前に）、`_LABEL_RE` が当たらず、クリップの要素が **すべて RecWindow（`_is_rec`）** のときは、名前を `"RecWindow"` にする。`add(RecWindow(...))` や、ほかの部品のキーは変わらない
- これで今まで `sc_intro@c6#1` のようになっていたクリップのキーは `RecWindow@c6#1` に変わる（その書き方の回では、そのキーに付けた微調整・ネストは付け替えが要る。もともと `add(RecWindow(...))` で差し込んだネストとつながるようになる方向の変更）

### 4. 文書

- `docs/kit.md`・`docs/usage.md` などで「差し込んだ録画は演出の下」と書いている所があれば、「その RecWindow の z の位置（RecWindow より z の低い演出の上、RecWindow の枠・ラベルと z の高い演出の下）」に直す。`add(rw)` の形でもキーが `RecWindow@…` になることも書く

## テスト

- Python（`engine/` にテストを足す。`npm run test:py` で回る素の assert）:
  - パネル（Sprite、z=20）と RecWindow（z=30）とテロップ（z=40）を置いたタイムラインで `frame` を呼ぶと、いちばん下の層にパネルだけ、層 1 つ（キーが RecWindow のクリップ）に RecWindow とテロップが入る
  - RecWindow の z をパネルより下（z=10）にすると、いちばん下の層は空で、層にパネルも入る
  - RecWindow が無いコマでは層が無く、絵は今と同じ
  - `solo` では層が無い
  - `rw = RecWindow(...); add(rw)` のキーが `RecWindow@c<n>#1` になる。`add(Sprite(...))` などのキーは変わらない
  - 既存のワーカーのテストの作り方（`engine/test_*.py`）に合わせる
- Rust:
  - `parse_frame`: 層つきのヘッダ・データを正しく切り分ける／層の rect がはみ出す・長さが合わない・データが足りない／余る、を「壊れています」で失敗にする
  - `CachedOverlay::from_frame` が層を保つ
  - 合成: パネル（いちばん下の層）・ネストの素材・RecWindow の層、の順になること（純粋な関数に切り出せる所はそうしてテストする。例: 層のキーの一覧から「先に描くネストのキー」を決める関数）
- `npm run test:all` を通す

## 確かめ方

- 回の写しで、パネル（Sprite、半透明、z=20）の上に RecWindow（z=30、box がパネルの内側）を置き、`media_add` の `nest` で画像を差し込む
- `render_still` で、画像がパネルの塗りより手前（暗くならない）に出て、RecWindow の枠・ラベルは画像の上に出ること
- RecWindow の z を 10 に下げると、画像がパネルの下になる（今と同じ）こと
- 書き出し（export）の 1 コマでも同じになること
