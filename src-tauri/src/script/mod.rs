//! 台本（<回>/script/*.yaml）の読み書き。形式は docs/script-format.md（v2: YAML ＋ 行の id）。
//!
//! ```yaml
//! title: 見出し
//! notes: |-
//!   ブロックのメモ（画面に出すものなど）
//! lines:
//!   - id: l51x9k
//!     sp: トーク1
//!     start: 3.15
//!     text: ｜時雨《しぐれ》ジェシカです
//!     memo: ここで実際のPUメンバーを出す
//! ```
//! `memo` は Claude 向け（読み上げ・字幕には使わない）。`start`・`memo`・`notes` は無ければ書かない。
//! 書き出しは自前（決まった形で、差分が小さくなるように）。読むときは YAML として正しければ何でも読む。
//!
//! `_order.json`（ファイルの並び）、`_voice.json`（アクセントの手直し）、`_voice_params.json`（抑揚・長さ）は、
//! 鍵が **行の id**（`{id: {sp, text, analyzed_text|params}}`）。`_voice_speakers.json`（話者ごとの声）、
//! `_memo.md`（構成メモ）はそのまま読み書きする。上書きする前の内容は `_backup/<日時>/` に残す。
//!
//! 古い回（`.md` の表）は、開いたときに `_backup/<日時>-v2/` に写してから `.yaml` に移す（`migrate_md`）。

pub mod format;
pub mod ops;
pub mod rekey;
pub mod store;

pub use format::*;
pub use store::*;
