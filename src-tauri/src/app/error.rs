//! 失敗に種類（`ErrorCode`）を付けて画面・HTTP に返す。
//!
//! - 中では今までどおり anyhow。コードを付けたい所だけ `coded(...)` で作る
//! - 返すときに `api_error` が連鎖を見て分類し、和文の `message` と元の連鎖の `detail` にする
//! - 画面へのコマンドの失敗は `CmdError`（`ApiError` の形のまま JSON になる）

use ak_proto::{ApiError, ErrorCode};
use serde::Serialize;
use std::fmt;

/// コードを付ける印。anyhow の連鎖の中から `api_error` が見つける
#[derive(Debug)]
pub struct Coded {
    pub code: ErrorCode,
    pub msg: String,
    /// 元の連鎖など、`detail` に入れる文（無ければ `msg` がそのまま入る）
    pub detail: Option<String>,
    /// `PartlySaved` で、あとの失敗の元の種類
    pub inner: Option<ErrorCode>,
}

impl fmt::Display for Coded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for Coded {}

/// コードつきの失敗を作る（`return Err(coded(ErrorCode::BadInput, "…"))`）
pub fn coded(code: ErrorCode, msg: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(Coded { code, msg: msg.into(), detail: None, inner: None })
}

/// コードつきの失敗に、元の原因（英語を含んでよい）を `detail` として残す
pub fn coded_detail(code: ErrorCode, msg: impl Into<String>, detail: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(Coded { code, msg: msg.into(), detail: Some(detail.into()), inner: None })
}

/// 保存はできたが、あとの作り直しだけ失敗した。文の後ろは `cause` の和文、`detail` は元の連鎖、
/// 元の種類（ffmpeg なし・演出ワーカー停止など）は `cause` として画面に渡す
pub fn partly_saved(head: &str, cause: &anyhow::Error) -> anyhow::Error {
    let a = api_error(cause);
    let inner = (a.code != ErrorCode::Failed).then_some(a.code);
    anyhow::Error::new(Coded {
        code: ErrorCode::PartlySaved,
        msg: format!("{head}: {}", a.message),
        detail: Some(format!("{head}: {}", a.detail)),
        inner,
    })
}

/// 渡した値が正しくないときの失敗
pub fn bad_input(msg: impl Into<String>) -> anyhow::Error {
    coded(ErrorCode::BadInput, msg)
}

/// ファイル・行・クリップなどが無いときの失敗
pub fn not_found(msg: impl Into<String>) -> anyhow::Error {
    coded(ErrorCode::NotFound, msg)
}

/// 回を開いていないときの失敗（`msg::NO_PROJECT` に `NoProject` を付けたもの）
pub fn no_project() -> anyhow::Error {
    coded(ErrorCode::NoProject, crate::msg::NO_PROJECT)
}

/// 連鎖の 1 つ分を和文にする。serde の英語は書式の案内に、io の `(os error N)` は削る
fn link_text(c: &(dyn std::error::Error + 'static)) -> String {
    if let Some(j) = c.downcast_ref::<serde_json::Error>() {
        return bad_format(j.line(), j.column());
    }
    if let Some(y) = c.downcast_ref::<serde_yaml_ng::Error>() {
        return match y.location() {
            Some(l) => bad_format(l.line(), l.column()),
            None => "書式が正しくありません".to_string(),
        };
    }
    if let Some(n) = c.downcast_ref::<notify::Error>() {
        return notify_text(n);
    }
    let s = c.to_string();
    if c.downcast_ref::<std::io::Error>().is_some() {
        return strip_os_error(&s);
    }
    s
}

/// ファイルの見張り（notify）の失敗。英語の決まり文句は和文に、io の `(os error N)` は削る
/// （notify が付ける場所は英語の文の中にあるので落とす。場所は呼ぶ側が文の頭に書く）
fn notify_text(n: &notify::Error) -> String {
    use notify::ErrorKind as K;
    match &n.kind {
        K::Io(io) => strip_os_error(&io.to_string()),
        K::PathNotFound => "場所が見つかりません".to_string(),
        K::WatchNotFound => "見張りが見つかりません".to_string(),
        K::MaxFilesWatch => "OS のファイルの見張りの数の上限に達しました".to_string(),
        _ => n.to_string(),
    }
}

fn bad_format(line: usize, col: usize) -> String {
    if line == 0 {
        // from_value など、文字の位置が無い読み込み（版が違う・項目が足りない）
        "中身の形が合いません（版が違うかもしれません）".to_string()
    } else {
        format!("書式が正しくありません（行 {line}、列 {col}）")
    }
}

/// 「…（末尾）(os error 2)」の `(os error N)` を取り除く
fn strip_os_error(s: &str) -> String {
    match s.rfind(" (os error ") {
        Some(i) if s.ends_with(')') => s[..i].to_string(),
        _ => s.to_string(),
    }
}

/// anyhow の連鎖を画面・HTTP 向けの `ApiError` にする。
/// コードは、連鎖で最初の `Coded` → io の kind → serde の書式 → `Failed` の順に決める
pub fn api_error(e: &anyhow::Error) -> ApiError {
    let mut code = e.chain().find_map(|c| c.downcast_ref::<Coded>().map(|k| k.code));
    if code.is_none() {
        code = e.chain().find_map(|c| {
            let io = c.downcast_ref::<std::io::Error>()?;
            match io.kind() {
                std::io::ErrorKind::NotFound => Some(ErrorCode::NotFound),
                std::io::ErrorKind::PermissionDenied => Some(ErrorCode::Denied),
                _ => None,
            }
        });
    }
    if code.is_none() {
        let serde = e.chain().any(|c| c.is::<serde_json::Error>() || c.is::<serde_yaml_ng::Error>());
        if serde {
            code = Some(ErrorCode::BadFormat);
        }
    }
    let message = e.chain().map(link_text).collect::<Vec<_>>().join(": ");
    let first = e.chain().find_map(|c| c.downcast_ref::<Coded>());
    let cause = first.and_then(|k| k.inner);
    // Coded に detail があるときは、その文を連鎖の中の表示の代わりに使う
    let detail = if e.chain().any(|c| c.downcast_ref::<Coded>().is_some_and(|k| k.detail.is_some())) {
        e.chain()
            .map(|c| match c.downcast_ref::<Coded>().and_then(|k| k.detail.clone()) {
                Some(d) => d,
                None => c.to_string(),
            })
            .collect::<Vec<_>>()
            .join(": ")
    } else {
        format!("{e:#}")
    };
    ApiError { code: code.unwrap_or(ErrorCode::Failed), message, detail, cause }
}

/// Rust から出す知らせの文（「{head}: 理由」）と、ログ・ツールチップに残す元の連鎖。
/// 理由は `api_error` の和文（serde の英語・`(os error N)` を含まない）。head が空なら理由だけ
pub fn notice_text(head: &str, e: &anyhow::Error) -> (String, String) {
    let a = api_error(e);
    if head.is_empty() {
        (a.message, a.detail)
    } else {
        (format!("{head}: {}", a.message), format!("{head}: {}", a.detail))
    }
}

/// 連鎖を持たない 1 つの失敗（serde・io・notify）の理由だけを和文にする（`api_error` の message と同じ削り方）。
/// 知らせの文の中に `{e}` で埋め込む代わりに使う
pub fn reason(e: impl Into<anyhow::Error>) -> String {
    api_error(&e.into()).message
}

/// コマンドの失敗。JSON にすると `ApiError` そのまま
#[derive(Debug, Serialize)]
#[serde(transparent)]
pub struct CmdError(pub ApiError);

impl From<anyhow::Error> for CmdError {
    fn from(e: anyhow::Error) -> Self {
        CmdError(api_error(&e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;

    #[test]
    fn io_not_found_with_context() {
        let io = std::io::Error::from_raw_os_error(2);
        let e = anyhow::Error::new(io).context("台本を読めません");
        let a = api_error(&e);
        assert_eq!(a.code, ErrorCode::NotFound);
        assert!(a.message.starts_with("台本を読めません: "), "{}", a.message);
        assert!(!a.message.contains("(os error"), "{}", a.message);
        assert!(a.detail.contains("(os error 2)"), "{}", a.detail);
    }

    #[test]
    fn io_denied() {
        let e = anyhow::Error::new(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        assert_eq!(api_error(&e).code, ErrorCode::Denied);
    }

    #[test]
    fn serde_json_is_bad_format_in_japanese() {
        let je = serde_json::from_str::<serde_json::Value>("{\n  \"a\": }").unwrap_err();
        let e = anyhow::Error::new(je).context("設定を読めません");
        let a = api_error(&e);
        assert_eq!(a.code, ErrorCode::BadFormat);
        assert!(!a.message.contains("expected"), "{}", a.message);
        assert!(a.message.contains('行') && a.message.contains('列'), "{}", a.message);
        assert!(a.detail.contains("expected"), "{}", a.detail);
    }

    #[test]
    fn serde_yaml_is_bad_format() {
        let ye = serde_yaml_ng::from_str::<serde_yaml_ng::Value>("a: : b\n  - x").unwrap_err();
        let a = api_error(&anyhow::Error::new(ye));
        assert_eq!(a.code, ErrorCode::BadFormat);
        assert!(a.message.contains("書式が正しくありません"), "{}", a.message);
    }

    #[test]
    fn coded_survives_contexts() {
        let e: anyhow::Result<()> = Err(coded(ErrorCode::NeedFfmpeg, "ffmpeg が見つかりません"));
        let e = e.context("書き出し").context("動画").unwrap_err();
        let a = api_error(&e);
        assert_eq!(a.code, ErrorCode::NeedFfmpeg);
        assert_eq!(a.message, "動画: 書き出し: ffmpeg が見つかりません");
    }

    #[test]
    fn plain_is_failed() {
        let a = api_error(&anyhow::anyhow!("だめでした"));
        assert_eq!(a.code, ErrorCode::Failed);
        assert_eq!(a.message, "だめでした");
    }

    #[test]
    fn cmd_error_json_shape() {
        let c: CmdError = coded(ErrorCode::BadInput, "値が違います").into();
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v, serde_json::json!({"code": "bad_input", "message": "値が違います", "detail": "値が違います"}));
    }

    #[test]
    fn bad_format_without_position() {
        // from_value のように文字の位置が無い serde の失敗
        let je = serde_json::from_value::<Vec<u32>>(serde_json::json!({"a": 1})).unwrap_err();
        assert_eq!(je.line(), 0);
        let a = api_error(&anyhow::Error::new(je));
        assert_eq!(a.code, ErrorCode::BadFormat);
        assert_eq!(a.message, "中身の形が合いません（版が違うかもしれません）");
        assert!(a.detail.contains("invalid type"), "{}", a.detail);
    }

    #[test]
    fn coded_detail_keeps_cause_in_detail() {
        let e = coded_detail(ErrorCode::VoiceDown, "中継に接続できません", "connection refused (os error 10061)");
        let a = api_error(&e.context("辞書を読み込めませんでした"));
        assert_eq!(a.code, ErrorCode::VoiceDown);
        assert_eq!(a.message, "辞書を読み込めませんでした: 中継に接続できません");
        assert!(a.detail.contains("connection refused"), "{}", a.detail);
        assert_eq!(a.cause, None);
    }

    #[test]
    fn notice_text_is_japanese_and_keeps_chain_in_detail() {
        let io = std::io::Error::from_raw_os_error(2);
        let e = anyhow::Error::new(io).context("台本を読めません");
        let (t, d) = notice_text("外で変わった台本を読み込めませんでした", &e);
        assert!(t.starts_with("外で変わった台本を読み込めませんでした: 台本を読めません: "), "{t}");
        assert!(!t.contains("(os error"), "{t}");
        assert!(d.contains("(os error 2)"), "{d}");

        let je = serde_json::from_str::<serde_json::Value>("{
  \"a\": }").unwrap_err();
        let (t, d) = notice_text("", &anyhow::Error::new(je).context("設定を読めません"));
        assert!(t.starts_with("設定を読めません: 書式が正しくありません（行 2、"), "{t}");
        assert!(!t.contains("expected"), "{t}");
        assert!(d.contains("expected"), "{d}");
    }

    #[test]
    fn reason_strips_os_error_and_serde_english() {
        let r = reason(std::io::Error::from_raw_os_error(5));
        assert!(!r.contains("(os error"), "{r}");
        let je = serde_json::from_str::<serde_json::Value>("[1,").unwrap_err();
        assert!(reason(je).starts_with("書式が正しくありません（行 1、"));
    }

    #[test]
    fn notify_error_in_japanese() {
        let n = notify::Error::io(std::io::Error::from_raw_os_error(3)).add_path("E:/x/fx".into());
        let (t, d) = notice_text("見張れませんでした", &anyhow::Error::new(n));
        assert!(t.starts_with("見張れませんでした: "), "{t}");
        assert!(!t.contains("(os error"), "{t}");
        assert!(d.contains("(os error 3)"), "{d}");
        let (t, _) = notice_text("", &anyhow::Error::new(notify::Error::new(notify::ErrorKind::MaxFilesWatch)));
        assert_eq!(t, "OS のファイルの見張りの数の上限に達しました");
    }

    #[test]
    fn partly_saved_keeps_inner_code_and_cleans_message() {
        let io = std::io::Error::from_raw_os_error(2);
        let inner = anyhow::Error::new(io).context("ffmpeg を起動できませんでした");
        let a = api_error(&partly_saved("設定は保存しましたが、作り直せませんでした", &inner));
        assert_eq!(a.code, ErrorCode::PartlySaved);
        assert_eq!(a.cause, Some(ErrorCode::NotFound));
        assert!(a.message.starts_with("設定は保存しましたが、作り直せませんでした: ffmpeg を起動できませんでした"), "{}", a.message);
        assert!(!a.message.contains("(os error"), "{}", a.message);
        assert!(a.detail.contains("(os error 2)"), "{}", a.detail);

        let inner = coded(ErrorCode::NeedFfmpeg, "ffmpeg が見つかりません");
        let a = api_error(&partly_saved("保存しましたが、作り直せませんでした", &inner));
        assert_eq!(a.cause, Some(ErrorCode::NeedFfmpeg));
        assert_eq!(a.message, "保存しましたが、作り直せませんでした: ffmpeg が見つかりません");
        // 種類の無い失敗は cause なし
        assert_eq!(api_error(&partly_saved("x", &anyhow::anyhow!("だめ"))).cause, None);
    }
}
