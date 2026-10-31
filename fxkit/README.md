# fxkit

AK Studio の演出のエンジンと部品。各回の `fx/<名前>_fx.py` が `from fxkit import *` で使う。

## 準備

```
python -m pip install -e fxkit --config-settings editable_mode=compat
python -m pip install -e "fxkit[sync,track]" --config-settings editable_mode=compat   # 同期点（faster-whisper）・録画の追跡（OpenCV）も使うなら
```

`editable_mode=compat` が要る。既定の editable は import フックで入るため、アプリが `sys.path` の末尾に足す埋め込みの版より後に探されてしまう。
入ったかは `python -c "import fxkit; print(fxkit.__file__)"`（このフォルダを指せばよい）。

## どの fxkit が使われるか

1. 回の `fx/` にある `fxkit.py` など（古い回はこれ。そのときの版のまま描ける）
2. `pip install -e` したこのフォルダ
3. アプリに埋め込んだ版（`%LOCALAPPDATA%\ak-studio\fxkit\`。Python に fxkit を入れていない PC 用）

アプリの「演出エンジン」の画面に、今使っている fxkit の場所が出る。
回を今の版に固定する（1. を作る）: `python -m fxfreeze <回のフォルダ>`（すでに違うものがあれば `--force`。控えは `fx/_fxkit_backup_<日時>/`）。

## ファイル

| 分類 | ファイル |
|---|---|
| 芯 | `fxcore.py`（`Timeline`・`El`・`Sprite`・入りと抜けの動き・`put`・`RecWindowBase` など）・`fxse.py`（効果音の仕組み: bank.json の読み書き・合成・書き出し） |
| 素の kit | `ak_kit_basic.py`（kit が無いときに使う、地味な見た目の版） |
| 道具 | `align_sync.py`（同期点）・`rec_track.py`（録画の追跡）・`fxfreeze.py` |
| 素材 | `fonts/`（ライセンス文つき。[THIRD_PARTY.md](../THIRD_PARTY.md)） |

見た目を足す kit の作り方は [docs/kit.md](../docs/kit.md)。kit のフォルダのファイルも、アプリへの埋め込みでは公開側と平らに並ぶ（モジュールの名前は重ねない）。
ファイルを足したら `src-tauri/src/settings/embed.rs` の対応と `pyproject.toml` の `py-modules` も見る（表のずれは `cargo test` で落ちる）。
