//! 台本フォルダ（<回>/script/）の読み書き。外の変更を黙って消さずに保存する。

use anyhow::{anyhow, Context, Result};
use regex::Regex;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use super::format::*;

// ---------------------------------------------------------------------------------------------
// 台本フォルダ

const VOICE: &str = "_voice.json";
const PARAMS: &str = "_voice_params.json";
/// 話者の声の設定ファイル（VoiSona = 組み込みの既定。ほかのエンジンは `speakers_file` で別のファイルにする）
const SPEAKERS: &str = "_voice_speakers.json";

/// 声のエンジン（設定の `voice_engine`）の、話者ごとの声の設定のファイル名。
/// 声の単位がエンジンで違う（VoiSona の音量 dB と VOICEVOX の倍率など）ので、エンジンごとに別のファイルに持つ。
/// voisona は `_voice_speakers.json`（今までのまま）、それ以外は `_voice_speakers.<名前>.json`
/// （名前は組み込みならその名前、.py のパスならファイル名の拡張子を除いた部分。英数字・`_`・`-` 以外は落とし、空なら custom）。
/// voice_server.py の `speakers_file` と同じ規則
pub fn speakers_file(voice_engine: &str) -> String {
    let v = voice_engine.trim();
    if v == "voisona" {
        return SPEAKERS.to_string();
    }
    let last = v.replace('\\', "/");
    let last = last.rsplit('/').next().unwrap_or("");
    let stem = if last.to_lowercase().ends_with(".py") { &last[..last.len() - 3] } else { last };
    let clean: String = stem.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').collect();
    format!("_voice_speakers.{}.json", if clean.is_empty() { "custom" } else { clean.as_str() })
}
const MEMO: &str = "_memo.md";
const ORDER: &str = "_order.json";

/// 画面に出す知らせ（移行したこと・警告）。core が toast にする
#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub text: String,
    pub error: bool,
}

/// 台本フォルダ。読み込んだ時点の各ファイルの中身を覚えておき、変わった所だけ書く。
pub struct ScriptStore {
    pub dir: PathBuf,
    pub script: Script,
    /// 読み込んだ（書いた）ときのファイルの中身（外での変更を見つけるため）
    loaded: HashMap<String, String>,
    /// 読み込んだ中身を、決まった形で書き直したもの。画面の内容がこれと同じなら、人が手で整えたファイルは書き換えない。
    /// id を付けたファイルは無い（次の保存で書いて、id を残す）
    canon: HashMap<String, String>,
    loaded_side: HashMap<String, String>,
    /// 話者の声の設定のファイル名（エンジンごと。`speakers_file`）
    speakers_file: String,
    session: String,
    backed_up: HashSet<String>,
    /// 直前の save で、外（Claude・voisona-accent）が書き換えたファイルの中身を残して読み直したか
    pub merged_external: bool,
    notices: Vec<Notice>,
    warned: HashSet<String>,
    /// 最近の rev ごとの台本（古い rev をもとに来た画面の保存を、3 者マージするため）
    history: std::collections::VecDeque<Script>,
}

const HISTORY: usize = 20;

/// 付属ファイル name の、台本から見た中身
fn side_of(s: &Script, name: &str) -> String {
    let obj = |m: &serde_json::Map<String, Value>| serde_json::to_string(&Value::Object(m.clone())).unwrap_or_default();
    match name {
        MEMO => s.memo.clone(),
        VOICE => obj(&s.voice),
        PARAMS => obj(&s.params),
        _ => obj(&s.speakers.clone().unwrap_or_default()),
    }
}

/// 付属の JSON（アクセントの手直し・抑揚・話者の声）を読む。無い・空なら空の Map。
/// 壊れていたら空として読まない（次の保存で {} を書いて手直しが消えるため）。書きかけのこともあるので、何も変えずにエラーにする
fn read_side_json(dir: &Path, name: &str) -> Result<(String, serde_json::Map<String, Value>)> {
    let t = read_opt(&dir.join(name)).unwrap_or_else(|| "{}".into());
    let v: serde_json::Map<String, Value> = if t.trim().is_empty() {
        Default::default()
    } else {
        serde_json::from_str(&t).with_context(|| format!("{name} が読めません（書きかけかもしれません）"))?
    };
    Ok((t, v))
}

fn read_opt(p: &Path) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim_start_matches('\u{feff}').to_string())
}

fn is_yaml(n: &str) -> bool {
    n.to_lowercase().ends_with(".yaml") && !n.starts_with('_')
}

fn is_md(n: &str) -> bool {
    n.to_lowercase().ends_with(".md") && !n.starts_with('_')
}

fn yaml_name(md: &str) -> String {
    format!("{}.yaml", &md[..md.len() - 3])
}

/// 台本フォルダの中の (yaml のファイル名, md のファイル名)
fn scan(dir: &Path) -> Result<(Vec<String>, Vec<String>)> {
    let (mut y, mut m) = (vec![], vec![]);
    for e in std::fs::read_dir(dir).context("台本フォルダを読めません")? {
        let e = e?;
        let name = e.file_name().to_string_lossy().to_string();
        if !e.file_type()?.is_file() {
            continue;
        }
        if is_yaml(&name) {
            y.push(name);
        } else if is_md(&name) {
            m.push(name);
        }
    }
    Ok((y, m))
}

/// 移せる古い台本（同じ名前の .yaml が無い .md）と、両方あるもの（.md を残す）
fn md_todo(dir: &Path) -> Result<(Vec<String>, Vec<String>)> {
    let (y, m) = scan(dir)?;
    let ys: HashSet<String> = y.into_iter().collect();
    let (both, todo): (Vec<String>, Vec<String>) = m.into_iter().partition(|n| ys.contains(&yaml_name(n)));
    Ok((todo, both))
}

impl ScriptStore {
    pub fn open(dir: &Path) -> Result<Self> {
        Self::open_with(dir, SPEAKERS)
    }

    /// 話者の声の設定を speakers_file（`speakers_file()` の返り）から読む形で開く
    pub fn open_with(dir: &Path, speakers_file: &str) -> Result<Self> {
        std::fs::create_dir_all(dir).ok();
        let mut st = ScriptStore {
            dir: dir.to_path_buf(),
            script: Script::default(),
            loaded: HashMap::new(),
            canon: HashMap::new(),
            loaded_side: HashMap::new(),
            speakers_file: speakers_file.to_string(),
            session: chrono::Local::now().format("%Y%m%d-%H%M%S").to_string(),
            backed_up: HashSet::new(),
            merged_external: false,
            notices: vec![],
            warned: HashSet::new(),
            history: Default::default(),
        };
        st.reload(None)?;
        Ok(st)
    }

    /// 画面に知らせることを取り出す（取り出すと空になる）
    pub fn take_notices(&mut self) -> Vec<Notice> {
        std::mem::take(&mut self.notices)
    }

    /// 古い台本（.md）を .yaml に移す（docs/script-format.md §3 の 1・2・4）。声の手直しの鍵の移しは reload の中。
    /// 移す前に控えを写せなければ、何も変えずにエラー。途中で失敗したら、書いた .yaml は消して元のまま
    fn migrate_md(&mut self) -> Result<()> {
        let (todo, both) = md_todo(&self.dir)?;
        for n in both {
            if self.warned.insert(n.clone()) {
                self.notices.push(Notice {
                    text: format!("{n} と {} の両方があるので、{} を使います（{n} は触りません）", yaml_name(&n), yaml_name(&n)),
                    error: true,
                });
            }
        }
        if todo.is_empty() {
            return Ok(());
        }
        let stamp = format!("{}-v2", chrono::Local::now().format("%Y%m%d-%H%M%S"));
        let bdir = self.dir.join("_backup").join(&stamp);
        std::fs::create_dir_all(&bdir).context("台本の控えの場所を作れないので、新しい形式への移行をやめました")?;
        for n in todo.iter().map(|s| s.as_str()).chain([ORDER, VOICE, PARAMS]) {
            let src = self.dir.join(n);
            if src.is_file() {
                std::fs::copy(&src, bdir.join(n))
                    .with_context(|| format!("{n} の控えを写せないので、新しい形式への移行をやめました"))?;
                // 声の手直しの鍵を直すときの控えは、これで足りる
                self.backed_up.insert(n.to_string());
            }
        }
        let mut written: Vec<PathBuf> = vec![];
        let res = (|| -> Result<()> {
            let mut seen: HashSet<String> = HashSet::new();
            for n in &todo {
                let text = read_opt(&self.dir.join(n)).ok_or_else(|| anyhow!("{n} を読めません"))?;
                let mut p = parse_md(&text);
                for l in p.lines.iter_mut() {
                    while !seen.insert(l.id.clone()) {
                        l.id = new_id("l");
                    }
                }
                let b = Block {
                    id: String::new(),
                    file: None,
                    title: p.title.unwrap_or_else(|| n[..n.len() - 3].to_string()),
                    lines: p.lines,
                    notes: p.notes,
                };
                let to = self.dir.join(yaml_name(n));
                crate::settings::write_atomic(&to, to_yaml(&b))?;
                written.push(to);
            }
            // _order.json の .md を .yaml に
            if let Some(t) = read_opt(&self.dir.join(ORDER)) {
                if let Ok(order) = serde_json::from_str::<Vec<String>>(&t) {
                    let order: Vec<String> =
                        order.into_iter().map(|o| if todo.contains(&o) { yaml_name(&o) } else { o }).collect();
                    crate::settings::write_atomic(&self.dir.join(ORDER), serde_json::to_string_pretty(&order)?)?;
                }
            }
            Ok(())
        })();
        if let Err(e) = res {
            for p in written {
                let _ = std::fs::remove_file(p);
            }
            return Err(e);
        }
        for n in &todo {
            let _ = std::fs::remove_file(self.dir.join(n));
        }
        self.notices.push(Notice {
            text: format!("台本を新しい形式（YAML）に移しました。前のファイルは script/_backup/{stamp}/ にあります"),
            error: false,
        });
        Ok(())
    }

    /// 古い形（鍵が 話者+\u0001+読み上げの文）の手直しを、行の id の鍵に直して書く。合う行すべてに同じ手直しを移し、
    /// どの行にも合わないものは捨てる（控えには残る）。古い鍵が無ければ何もしない
    fn convert_legacy(
        &mut self,
        name: &'static str,
        text: String,
        map: serde_json::Map<String, Value>,
        blocks: &[Block],
        field: &str,
    ) -> Result<(String, serde_json::Map<String, Value>)> {
        if !map.keys().any(|k| k.contains('\u{1}')) {
            return Ok((text, map));
        }
        let mut by_key: HashMap<String, Vec<&Line>> = HashMap::new();
        for l in blocks.iter().flat_map(|b| b.lines.iter()) {
            by_key.entry(vkey(&l.sp, &l.text)).or_default().push(l);
        }
        let mut out = serde_json::Map::new();
        for (k, v) in map.iter().filter(|(k, _)| !k.contains('\u{1}')) {
            out.insert(k.clone(), v.clone());
        }
        for (k, v) in map.iter().filter(|(k, _)| k.contains('\u{1}')) {
            let (sp, t) = k.split_once('\u{1}').unwrap_or(("", ""));
            for l in by_key.get(k).into_iter().flatten() {
                out.entry(l.id.clone()).or_insert_with(|| serde_json::json!({"sp": sp, "text": t, field: v}));
            }
        }
        let s = serde_json::to_string(&Value::Object(out.clone()))?;
        self.backup(name, &text).with_context(|| format!("{name} の声の手直しを新しい形式に移せませんでした（控えを残せないため）"))?;
        crate::settings::write_atomic(&self.dir.join(name), &s)?;
        Ok((s, out))
    }

    /// ディスクから読み直す。id はファイルに書いてあるものが優先。id の無い行・重なる行には新しい id を付ける
    /// （prev があれば、同じファイルで話者・文が同じ行の前の id を使う）。
    pub fn reload(&mut self, prev: Option<&Script>) -> Result<()> {
        // アクセントの手直し・抑揚（voisona-accent スキルが書く）は、壊れていたら空として読まない。
        // 空として読むと、次の保存で {} を書いて手直しが消える。書きかけのこともあるので、何も変えずにエラーにする
        let side_json = |name: &str| read_side_json(&self.dir, name);
        let (voice, voice_v) = side_json(VOICE)?;
        let (params, params_v) = side_json(PARAMS)?;
        let (speakers, speakers_v) = side_json(&self.speakers_file)?;
        self.migrate_md()?;
        let (mut files, _) = scan(&self.dir)?;
        // 順番（古い .md の名前で書いてあっても .yaml として扱う）
        let order: Vec<String> = read_opt(&self.dir.join(ORDER))
            .and_then(|t| serde_json::from_str::<Vec<String>>(&t).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|o| if is_md(&o) { yaml_name(&o) } else { o })
            .collect();
        let rank = |f: &String| order.iter().position(|o| o == f).unwrap_or(usize::MAX);
        files.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| natural_cmp(a, b)));
        // 全部読んでから組み立てる（1 つでも読めなければ、何も変えずにエラー。壊れたファイルを空と見て上書きしないため）
        let mut raws = vec![];
        for f in files {
            let text = read_opt(&self.dir.join(&f)).ok_or_else(|| anyhow!("台本 {f} を読めません"))?;
            let r = parse_yaml(&text, &f)?;
            raws.push((f, text, r));
        }
        // id: 最初に出てきたものが優先。無い・重なるものは、あとで付ける
        let mut seen: HashSet<String> = HashSet::new();
        let ok: Vec<Vec<bool>> = raws
            .iter()
            .map(|(_, _, r)| r.lines.iter().map(|l| l.id.as_ref().is_some_and(|id| seen.insert(id.clone()))).collect())
            .collect();
        let mut blocks = vec![];
        let mut loaded = HashMap::new();
        let mut canon = HashMap::new();
        for ((f, text, r), oks) in raws.into_iter().zip(ok) {
            let pb = prev.and_then(|p| p.blocks.iter().find(|x| x.file.as_deref() == Some(f.as_str())));
            let mut gen = false;
            let mut lines = vec![];
            for (rl, good) in r.lines.into_iter().zip(oks) {
                let id = if good {
                    rl.id.unwrap_or_default()
                } else {
                    gen = true;
                    let carried = pb.and_then(|pb| {
                        pb.lines
                            .iter()
                            .find(|p| p.sp == rl.sp && p.text == rl.text && !seen.contains(&p.id))
                            .map(|p| p.id.clone())
                    });
                    let id = carried.unwrap_or_else(|| loop {
                        let i = new_id("l");
                        if !seen.contains(&i) {
                            break i;
                        }
                    });
                    seen.insert(id.clone());
                    id
                };
                lines.push(Line { id, sp: rl.sp, text: rl.text, start: rl.start, memo: rl.memo });
            }
            let b = Block {
                id: pb.map(|p| p.id.clone()).unwrap_or_else(|| new_id("b")),
                title: r.title.unwrap_or_else(|| f.trim_end_matches(".yaml").to_string()),
                file: Some(f.clone()),
                lines,
                notes: r.notes,
            };
            if !gen {
                canon.insert(f.clone(), to_yaml(&b));
            }
            loaded.insert(f, text);
            blocks.push(b);
        }
        let (voice, voice_v) = self.convert_legacy(VOICE, voice, voice_v, &blocks, "analyzed_text")?;
        let (params, params_v) = self.convert_legacy(PARAMS, params, params_v, &blocks, "params")?;
        let memo = read_opt(&self.dir.join(MEMO)).unwrap_or_default();
        self.loaded = loaded;
        self.canon = canon;
        self.loaded_side.insert(MEMO.to_string(), memo.clone());
        self.loaded_side.insert(VOICE.to_string(), voice);
        self.loaded_side.insert(PARAMS.to_string(), params);
        self.loaded_side.insert(self.speakers_file.clone(), speakers);
        let rev = self.script.rev + 1;
        self.script = Script { blocks, memo, voice: voice_v, params: params_v, speakers: Some(speakers_v), rev };
        self.remember();
        Ok(())
    }

    /// 話者の声の設定のファイルを切り替える（声のエンジンを変えたとき）。新しいファイルから話者の声を読み直し、rev を進める。
    /// 変わったら true。新しいファイルが読めなければ、何も変えずにエラー（壊れたファイルを空として上書きしないため）。
    /// 前のエンジンのファイルは触らない（そのエンジンに戻したときに読み直す）
    pub fn set_speakers_file(&mut self, name: &str) -> Result<bool> {
        if self.speakers_file == name {
            return Ok(false);
        }
        let (text, map) = read_side_json(&self.dir, name)?;
        self.loaded_side.remove(&self.speakers_file);
        self.speakers_file = name.to_string();
        self.loaded_side.insert(name.to_string(), text);
        self.script.speakers = Some(map);
        self.script.rev += 1;
        self.remember();
        Ok(true)
    }

    /// 今の台本を rev つきで覚える（古い rev をもとにした保存のマージ用）
    fn remember(&mut self) {
        self.history.retain(|h| h.rev != self.script.rev);
        self.history.push_back(self.script.clone());
        while self.history.len() > HISTORY {
            self.history.pop_front();
        }
    }

    /// ディスクの中身が、最後に読み書きした内容から変わっているか（Claude などが直接書き換えたとき）。
    pub fn changed_on_disk(&self) -> bool {
        let Ok((names, _)) = scan(&self.dir) else { return false };
        if names.len() != self.loaded.len() || names.iter().any(|n| !self.loaded.contains_key(n)) {
            return true;
        }
        // 移せる古い台本が増えた
        if md_todo(&self.dir).map(|(t, _)| !t.is_empty()).unwrap_or(false) {
            return true;
        }
        for (f, c) in &self.loaded {
            if read_opt(&self.dir.join(f)).as_deref() != Some(c.as_str()) {
                return true;
            }
        }
        for (f, c) in &self.loaded_side {
            let disk = read_opt(&self.dir.join(f));
            let empty = if *f == MEMO { "" } else { "{}" };
            if disk.as_deref().unwrap_or(empty) != c.as_str() {
                return true;
            }
        }
        false
    }

    /// 書き換える前の中身を _backup に残す（セッションで 1 回）。残せなければ Err（呼び出し元は書き換えをやめる）
    fn backup(&mut self, name: &str, content: &str) -> Result<()> {
        if self.backed_up.contains(name) {
            return Ok(());
        }
        self.write_backup(name, content)?;
        self.backed_up.insert(name.to_string());
        Ok(())
    }

    /// 画面と外の両方が同じファイルを変えていたとき、外の中身を _backup に残す（画面の中身で書くので）。
    /// 残せなければ Err（外の中身が消えるので、呼び出し元は書き換えをやめる）
    fn backup_external(&self, name: &str, content: &str) -> Result<()> {
        // 同じ秒に 2 回ぶつかっても前の控えを上書きしないよう、ミリ秒まで
        let stamp = chrono::Local::now().format("%H%M%S%3f");
        self.write_backup(&format!("{name}.external-{stamp}"), content)
    }

    fn write_backup(&self, file: &str, content: &str) -> Result<()> {
        let d = self.dir.join("_backup").join(&self.session);
        (|| -> Result<()> {
            std::fs::create_dir_all(&d)?;
            std::fs::write(d.join(file), content)?;
            Ok(())
        })()
        .with_context(|| format!("台本の控え（script/_backup/{}/{file}）を保存できませんでした", self.session))
    }

    /// 画面（や Claude）から来た台本を保存する。変わったファイルだけ書く。
    pub fn save(&mut self, mut s: Script) -> Result<bool> {
        if s.speakers.is_none() {
            s.speakers = self.script.speakers.clone();
        }
        // 画面が見ていた rev が今より古いときは、その rev の台本と比べて「画面が変えたか」を決める
        // （画面が反映を受け取る前に Claude などが書いたものを、古い中身で上書きしないため）
        let base: Option<Script> = if s.rev < self.script.rev {
            self.history.iter().find(|h| h.rev == s.rev).cloned()
        } else {
            None
        };
        let base_yaml = |f: &str| -> Option<String> {
            base.as_ref().and_then(|b| b.blocks.iter().find(|x| x.file.as_deref() == Some(f)).map(to_yaml))
        };
        let mut used: HashSet<String> = s.blocks.iter().filter_map(|b| b.file.clone()).collect();
        if let Ok(rd) = std::fs::read_dir(&self.dir) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                if !self.loaded.contains_key(&n) {
                    used.insert(n);
                }
            }
        }
        for b in s.blocks.iter_mut() {
            if b.file.is_none() {
                let name = safe_name(&b.title, &used);
                used.insert(name.clone());
                b.file = Some(name);
            }
        }
        let mut changed = false;
        // 外で書き換わったファイル: 画面がそのファイルを変えていなければ外の中身を残し（あとで読み直す）、
        // 両方が変えていたら画面の中身で書いて、外の中身は _backup に残す
        let mut external = false;
        let mut snapshot: Vec<(String, String)> =
            s.blocks.iter().map(|b| (b.file.clone().unwrap(), to_yaml(b))).collect();
        if base.is_some() {
            // 画面が知らないうちに外が消したファイル（画面は変えていない）は、書き戻さない
            let before = snapshot.len();
            snapshot.retain(|(f, c)| self.loaded.contains_key(f) || base_yaml(f).is_none_or(|b| b != *c));
            external |= snapshot.len() != before;
        }
        let cur_yaml: HashMap<String, String> = self
            .script
            .blocks
            .iter()
            .filter_map(|b| b.file.clone().map(|f| (f, to_yaml(b))))
            .collect();
        let mut new_loaded: HashMap<String, String> = HashMap::new();
        let mut new_canon: HashMap<String, String> = HashMap::new();
        // 書くもの・消すものは、控えをすべて残せてから書く（控えが残せなければ何も書かずに Err）
        let mut writes: Vec<(String, String)> = vec![];
        let mut removes: Vec<String> = vec![];
        let mut side_written: Vec<(String, String)> = vec![];
        for (f, c) in &snapshot {
            if let Some(base) = self.loaded.get(f).cloned() {
                let disk = read_opt(&self.dir.join(f));
                // 画面が変えていない（読んだときの中身を決まった形にしたものと同じ）
                let from_base = base_yaml(f).is_some_and(|b| b == *c);
                let ui_same = self.canon.get(f) == Some(c) || base == *c || from_base;
                if from_base && cur_yaml.get(f) != Some(c) {
                    // 画面が見ていた古い中身のまま。今の（外が書いた）中身を残して、画面に返す
                    external = true;
                }
                if disk.as_ref() != Some(&base) {
                    if ui_same {
                        external = true;
                        new_loaded.insert(f.clone(), base);
                        continue;
                    }
                    if let Some(d) = &disk {
                        self.backup_external(f, d)?;
                    }
                } else if ui_same {
                    // 手で整えたファイルは、画面が変えていなければ書き換えない
                    new_loaded.insert(f.clone(), base);
                    new_canon.insert(f.clone(), c.clone());
                    continue;
                }
                if disk.as_ref() == Some(&base) && base_yaml(f).is_some_and(|b| cur_yaml.get(f) != Some(&b)) {
                    // 画面と外の両方が変えた（外が書いた今の中身を残す）
                    self.backup_external(f, &base)?;
                }
                self.backup(f, &base)?;
            }
            writes.push((f.clone(), c.clone()));
            new_loaded.insert(f.clone(), c.clone());
            new_canon.insert(f.clone(), c.clone());
            changed = true;
        }
        let keep: HashSet<&String> = snapshot.iter().map(|(f, _)| f).collect();
        // 画面が知らないうちに外が増やしたファイル（その rev に無い）は、消さない
        let removed: Vec<String> = self
            .loaded
            .keys()
            .filter(|f| !keep.contains(f))
            .filter(|f| {
                let unknown = base.as_ref().is_some_and(|b| !b.blocks.iter().any(|x| x.file.as_deref() == Some(f.as_str())));
                if unknown {
                    external = true;
                }
                !unknown
            })
            .cloned()
            .collect();
        for f in removed {
            if let Some(old) = self.loaded.get(&f).cloned() {
                self.backup(&f, &old)?;
                if let Some(d) = read_opt(&self.dir.join(&f)).filter(|d| *d != old) {
                    self.backup_external(&f, &d)?;
                }
            }
            removes.push(f);
            changed = true;
        }
        let mut order: Vec<String> = snapshot.iter().map(|(f, _)| f.clone()).collect();
        if external && base.is_some() {
            // 外が増やしたファイルも順番に残す
            for f in new_loaded.keys() {
                if !order.contains(f) {
                    order.push(f.clone());
                }
            }
        }
        let order_s = serde_json::to_string_pretty(&order)?;
        if read_opt(&self.dir.join(ORDER)).as_deref() != Some(order_s.as_str()) {
            writes.push((ORDER.to_string(), order_s));
            changed = true;
        }
        // 付属ファイル（中身が変わったときだけ）
        let side: [(String, String); 4] = [
            (MEMO.to_string(), s.memo.clone()),
            (VOICE.to_string(), serde_json::to_string(&Value::Object(s.voice.clone()))?),
            (PARAMS.to_string(), serde_json::to_string(&Value::Object(s.params.clone()))?),
            (self.speakers_file.clone(), serde_json::to_string(&Value::Object(s.speakers.clone().unwrap_or_default()))?),
        ];
        for (name, content) in side {
            let name = name.as_str();
            let prev = self.loaded_side.get(name).cloned().unwrap_or_default();
            let empty = if name == MEMO { "" } else { "{}" };
            let same_cur = prev == content || (prev.is_empty() && content == empty);
            let base_c = base.as_ref().map(|b| side_of(b, name));
            let from_base = base_c.as_ref().is_some_and(|b| *b == content || (b.is_empty() && content == empty));
            let same = same_cur || from_base;
            let disk = read_opt(&self.dir.join(name)).unwrap_or_else(|| empty.to_string());
            let disk_changed = disk != prev && !(prev.is_empty() && disk == empty);
            if disk_changed {
                if same {
                    external = true;
                    continue;
                }
                self.backup_external(name, &disk)?;
            } else if !same && base_c.as_ref().is_some_and(|b| *b != prev) {
                // 画面と外の両方が変えた（外の中身は書いたあとで読み直せないので残す）
                self.backup_external(name, &disk)?;
            }
            if same {
                if !same_cur {
                    external = true;
                }
                continue;
            }
            if !prev.is_empty() {
                self.backup(name, &prev)?;
            }
            writes.push((name.to_string(), content.clone()));
            side_written.push((name.to_string(), content));
            changed = true;
        }
        // 途中で書けなかったときは、書けた分も loaded に入らない。次の保存で自分の書いた中身を外の変更として控えに残すが、消えはしない
        for (f, c) in &writes {
            crate::settings::write_atomic(&self.dir.join(f), c)?;
        }
        for f in &removes {
            match std::fs::remove_file(self.dir.join(f)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    // 残ったファイルは読んだ中身のまま覚えておく（次の読み直しで、黙って生き返らせない）
                    if let Some(old) = self.loaded.get(f) {
                        new_loaded.insert(f.clone(), old.clone());
                    }
                    self.notices.push(Notice { text: format!("台本 {f} を消せませんでした: {e}"), error: true });
                }
            }
        }
        self.loaded = new_loaded;
        self.canon = new_canon;
        self.loaded_side.extend(side_written);
        if changed {
            s.rev = self.script.rev + 1;
        } else {
            s.rev = self.script.rev;
        }
        self.merged_external = external;
        if external {
            // 画面の変更は書いたので、外の変更と合わせてディスクから読み直す（行の id は引き継ぐ）
            self.reload(Some(&s))?;
            return Ok(true);
        }
        self.script = s;
        self.remember();
        Ok(changed)
    }
}

fn safe_name(title: &str, used: &HashSet<String>) -> String {
    let mut base: String = title
        .chars()
        .map(|c| if "\\/:*?\"<>|".contains(c) || c.is_whitespace() { '_' } else { c })
        .take(40)
        .collect();
    if base.is_empty() {
        base = "block".into();
    }
    let mut name = format!("{}.yaml", base);
    let mut i = 2;
    while used.contains(&name) {
        name = format!("{}_{}.yaml", base, i);
        i += 1;
    }
    name
}

/// 数字を数として比べる名前順（"2.yaml" < "10.yaml"）。
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let re = Regex::new(r"(\d+)|(\D+)").unwrap();
    let pa: Vec<String> = re.find_iter(a).map(|m| m.as_str().to_string()).collect();
    let pb: Vec<String> = re.find_iter(b).map(|m| m.as_str().to_string()).collect();
    for (x, y) in pa.iter().zip(pb.iter()) {
        let o = match (x.parse::<u64>(), y.parse::<u64>()) {
            (Ok(i), Ok(j)) => i.cmp(&j),
            _ => x.cmp(y),
        };
        if o != std::cmp::Ordering::Equal {
            return o;
        }
    }
    pa.len().cmp(&pb.len())
}

/// ほかの回の台本を、書き換えずに読む。`dir` は回の `script/` フォルダ。
/// 直下のファイルだけを一時フォルダに写し、そこで開く（古い .md の移行や _voice.json の鍵の書き換えは一時フォルダの中で起きる）。
/// 一時フォルダは読み終わったら消す（消せなくてもエラーにしない）。話者の声は speakers_file（今のエンジンのもの）から読む
pub fn peek(dir: &Path, speakers_file: &str) -> Result<Script> {
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let rd = std::fs::read_dir(dir).with_context(|| format!("{} を読めません", dir.display()))?;
    let tmp = std::env::temp_dir().join(format!(
        "ak-studio-peek-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).context("一時フォルダを作れません")?;
    let run = || -> Result<Script> {
        for e in rd {
            let e = e?;
            if e.file_type()?.is_file() {
                std::fs::copy(e.path(), tmp.join(e.file_name()))
                    .with_context(|| format!("{} を写せません", e.file_name().to_string_lossy()))?;
            }
        }
        Ok(ScriptStore::open_with(&tmp, speakers_file)?.script)
    };
    let r = run();
    let _ = std::fs::remove_dir_all(&tmp);
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("aks-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn line(id: &str, sp: &str, text: &str) -> Line {
        Line { id: id.into(), sp: sp.into(), text: text.into(), start: None, memo: String::new() }
    }

    fn block(title: &str, lines: Vec<Line>, notes: &str) -> Block {
        Block { id: "b".into(), file: None, title: title.into(), lines, notes: notes.into() }
    }

    fn yaml(t: &str) -> String {
        format!("title: A\nlines:\n  - id: l1\n    sp: トーク1\n    text: {t}\n")
    }

    // ---- 保存・外の変更

    #[test]
    fn save_keeps_external_changes() {
        let dir = tmp("scriptext");
        std::fs::write(dir.join("1.yaml"), yaml("前")).unwrap();
        std::fs::write(dir.join("2.yaml"), yaml("に").replace("l1", "l2")).unwrap();
        let mut st = ScriptStore::open(&dir).unwrap();
        // 外（Claude など）が 1.yaml と _voice.json を書き換え、画面は 2.yaml だけ直して保存する
        std::fs::write(dir.join("1.yaml"), yaml("外")).unwrap();
        std::fs::write(dir.join(VOICE), r#"{"k": 1}"#).unwrap();
        let mut s = st.script.clone();
        let b2 = s.blocks.iter_mut().find(|b| b.file.as_deref() == Some("2.yaml")).unwrap();
        b2.lines[0].text = "画面".into();
        st.save(s).unwrap();
        assert!(st.merged_external);
        assert_eq!(std::fs::read_to_string(dir.join("1.yaml")).unwrap(), yaml("外"));
        assert!(std::fs::read_to_string(dir.join("2.yaml")).unwrap().contains("text: 画面\n"));
        assert_eq!(std::fs::read_to_string(dir.join(VOICE)).unwrap(), r#"{"k": 1}"#);
        assert!(st.script.voice.contains_key("k"));
        // 書きかけの _voice.json は読み直さずにエラーにし、前の中身を残す
        std::fs::write(dir.join(VOICE), r#"{"k": "#).unwrap();
        let prev = st.script.clone();
        assert!(st.reload(Some(&prev)).is_err());
        assert!(st.script.voice.contains_key("k"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 外（API の memo_write など）が書いた直後に、それを知らない画面が古い台本を保存しても、外の中身が残る
    fn api_write(st: &mut ScriptStore, f: impl FnOnce(&mut Script)) {
        let mut s = st.script.clone();
        f(&mut s);
        st.save(s).unwrap();
    }

    #[test]
    fn stale_save_keeps_api_memo() {
        let dir = tmp("stalememo");
        std::fs::write(dir.join("1.yaml"), yaml("前")).unwrap();
        let mut st = ScriptStore::open(&dir).unwrap();
        let ui = st.script.clone(); // 画面が持っている（rev は古い）
        api_write(&mut st, |s| s.memo = "新しいメモ".into());
        assert!(st.script.rev > ui.rev);
        // 画面は台本の文を直しただけ（メモは古いまま）
        let mut ui = ui;
        ui.blocks[0].lines[0].text = "画面".into();
        st.save(ui).unwrap();
        assert!(st.merged_external);
        assert_eq!(std::fs::read_to_string(dir.join(MEMO)).unwrap(), "新しいメモ");
        assert_eq!(st.script.memo, "新しいメモ");
        assert!(std::fs::read_to_string(dir.join("1.yaml")).unwrap().contains("text: 画面
"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_save_keeps_api_block_edit() {
        let dir = tmp("staleblock");
        std::fs::write(dir.join("1.yaml"), yaml("前")).unwrap();
        let mut st = ScriptStore::open(&dir).unwrap();
        let ui = st.script.clone();
        api_write(&mut st, |s| {
            s.blocks[0].lines[0].text = "外".into();
            s.blocks[0].lines[0].memo = "行メモ".into();
        });
        // 画面は何も変えていない古い写しを保存する
        st.save(ui).unwrap();
        assert!(st.merged_external);
        let y = std::fs::read_to_string(dir.join("1.yaml")).unwrap();
        assert!(y.contains("text: 外
") && y.contains("行メモ"), "{y}");
        assert_eq!(st.script.blocks[0].lines[0].text, "外");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ui_edit_after_seeing_new_memo_wins() {
        let dir = tmp("uiwins");
        std::fs::write(dir.join("1.yaml"), yaml("前")).unwrap();
        let mut st = ScriptStore::open(&dir).unwrap();
        api_write(&mut st, |s| s.memo = "API".into());
        // 画面が新しい rev を受け取ってから、メモを書き換えた
        let mut ui = st.script.clone();
        ui.memo = "画面で直した".into();
        st.save(ui).unwrap();
        assert!(!st.merged_external);
        assert_eq!(std::fs::read_to_string(dir.join(MEMO)).unwrap(), "画面で直した");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_both_changed_ui_wins_with_backup() {
        let dir = tmp("bothchanged");
        std::fs::write(dir.join("1.yaml"), yaml("前")).unwrap();
        let mut st = ScriptStore::open(&dir).unwrap();
        let mut ui = st.script.clone();
        api_write(&mut st, |s| {
            s.memo = "API".into();
            s.blocks[0].lines[0].text = "外".into();
        });
        ui.memo = "画面".into();
        ui.blocks[0].lines[0].text = "画面の文".into();
        st.save(ui).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join(MEMO)).unwrap(), "画面");
        assert!(std::fs::read_to_string(dir.join("1.yaml")).unwrap().contains("text: 画面の文
"));
        // 外の中身は _backup（セッションごとのフォルダ）に残る
        let sess = dir.join("_backup").join(backups(&dir).remove(0));
        let names: Vec<String> =
            std::fs::read_dir(&sess).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        assert!(names.iter().any(|n| n.starts_with("1.yaml.external-")), "{names:?}");
        assert!(names.iter().any(|n| n.starts_with("_memo.md.external-")), "{names:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 控えを残せないときは、外の中身を消さないよう何も書かずに Err（旧#137）
    #[test]
    fn backup_failure_stops_save() {
        let dir = tmp("backupfail");
        std::fs::write(dir.join("1.yaml"), yaml("前")).unwrap();
        std::fs::write(dir.join("2.yaml"), yaml("に").replace("l1", "l2")).unwrap();
        let mut st = ScriptStore::open(&dir).unwrap();
        let mut ui = st.script.clone();
        api_write(&mut st, |s| {
            s.memo = "API".into();
            s.blocks[0].lines[0].text = "外".into();
        });
        // _backup をファイルにして、控えのフォルダを作れなくする
        let _ = std::fs::remove_dir_all(dir.join("_backup"));
        std::fs::write(dir.join("_backup"), "").unwrap();
        let files = |d: &Path| -> Vec<(String, String)> {
            let mut v: Vec<(String, String)> = std::fs::read_dir(d)
                .unwrap()
                .flatten()
                .map(|e| (e.file_name().to_string_lossy().to_string(), std::fs::read_to_string(e.path()).unwrap()))
                .collect();
            v.sort();
            v
        };
        let before = files(&dir);
        // 画面と外の両方が変えた（外の中身を控えに残せない）。画面は 2.yaml も直している
        ui.memo = "画面".into();
        ui.blocks[0].lines[0].text = "画面の文".into();
        ui.blocks[1].lines[0].text = "画面の 2".into();
        let e = st.save(ui).unwrap_err();
        assert!(format!("{e:#}").contains("控え"), "{e:#}");
        assert_eq!(files(&dir), before, "1 つも書かない");
        // 画面だけが変えたときも、書き換える前の中身を残せなければ書かない。
        // 1.yaml は控え済みなので先に書く予定に入るが、2.yaml の控えで失敗したら 1.yaml も書かない
        let kept = st.script.clone();
        let mut ui = st.script.clone();
        ui.blocks[0].lines[0].text = "画面の 1".into();
        ui.blocks[1].lines[0].text = "画面だけ".into();
        assert!(st.save(ui).is_err());
        assert_eq!(files(&dir), before);
        assert_eq!((st.script.rev, &st.script.blocks), (kept.rev, &kept.blocks), "覚えている台本も変えない");
        // 付属ファイル（メモ）も同じ
        let mut ui = st.script.clone();
        ui.memo = "画面のメモ".into();
        assert!(st.save(ui).is_err());
        assert_eq!(files(&dir), before);
        // 控えを残せるようになれば保存できる
        std::fs::remove_file(dir.join("_backup")).unwrap();
        let mut ui = st.script.clone();
        ui.blocks[1].lines[0].text = "画面だけ".into();
        st.save(ui).unwrap();
        assert!(std::fs::read_to_string(dir.join("2.yaml")).unwrap().contains("text: 画面だけ\n"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_save_before_watcher_reload_keeps_disk_memo() {
        let dir = tmp("diskmemo");
        std::fs::write(dir.join("1.yaml"), yaml("前")).unwrap();
        let mut st = ScriptStore::open(&dir).unwrap();
        // Claude がファイルを直接書き換え、見張りの読み直しの前に画面の保存が来る
        std::fs::write(dir.join(MEMO), "ファイルのメモ").unwrap();
        let mut ui = st.script.clone();
        ui.blocks[0].lines[0].text = "画面".into();
        st.save(ui).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join(MEMO)).unwrap(), "ファイルのメモ");
        assert_eq!(st.script.memo, "ファイルのメモ");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_save_keeps_externally_added_file() {
        let dir = tmp("staleadd");
        std::fs::write(dir.join("1.yaml"), yaml("前")).unwrap();
        let mut st = ScriptStore::open(&dir).unwrap();
        let ui = st.script.clone();
        std::fs::write(dir.join("2.yaml"), yaml("に").replace("l1", "l2")).unwrap();
        let prev = st.script.clone();
        st.reload(Some(&prev)).unwrap();
        st.save(ui).unwrap();
        assert!(dir.join("2.yaml").exists());
        assert_eq!(st.script.blocks.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ids_come_from_file_and_persist() {
        let dir = tmp("lineids");
        // id 無し・重なる id の手書き。手で整えた書き方（コメント・字下げ違い）
        let hand = "# 手書き\ntitle: A\nlines:\n- id: a\n  sp: トーク1\n  text: ひとつめ\n- id: a\n  sp: トーク2\n  text: ふたつめ\n- sp: トーク1\n  text: みっつめ\n";
        std::fs::write(dir.join("1.yaml"), hand).unwrap();
        let mut st = ScriptStore::open(&dir).unwrap();
        let ls = &st.script.blocks[0].lines;
        assert_eq!(ls[0].id, "a");
        assert_ne!(ls[1].id, "a");
        assert_ne!(ls[2].id, ls[1].id);
        let ids: Vec<String> = ls.iter().map(|l| l.id.clone()).collect();
        // 画面が変えないうちは、手書きのファイルを書き換えない…が、id を付けたファイルは次の保存で書く
        st.save(st.script.clone()).unwrap();
        let on_disk = std::fs::read_to_string(dir.join("1.yaml")).unwrap();
        assert!(on_disk.contains(&ids[1]) && on_disk.contains(&ids[2]));
        let st2 = ScriptStore::open(&dir).unwrap();
        let ids2: Vec<String> = st2.script.blocks[0].lines.iter().map(|l| l.id.clone()).collect();
        assert_eq!(ids, ids2);
        // 全部 id がある手書きのファイルは、画面が変えなければそのまま
        let hand2 = "title: A\nlines:\n- id: a\n  sp: トーク1\n  text: ひとつめ\n";
        std::fs::write(dir.join("1.yaml"), hand2).unwrap();
        let mut st3 = ScriptStore::open(&dir).unwrap();
        st3.save(st3.script.clone()).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("1.yaml")).unwrap(), hand2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn broken_yaml_is_never_overwritten() {
        let dir = tmp("yamlbroken");
        std::fs::write(dir.join("1.yaml"), yaml("あ")).unwrap();
        std::fs::write(dir.join("2.yaml"), yaml("い").replace("l1", "l2")).unwrap();
        let mut st = ScriptStore::open(&dir).unwrap();
        let broken = "title: [\nlines:\n  - id";
        std::fs::write(dir.join("1.yaml"), broken).unwrap();
        // 開き直しはエラー（ファイル名が出る）
        let e = ScriptStore::open(&dir).err().unwrap().to_string();
        assert!(e.contains("1.yaml"), "{e}");
        // 読み直しもエラーで、前の台本はそのまま
        let prev = st.script.clone();
        assert!(st.reload(Some(&prev)).is_err());
        assert_eq!(st.script.blocks.len(), 2);
        // 画面が別のブロックを直して保存しても、壊れたファイルは書き換えない
        let mut s = st.script.clone();
        s.blocks[1].lines[0].text = "画面".into();
        let _ = st.save(s);
        assert_eq!(std::fs::read_to_string(dir.join("1.yaml")).unwrap(), broken);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn new_block_and_remove() {
        let dir = tmp("newblock");
        std::fs::write(dir.join("1.yaml"), yaml("あ")).unwrap();
        let mut st = ScriptStore::open(&dir).unwrap();
        let mut s = st.script.clone();
        s.blocks.push(block("新しい: ブロック", vec![line("lnew", "トーク2", "い")], ""));
        s.blocks[0].lines.clear();
        st.save(s).unwrap();
        let order: Vec<String> = serde_json::from_str(&std::fs::read_to_string(dir.join(ORDER)).unwrap()).unwrap();
        assert_eq!(order, vec!["1.yaml", "新しい__ブロック.yaml"]);
        let st2 = ScriptStore::open(&dir).unwrap();
        assert_eq!(st2.script.blocks[1].title, "新しい: ブロック");
        assert_eq!(st2.script.blocks[1].lines[0].id, "lnew");
        assert!(st2.script.blocks[0].lines.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn speakers_roundtrip() {
        let dir = tmp("speakers");
        std::fs::write(dir.join("1.yaml"), yaml("あ")).unwrap();
        let mut st = ScriptStore::open(&dir).unwrap();
        assert!(st.script.speakers.as_ref().unwrap().is_empty());
        // 調整が無いうちはファイルを作らない
        st.save(st.script.clone()).unwrap();
        assert!(!dir.join(SPEAKERS).exists());
        let mut s = st.script.clone();
        s.speakers.as_mut().unwrap().insert("トーク1".into(), serde_json::json!({"speed": 1.1, "style_weights": [1.0, 0.0]}));
        assert!(st.save(s).unwrap());
        let disk: Value = serde_json::from_str(&std::fs::read_to_string(dir.join(SPEAKERS)).unwrap()).unwrap();
        assert_eq!(disk["トーク1"]["speed"], 1.1);
        let mut st2 = ScriptStore::open(&dir).unwrap();
        assert_eq!(st2.script.speakers.as_ref().unwrap()["トーク1"]["style_weights"][0], 1.0);
        // 台本だけを直しても、話者の声はそのまま残る
        let mut s = st2.script.clone();
        s.blocks[0].lines[0].text = "い".into();
        st2.save(s).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join(SPEAKERS)).unwrap(), serde_json::to_string(&disk).unwrap());
        // speakers の無い Script を保存しても、話者の声は消さない
        let mut old: Script = serde_json::from_str(r#"{"blocks":[],"memo":"","voice":{},"params":{},"rev":1}"#).unwrap();
        old.blocks = st2.script.blocks.clone();
        assert!(old.speakers.is_none());
        st2.save(old).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join(SPEAKERS)).unwrap(), serde_json::to_string(&disk).unwrap());
        assert!(st2.script.speakers.as_ref().unwrap().contains_key("トーク1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn speakers_file_per_engine() {
        assert_eq!(speakers_file("voisona"), "_voice_speakers.json");
        assert_eq!(speakers_file(" voisona "), "_voice_speakers.json");
        assert_eq!(speakers_file("voicevox"), "_voice_speakers.voicevox.json");
        // パスのエンジンはファイル名の拡張子を除いた部分（区切りは \ も /）
        assert_eq!(speakers_file(r"D:\Engines\My_TTS.py"), "_voice_speakers.My_TTS.json");
        assert_eq!(speakers_file("D:/e/my-tts.PY"), "_voice_speakers.my-tts.json");
        // 英数字・_・- 以外は落とし、空なら custom
        assert_eq!(speakers_file("D:/e/my tts(2).py"), "_voice_speakers.mytts2.json");
        assert_eq!(speakers_file("D:/e/読み上げ.py"), "_voice_speakers.custom.json");
        assert_eq!(speakers_file(""), "_voice_speakers.custom.json");
    }

    #[test]
    fn speakers_follow_the_engine_file() {
        let dir = tmp("speakers-engine");
        std::fs::write(dir.join("1.yaml"), yaml("あ")).unwrap();
        let vv = speakers_file("voicevox");
        std::fs::write(dir.join(SPEAKERS), r#"{"トーク1":{"volume":-3.0}}"#).unwrap();
        let mut st = ScriptStore::open(&dir).unwrap();
        assert_eq!(st.script.speakers.as_ref().unwrap()["トーク1"]["volume"], -3.0);
        // VOICEVOX に切り替える: VoiSona の値（dB）は持ち越さず、VOICEVOX のファイル（無ければ空）を読む
        let rev = st.script.rev;
        assert!(st.set_speakers_file(&vv).unwrap());
        assert!(st.script.speakers.as_ref().unwrap().is_empty());
        assert!(st.script.rev > rev);
        assert!(!st.set_speakers_file(&vv).unwrap());
        // 保存は VOICEVOX のファイルに書き、VoiSona のファイルは変えない
        let mut s = st.script.clone();
        s.speakers.as_mut().unwrap().insert("トーク1".into(), serde_json::json!({"volume": 1.2}));
        assert!(st.save(s).unwrap());
        let disk: Value = serde_json::from_str(&std::fs::read_to_string(dir.join(&vv)).unwrap()).unwrap();
        assert_eq!(disk["トーク1"]["volume"], 1.2);
        let vs: Value = serde_json::from_str(&std::fs::read_to_string(dir.join(SPEAKERS)).unwrap()).unwrap();
        assert_eq!(vs["トーク1"]["volume"], -3.0);
        // 開き直しても、エンジンのファイルを読む
        assert_eq!(ScriptStore::open_with(&dir, &vv).unwrap().script.speakers.as_ref().unwrap()["トーク1"]["volume"], 1.2);
        assert_eq!(ScriptStore::open(&dir).unwrap().script.speakers.as_ref().unwrap()["トーク1"]["volume"], -3.0);
        // 見張り: 今のエンジンのファイルが外で変わったときだけ「変わった」。別のエンジンのファイルは関係ない
        assert!(!st.changed_on_disk());
        std::fs::write(dir.join(SPEAKERS), r#"{"トーク1":{"volume":0.0}}"#).unwrap();
        assert!(!st.changed_on_disk());
        std::fs::write(dir.join(&vv), r#"{"トーク1":{"volume":1.5}}"#).unwrap();
        assert!(st.changed_on_disk());
        let prev = st.script.clone();
        st.reload(Some(&prev)).unwrap();
        assert_eq!(st.script.speakers.as_ref().unwrap()["トーク1"]["volume"], 1.5);
        // VoiSona に戻すと、VoiSona のファイルを読み直す
        assert!(st.set_speakers_file(SPEAKERS).unwrap());
        assert_eq!(st.script.speakers.as_ref().unwrap()["トーク1"]["volume"], 0.0);
        // 読めないファイルへの切り替えは、何も変えずにエラー
        std::fs::write(dir.join(&vv), r#"{"トーク1": "#).unwrap();
        assert!(st.set_speakers_file(&vv).is_err());
        assert_eq!(st.script.speakers.as_ref().unwrap()["トーク1"]["volume"], 0.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn peek_reads_the_given_speakers_file() {
        let dir = tmp("peek-speakers");
        std::fs::write(dir.join("1.yaml"), yaml("あ")).unwrap();
        std::fs::write(dir.join(SPEAKERS), r#"{"トーク1":{"volume":-3.0}}"#).unwrap();
        std::fs::write(dir.join("_voice_speakers.voicevox.json"), r#"{"トーク1":{"volume":1.1}}"#).unwrap();
        assert_eq!(peek(&dir, SPEAKERS).unwrap().speakers.unwrap()["トーク1"]["volume"], -3.0);
        assert_eq!(peek(&dir, "_voice_speakers.voicevox.json").unwrap().speakers.unwrap()["トーク1"]["volume"], 1.1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- 古い回（.md）の移行

    fn backups(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir.join("_backup"))
            .map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect())
            .unwrap_or_default();
        v.sort();
        v
    }

    #[test]
    fn migrate_md_to_yaml() {
        let dir = tmp("migrate");
        std::fs::write(
            dir.join("1.md"),
            "## 序論\n\n| # | セリフ | 話者 | 開始 | メモ |\n|---|---|---|---|---|\n| 1 | 同じ文です | トーク1 | 3.150 | ここで出す<br>2行目 |\n| 2 | 同じ文です | トーク1 | 5.000 |  |\n| 3 | ｜時雨《しぐれ》です | トーク2 |  |  |\n\n画面に出すもの：旧#1\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("2.md"),
            "## 本編\n\n| # | セリフ | 話者 | 開始 |\n|---|---|---|---|\n| 1 | 同じ文です | トーク1 | 10.000 |\n| 2 | yes: no | talk2 |  |\n",
        )
        .unwrap();
        std::fs::write(dir.join("_order.json"), r#"["2.md","1.md"]"#).unwrap();
        std::fs::write(dir.join(MEMO), "構成メモ").unwrap();
        let v = serde_json::json!({
            "トーク1\u{1}同じ文です": "<xml-same/>",
            "トーク2\u{1}しぐれです": "<xml-ruby/>",
            "トーク2\u{1}どの行にも合わない": "<xml-gone/>"
        });
        std::fs::write(dir.join(VOICE), serde_json::to_string(&v).unwrap()).unwrap();
        let p = serde_json::json!({"トーク1\u{1}同じ文です": {"intonation": 1.2}, "トーク1\u{1}無い": {"speed": 2}});
        std::fs::write(dir.join(PARAMS), serde_json::to_string(&p).unwrap()).unwrap();

        let mut st = ScriptStore::open(&dir).unwrap();
        let n = st.take_notices();
        assert_eq!(n.len(), 1);
        assert!(n[0].text.contains("YAML") && n[0].text.contains("script/_backup/"), "{:?}", n[0]);
        // 元の .md は消え、.yaml ができ、順番は .yaml に
        assert!(!dir.join("1.md").exists() && !dir.join("2.md").exists());
        assert!(dir.join("1.yaml").exists() && dir.join("2.yaml").exists());
        assert!(dir.join(MEMO).exists());
        let order: Vec<String> = serde_json::from_str(&std::fs::read_to_string(dir.join(ORDER)).unwrap()).unwrap();
        assert_eq!(order, vec!["2.yaml", "1.yaml"]);
        // 控え
        let b = backups(&dir);
        assert_eq!(b.len(), 1);
        assert!(b[0].ends_with("-v2"));
        let bd = dir.join("_backup").join(&b[0]);
        for f in ["1.md", "2.md", "_order.json", "_voice.json", "_voice_params.json"] {
            assert!(bd.join(f).exists(), "{f} の控えが無い");
        }
        assert!(std::fs::read_to_string(bd.join("_voice.json")).unwrap().contains("xml-same"));
        // 中身
        let s = &st.script;
        assert_eq!(s.blocks.len(), 2);
        assert_eq!(s.blocks[0].title, "本編");
        assert_eq!(s.blocks[1].title, "序論");
        assert_eq!(s.blocks[1].notes, "画面に出すもの：旧#1");
        let l = &s.blocks[1].lines;
        assert_eq!(l[0].start, Some(3.15));
        assert_eq!(l[0].memo, "ここで出す\n2行目");
        assert_eq!(l[2].text, "｜時雨《しぐれ》です");
        assert_eq!(s.blocks[0].lines[1].sp, "トーク2");
        assert_eq!(s.blocks[0].lines[1].text, "yes: no");
        let mut ids = s.line_ids();
        assert_eq!(ids.len(), 5);
        assert!(ids.iter().all(|i| is_id_shape(i)));
        // 同じ文の 3 行（2 つのファイルにまたがる）は、みんな同じ手直し
        for line in [&l[0], &l[1], &s.blocks[0].lines[0]] {
            assert_eq!(s.voice_of(line), Some("<xml-same/>"));
            assert_eq!(s.params_of(line).unwrap()["intonation"], 1.2);
        }
        assert_eq!(s.voice_of(&l[2]), Some("<xml-ruby/>"));
        assert_eq!(s.voice.len(), 4);
        assert_eq!(s.params.len(), 3);
        assert!(!serde_json::to_string(&s.voice).unwrap().contains("xml-gone"));
        // ディスクの _voice.json も id の鍵
        let dv: Value = serde_json::from_str(&std::fs::read_to_string(dir.join(VOICE)).unwrap()).unwrap();
        assert!(dv.as_object().unwrap().keys().all(|k| ids.remove(k)));
        // 2 回目に開いたときは何も起きない（知らせ無し・控え増えない・ファイルそのまま）
        let before: Vec<String> = ["1.yaml", "2.yaml", VOICE, PARAMS, ORDER].iter().map(|f| std::fs::read_to_string(dir.join(f)).unwrap()).collect();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let mut st2 = ScriptStore::open(&dir).unwrap();
        assert!(st2.take_notices().is_empty());
        assert_eq!(backups(&dir), b);
        let after: Vec<String> = ["1.yaml", "2.yaml", VOICE, PARAMS, ORDER].iter().map(|f| std::fs::read_to_string(dir.join(f)).unwrap()).collect();
        assert_eq!(before, after);
        assert_eq!(st2.script.line_ids(), st.script.line_ids());
        // 保存しても id・手直しは変わらない
        st2.save(st2.script.clone()).unwrap();
        let st3 = ScriptStore::open(&dir).unwrap();
        assert_eq!(st3.script.line_ids(), st.script.line_ids());
        assert_eq!(st3.script.voice.len(), 4);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// フォルダの中身（相対パス → 中身）
    fn snapshot(dir: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
        fn walk(base: &Path, d: &Path, out: &mut std::collections::BTreeMap<String, Vec<u8>>) {
            for e in std::fs::read_dir(d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(base, &p, out);
                } else {
                    out.insert(p.strip_prefix(base).unwrap().to_string_lossy().to_string(), std::fs::read(&p).unwrap());
                }
            }
        }
        let mut m = Default::default();
        walk(dir, dir, &mut m);
        m
    }

    #[test]
    fn peek_reads_yaml() {
        let dir = tmp("peek-yaml");
        std::fs::write(dir.join("1.yaml"), yaml("よむだけ")).unwrap();
        let before = snapshot(&dir);
        let s = peek(&dir, SPEAKERS).unwrap();
        assert_eq!(s.blocks.len(), 1);
        assert_eq!(s.blocks[0].lines[0].text, "よむだけ");
        assert_eq!(snapshot(&dir), before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn peek_old_md_does_not_touch_source() {
        let dir = tmp("peek-md");
        std::fs::write(
            dir.join("1.md"),
            "## 序論\n\n| # | セリフ | 話者 | 開始 |\n|---|---|---|---|\n| 1 | 同じ文です | トーク1 | 3.150 |\n",
        )
        .unwrap();
        let v = serde_json::json!({"トーク1\u{1}同じ文です": "<xml-same/>"});
        std::fs::write(dir.join(VOICE), serde_json::to_string(&v).unwrap()).unwrap();
        let before = snapshot(&dir);
        let s = peek(&dir, SPEAKERS).unwrap();
        let l = &s.blocks[0].lines[0];
        assert_eq!(s.voice_of(l), Some("<xml-same/>"));
        assert!(s.voice.contains_key(&l.id), "鍵は行の id");
        // 元のフォルダはファイルの一覧も中身も変わらない
        assert_eq!(snapshot(&dir), before);
        assert!(!dir.join("_backup").exists() && !dir.join("1.yaml").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn peek_missing_dir_errors() {
        let dir = tmp("peek-none").join("script");
        assert!(peek(&dir, SPEAKERS).is_err());
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }

    #[test]
    fn md_and_yaml_both_use_yaml() {
        let dir = tmp("mdyamlboth");
        std::fs::write(dir.join("1.md"), "## 古い\n\n| # | セリフ | 話者 | 開始 |\n|---|---|---|---|\n| 1 | ふるい | トーク1 |  |\n").unwrap();
        std::fs::write(dir.join("1.yaml"), yaml("あたらしい")).unwrap();
        let mut st = ScriptStore::open(&dir).unwrap();
        assert_eq!(st.script.blocks.len(), 1);
        assert_eq!(st.script.blocks[0].lines[0].text, "あたらしい");
        assert!(dir.join("1.md").exists());
        let n = st.take_notices();
        assert_eq!(n.len(), 1);
        assert!(n[0].error);
        assert!(!st.changed_on_disk());
        assert!(backups(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod real_files {
    use super::*;

    /// AK_TEST_SCRIPT_DIR の台本（.md）を、控えた上で一時フォルダに写して移行し、行の数・文が変わらないか見る
    #[test]
    fn migrate_real() {
        let Ok(dir) = std::env::var("AK_TEST_SCRIPT_DIR") else { return };
        let tmp = std::env::temp_dir().join(format!("aks-real-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let mut want: HashMap<String, Parsed> = HashMap::new();
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if e.path().is_file() && !n.starts_with('.') {
                std::fs::copy(e.path(), tmp.join(&n)).unwrap();
                if is_md(&n) {
                    want.insert(yaml_name(&n), parse_md(&std::fs::read_to_string(e.path()).unwrap()));
                }
            }
        }
        let st = ScriptStore::open(&tmp).unwrap();
        for b in &st.script.blocks {
            if let Some(p) = want.get(b.file.as_deref().unwrap()) {
                assert_eq!(b.lines.len(), p.lines.len(), "{:?} の行数", b.file);
                for (x, y) in b.lines.iter().zip(&p.lines) {
                    assert_eq!((&x.sp, &x.text, x.start, &x.memo), (&y.sp, &y.text, y.start, &y.memo));
                }
                assert_eq!(b.notes, p.notes);
            }
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
