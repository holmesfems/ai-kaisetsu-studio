//! Claude の変更（`guide_update`）の控えと「Claude の変更を戻す」の判断（純粋な関数）。
//!
//! 保存の前の guide.json を `studio/guide_backup/` に写し、Core が「控えのパス・理由・時刻・保存した後の中身の指紋」の
//! 積み重ねを持つ。戻せるのは、今の guide.json の指紋がいちばん上の変更の保存の後と同じ（その後に人が直していない）ときだけ。

use std::path::PathBuf;

/// 控えを置く場所（回の studio/ からの相対）
pub const BACKUP_DIR: &str = "guide_backup";
/// 残す控えの数（新しいものから）
pub const BACKUP_KEEP: usize = 10;

/// Claude の変更 1 つ
#[derive(Clone, Debug, PartialEq)]
pub struct ClaudeChange {
    /// 変更の番号（回を開いている間で一意）
    pub id: u32,
    /// 保存の前の guide.json の控え
    pub backup: PathBuf,
    pub reason: String,
    /// 直した時刻（ローカル時刻。"2026-10-21T21:03:00"）
    pub at: String,
    /// 保存した後の guide.json の指紋
    pub after: u64,
}

/// 文字列の指紋（FNV-1a 64 ビット。同じ文字列なら必ず同じ）
pub fn fingerprint(text: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// 戻せる変更（積み重ねのいちばん上）を、今の guide.json の中身 current_text と突き合わせる。
/// 無ければ・後で直されていれば Err（和文）
pub fn undo_target<'a>(stack: &'a [ClaudeChange], current_text: &str) -> Result<&'a ClaudeChange, String> {
    let Some(top) = stack.last() else {
        return Err("戻せる Claude の変更がありません".into());
    };
    if fingerprint(current_text) != top.after {
        return Err(format!(
            "Claude の変更の後に直しているので戻せません（控え: {}）",
            top.backup.to_string_lossy().replace('\\', "/")
        ));
    }
    Ok(top)
}

/// 控えのファイル名。ts は "20261021-210300"。同じ秒にもう 1 つあれば `_claude_2`・`_claude_3` と続ける
pub fn backup_name(ts: &str, existing: &[String]) -> String {
    let first = format!("guide_{ts}_claude.json");
    if !existing.contains(&first) {
        return first;
    }
    (2..)
        .map(|n| format!("guide_{ts}_claude_{n}.json"))
        .find(|name| !existing.contains(name))
        .expect("無限に続く")
}

/// 控えの名前を新しい順に並べるための鍵（日時の文字列・同じ秒の連番）。控えの名前でなければ None
fn backup_key(name: &str) -> Option<(String, u32)> {
    let rest = name.strip_prefix("guide_")?.strip_suffix(".json")?;
    let (ts, tail) = rest.split_once("_claude")?;
    let n = match tail {
        "" => 1,
        t => t.strip_prefix('_')?.parse().ok()?,
    };
    Some((ts.to_string(), n))
}

/// 控えの名前の一覧から、keep 個（新しいもの）を残して消すものを返す。控えでない名前は触らない
pub fn prune_backups(names: &[String], keep: usize) -> Vec<String> {
    let mut v: Vec<((String, u32), &String)> = names.iter().filter_map(|n| backup_key(n).map(|k| (k, n))).collect();
    v.sort_by(|a, b| b.0.cmp(&a.0));
    v.into_iter().skip(keep).map(|(_, n)| n.clone()).collect()
}

/// フォルダの中のファイル名の一覧（読めなければ Err。空と取り違えると、控えの名前が重なって上書きしかねない）
fn list_names(dir: &std::path::Path) -> Result<Vec<String>, String> {
    let rd = std::fs::read_dir(dir).map_err(|e| format!("控えのフォルダ（{}）を読めませんでした: {e}", dir.display()))?;
    let mut v = vec![];
    for e in rd {
        let e = e.map_err(|e| format!("控えのフォルダ（{}）を読めませんでした: {e}", dir.display()))?;
        if let Ok(n) = e.file_name().into_string() {
            v.push(n);
        }
    }
    Ok(v)
}

/// 保存の前の guide.json の中身 old_text を、dir（`studio/guide_backup/`）に控えとして原子的に書く。書いたパスを返す。書けなければ Err（和文）
pub fn write_backup(dir: &std::path::Path, ts: &str, old_text: &str) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("控えのフォルダ（{}）を作れませんでした: {e}", dir.display()))?;
    let path = dir.join(backup_name(ts, &list_names(dir)?));
    crate::settings::write_atomic(&path, old_text).map_err(|e| format!("控え（{}）を作れませんでした: {e}", path.display()))?;
    Ok(path)
}

/// 古い控えを消して、新しい keep 個を残す。消せなかったものの (名前, 理由) を返す（保存は済んでいるので、呼ぶ側は知らせるだけ）。
/// フォルダを読めなければ Err
pub fn prune_dir(dir: &std::path::Path, keep: usize) -> Result<Vec<(String, String)>, String> {
    Ok(prune_backups(&list_names(dir)?, keep)
        .into_iter()
        .filter_map(|n| std::fs::remove_file(dir.join(&n)).err().map(|e| (n, e.to_string())))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 控えを 12 回書くと、新しい 10 個だけが残る。同じ秒の控えは上書きしない
    #[test]
    fn write_backup_keeps_latest_ten_and_never_overwrites() {
        let dir = std::env::temp_dir().join(format!("ak-backup-test-{}", std::process::id())).join("guide_backup");
        let _ = std::fs::remove_dir_all(&dir);
        for i in 0..12 {
            let p = write_backup(&dir, &format!("20261021-2100{i:02}"), &format!("v{i}")).unwrap();
            assert_eq!(std::fs::read_to_string(p).unwrap(), format!("v{i}"));
        }
        let a = write_backup(&dir, "20261021-210011", "同じ秒").unwrap();
        assert!(a.to_string_lossy().ends_with("_claude_2.json"), "{a:?}");
        assert!(prune_dir(&dir, BACKUP_KEEP).unwrap().is_empty());
        let mut left = list_names(&dir).unwrap();
        left.sort();
        assert_eq!(left.len(), 10);
        // いちばん古い 3 つ（00・01・02）が消え、同じ秒の 2 つ目は残る
        assert_eq!(left[0], "guide_20261021-210003_claude.json");
        assert!(left.contains(&"guide_20261021-210011_claude_2.json".to_string()));
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }

    /// フォルダを読めないときは、空と取り違えず Err（控えの名前が重なって上書きしないように）
    #[test]
    fn unreadable_backup_dir_is_an_error_not_empty() {
        let dir = std::env::temp_dir().join(format!("ak-backup-missing-{}", std::process::id())).join("none");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(list_names(&dir).unwrap_err().contains("読めませんでした"));
        assert!(prune_dir(&dir, BACKUP_KEEP).is_err());
    }

    fn change(after_text: &str, backup: &str) -> ClaudeChange {
        ClaudeChange { id: 1, backup: backup.into(), reason: "r".into(), at: "2026-10-21T21:03:00".into(), after: fingerprint(after_text) }
    }

    #[test]
    fn undo_only_when_fingerprint_matches() {
        let stack = vec![change("A", "b1"), change("B", "b2")];
        // いちばん上（B の保存の後）と同じなら戻せる
        assert_eq!(undo_target(&stack, "B").unwrap().backup, PathBuf::from("b2"));
        // 後で人が直していたら断る。控えのパスを添える
        let e = undo_target(&stack, "B2").unwrap_err();
        assert!(e.contains("直しているので戻せません") && e.contains("b2"), "{e}");
        // 積み重ねが空
        assert!(undo_target(&[], "B").unwrap_err().contains("ありません"));
    }

    #[test]
    fn undo_stack_unwinds_in_order() {
        // A → (Claude 1) → B → (Claude 2) → C。C のうちに戻せるのは 2 つ目、戻したら B の中身になり 1 つ目が戻せる
        let mut stack = vec![change("B", "before1"), change("C", "before2")];
        assert!(undo_target(&stack, "B").is_err());
        assert_eq!(undo_target(&stack, "C").unwrap().backup, PathBuf::from("before2"));
        stack.pop();
        assert_eq!(undo_target(&stack, "B").unwrap().backup, PathBuf::from("before1"));
    }

    #[test]
    fn fingerprint_is_stable_and_sensitive() {
        assert_eq!(fingerprint("abc"), fingerprint("abc"));
        assert_ne!(fingerprint("abc"), fingerprint("abd"));
        assert_ne!(fingerprint(""), fingerprint(" "));
    }

    #[test]
    fn backup_names_do_not_collide() {
        let ts = "20261021-210300";
        assert_eq!(backup_name(ts, &[]), "guide_20261021-210300_claude.json");
        let ex = vec!["guide_20261021-210300_claude.json".to_string()];
        assert_eq!(backup_name(ts, &ex), "guide_20261021-210300_claude_2.json");
        let ex = vec![ex[0].clone(), "guide_20261021-210300_claude_2.json".to_string()];
        assert_eq!(backup_name(ts, &ex), "guide_20261021-210300_claude_3.json");
    }

    #[test]
    fn prune_keeps_newest_ten_and_ignores_other_files() {
        let mut names: Vec<String> = (0..12).map(|i| format!("guide_20261021-2100{i:02}_claude.json")).collect();
        names.push("guide_20261021-210011_claude_2.json".into());
        names.push("memo.txt".into());
        names.push("guide_20250101-000000_manual.json".into());
        let del = prune_backups(&names, BACKUP_KEEP);
        // 13 個の控えのうち古い 3 つ
        assert_eq!(del.len(), 3);
        assert!(del.contains(&"guide_20261021-210000_claude.json".to_string()));
        assert!(del.contains(&"guide_20261021-210001_claude.json".to_string()));
        assert!(del.contains(&"guide_20261021-210002_claude.json".to_string()));
        // 同じ秒の連番は新しいほうが残る
        assert!(!del.contains(&"guide_20261021-210011_claude_2.json".to_string()));
        assert!(prune_backups(&names[..5], 10).is_empty());
    }
}
