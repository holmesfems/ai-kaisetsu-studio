"""アクセント・辞書の案を、試聴しながら確認する画面（/review）用に作り、確認の結果を書き込む。

流れ：
    1. python voice_report.py --project <回> --words      全行の解析結果を一覧にする（voice_report.md）
    2. 一覧を読んで、案を draft.json に書く（下の形）
    3. python voice_review.py build --project <回> draft.json
         → <回>/voice_review/proposal.json。エディタの「アクセント確認」（http://localhost:8765/review）で開く
    4. 画面で試聴・調整して、採用／不採用／保留を決める（自動で <回>/voice_review/result.json に保存）
    5. python voice_review.py apply --project <回>            採用するものを見せるだけ
       python voice_review.py apply --project <回> --write    バックアップしてから辞書（voice_dict.json）と _voice.json に書き込む
         _voice.json は行の id が鍵：{"<id>": {"sp": 話者, "text": 読み上げの文, "analyzed_text": 解析結果の XML}}
         AK Studio は _voice.json が変わると自動で読み直す

draft.json の形（台本は script/*.yaml。行は台本の行の id で指す。id は voice_report.md の各行の `…` の中に出る）：
    {"dict": [{"surface": "S3", "reading": "エスサン", "accent": "lhhh", "why": "エス／サン に分かれる",
               "check": false, "question": "", "options": [{"label": "平板", "reading": "エスサン", "accent": "lhhh"}]}],
     "lines": [{"id": "l51x9k", "why": "…", "check": false, "question": "",
                "ops": [{"word": "いう", "hl": "hh", "chain": 1}]}]}

- lines の id は台本の行の id（必須）。file・no（ブロックのファイル名と、ブロック内の番号）は画面に出すためだけに
  proposal.json／result.json に付く（draft には書かなくてよい）。proposal.json・result.json の lines[].id も同じ行の id。
- dict の accent はモーラごとの h/l（読みのモーラ数と同じ長さ）。check=true は「要確認」（自信がない）で、
  question と options（選択肢）を付けると画面で選べる。
- lines の ops：word は単語の表記（全角半角は区別しない）、nth は同じ表記の何番目か（1から）、
  hl は高低、chain は 1＝前の単語と同じアクセント句／0＝ここで区切る。
  先に、今の辞書と draft の辞書で読みが一致する単語をまとめてから ops を当てる（"dict": false で当てない）。
- _voice.json に入った行（話者と読み上げの文が今の行と同じ手直しがある行）には辞書が効かなくなる。だから行の案には辞書の分も当てておく。
  読みを変える辞書（R2D2 → アールツーディーツー など）は解析し直さないと当たらないので、その行の手直しは解析し直してから。

voice_cache は回のフォルダのものを使う。サーバーを別のフォルダで起動していたときは --cache で足す。

辞書は声の中継（AK Studio が回を開くと動く。--port、無ければ AK_VOICE_PORT・8765）から読み書きする。
Claude のアプリの中から %LOCALAPPDATA% の辞書を直接書くと、AK Studio の読まない写しにだけ入るため。
中継が動いていなければ止まる。辞書のファイルを直接使うときは --dict-file（AK_VOICE_DICT か %LOCALAPPDATA%）。
"""
import argparse
import json
import shutil
import sys
import unicodedata
import xml.etree.ElementTree as ET
from datetime import datetime
from pathlib import Path

TOOLS = Path(__file__).resolve().parent
sys.path.insert(0, str(TOOLS))
import akscript as ak  # noqa: E402
import voisona_tts as vt  # noqa: E402
import voice_report as vr  # noqa: E402


def nk(s):
    return unicodedata.normalize("NFKC", s or "")


def apply_ops(xml, ops):
    root = ET.fromstring(xml)
    words = [w for w in root.iter("word") if w.get("phoneme")]
    for op in ops:
        hits = [w for w in words if nk(w.text or w.get("original")) == nk(op["word"])]
        nth = op.get("nth", 1)
        if len(hits) < nth:
            raise ValueError(f"「{op['word']}」({nth}番目) が見つかりません")
        w = hits[nth - 1]
        if "hl" in op:
            n = len(w.get("phoneme").split("|"))
            if len(op["hl"]) != n:
                raise ValueError(f"「{op['word']}」はモーラが {n} 個なのに、高低が {op['hl']} です")
            w.set("hl", op["hl"])
        if "chain" in op:
            w.set("chain", str(op["chain"]))
    return ET.tostring(root, encoding="unicode")


def mora_count(reading):
    return len([c for c in vt.kata(reading) if c not in "ァィゥェォャュョヮ"])


class Project:
    def __init__(self, path, caches=()):
        self.path = Path(path).resolve()
        self.script = self.path / "script"
        self.voice_file = self.script / "_voice.json"
        self.voice = ak.load_voice(self.script)
        self.latest = {}
        for c in reversed([self.path / "voice_cache"] + [Path(c).resolve() for c in caches]):
            self.latest.update(vr.index_cache(c))
        self.lines = {}  # 行の id → 行
        for blk in ak.read_blocks(self.script):
            for ln in blk["lines"]:
                self.lines[ln["id"]] = {"id": ln["id"], "file": blk["file"], "no": ln["no"], "title": blk["title"],
                                        "speaker": ln["sp"], "raw": ln["text"], "text": vr.tts_text(ln["text"])}

    def current(self, ln):
        """その行の今の解析結果（手直し済みなら _voice.json の分）と、手直し済みかどうか。"""
        edited = ak.active_voice(self.voice, {"id": ln["id"], "sp": ln["speaker"], "text": ln["raw"]})
        if edited:
            return edited, True
        d = self.latest.get((ln["speaker"], ln["text"]))
        return (d["analyzed_text"] if d else None), False


def example(proj, ln):
    """辞書の案を試す行。手直し済みの行には辞書が効かないので、その解析結果も付ける（「今」の再生用）。"""
    cur, edited = proj.current(ln)
    return {**{k: ln[k] for k in ("id", "file", "no", "title", "speaker", "text")},
            "edited": edited, "analyzed_text": cur if edited else None}


def read_dict(args):
    """今の辞書。ふだんは声の中継から（AK Studio と同じ本物の辞書）。--dict-file のときだけファイルから。"""
    if args.dict_file:
        return vt.load_dict()
    entries = vt.relay_dict(args.port)
    if entries is None:
        sys.exit(f"声の中継（{vt.relay_base(args.port)}）につながりません。AK Studio で回を開いてからやり直してください"
                 "（辞書のファイルを直接使うときは --dict-file）")
    return entries


def build(args):
    proj = Project(args.project, args.cache)
    draft = json.loads(Path(args.draft).read_text(encoding="utf-8"))
    entries = read_dict(args)
    draft_dict = draft.get("dict", [])
    for e in draft_dict:
        if e.get("accent") and len(e["accent"]) != mora_count(e["reading"]):
            sys.exit(f"辞書の案「{e['surface']}」：読み {e['reading']} のモーラ数と高低 {e['accent']} の長さが違います")
    trial = [x for x in entries if x.get("surface") not in {e["surface"] for e in draft_dict}] + draft_dict

    out_dict = []
    for i, e in enumerate(draft_dict, 1):
        ex = [ln for ln in proj.lines.values() if e["surface"] in ln["text"]]
        out_dict.append({"id": f"D{i}", "surface": e["surface"], "reading": e["reading"], "accent": e.get("accent", ""),
                         "why": e.get("why", ""), "check": bool(e.get("check")), "question": e.get("question", ""),
                         "options": e.get("options", []),
                         "registered": next((x for x in entries if x.get("surface") == e["surface"]), None),
                         "examples": [example(proj, ln) for ln in ex]})

    out_lines, used = [], set()
    for fx in draft.get("lines", []):
        lid = fx.get("id")
        ln = proj.lines.get(lid) if lid else None
        if not ln:
            sys.exit(f"行が見つかりません：id={lid!r}（draft の lines には台本の行の id を書く）")
        if lid in used:
            sys.exit(f"同じ行が二度出ています：{lid}（{ln['title']} #{ln['no']}）")
        used.add(lid)
        cur, edited = proj.current(ln)
        if not cur:
            sys.exit(f"まだ解析していない行です：{ln['title']} #{ln['no']}（{lid}。AK Studio で解析してから）")
        xml = vt.apply_accent(cur, vt.relevant(trial, ln["text"])) if fx.get("dict", True) else cur
        xml = apply_ops(xml, fx.get("ops", []))
        out_lines.append({"id": lid, **{k: ln[k] for k in ("file", "no", "title", "speaker", "text")},
                          "edited": edited, "current": cur, "proposed": xml, "why": fx.get("why", ""),
                          "check": bool(fx.get("check")), "question": fx.get("question", "")})
        print(f"{lid} {ln['title']} #{ln['no']}\n  前：{vr.render(vr.parse(cur))[0]}\n  案：{vr.render(vr.parse(xml))[0]}")

    d = proj.path / "voice_review"
    d.mkdir(exist_ok=True)
    out = {"project": proj.path.name, "created": datetime.now().isoformat(timespec="seconds"),
           "dict": out_dict, "lines": out_lines}
    (d / "proposal.json").write_text(json.dumps(out, ensure_ascii=False, indent=1), encoding="utf-8")
    if (d / "result.json").exists():  # 前の確認結果は、新しい案と混ざらないように退ける
        (d / "result.json").rename(d / f"result_{datetime.now():%Y%m%d-%H%M%S}.json")
    print(f"辞書 {len(out_dict)} 件、行 {len(out_lines)} 件 → {d / 'proposal.json'}")
    print("エディタの「アクセント確認」（http://localhost:8765/review）で開いてください")


def backup_dir(folder):
    bk = folder / "_backup" / f"claude-{datetime.now():%Y%m%d-%H%M%S}"  # エディタの _backup と同じ形
    bk.mkdir(parents=True, exist_ok=True)
    return bk


def backup(path, folder):
    dst = backup_dir(folder) / path.name
    shutil.copy2(path, dst)
    return dst


def backup_dict(entries, folder):
    """書き込む前の辞書を、回の script/_backup に控える（%LOCALAPPDATA% の側は写しに入ることがあるので回の側に）。"""
    dst = backup_dir(folder) / "voice_dict.json"
    dst.write_text(json.dumps({"entries": entries}, ensure_ascii=False, indent=1), encoding="utf-8")
    return dst


def apply(args):
    proj = Project(args.project)
    res_file = proj.path / "voice_review" / "result.json"
    if not res_file.exists():
        sys.exit(f"確認の結果がありません：{res_file}")
    res = json.loads(res_file.read_text(encoding="utf-8"))
    label = {"ok": "採用", "ng": "不採用", "hold": "保留"}

    entries = read_dict(args)
    new_dict = list(entries)
    print(f"確認の結果（{res.get('saved', '')}）")
    for e in res.get("dict", []):
        st = e.get("status") or "hold"
        print(f"  {e['id']} {label.get(st, st)}：{e['surface']} → {e['reading']} {e['accent']}"
              + (f"  メモ：{e['note']}" if e.get("note") else ""))
        if st == "ok":
            if len(e["accent"]) != mora_count(e["reading"]):
                sys.exit(f"  {e['id']} の高低の長さが読みのモーラ数と合いません")
            new_dict = [x for x in new_dict if x.get("surface") != e["surface"]]
            new_dict.append({"surface": e["surface"], "reading": e["reading"], "accent": e["accent"]})

    new_voice = dict(proj.voice)
    for ln in res.get("lines", []):
        st = ln.get("status") or "hold"
        print(f"  {ln['id']} {label.get(st, st)}：{ln.get('file', '')} #{ln.get('no', '')} {ln['text']}"
              + (f"  メモ：{ln['note']}" if ln.get("note") else ""))
        if st == "ok":
            ET.fromstring(ln["analyzed_text"])  # 壊れていないか
            cur = proj.lines.get(ln["id"])
            if not cur:
                print("    → この行はもう台本にないので書きません")
                continue
            if cur["speaker"] != ln["speaker"] or cur["text"] != ak.tts_text(ln["text"]):
                print("    → 確認したあとで台本の文か話者が変わっているので書きません（案を作り直してください）")
                continue
            new_voice[ln["id"]] = {"sp": cur["speaker"], "text": cur["text"], "analyzed_text": ln["analyzed_text"]}

    n_dict = len([x for x in new_dict if x not in entries])
    n_voice = len([k for k in new_voice if proj.voice.get(k) != new_voice[k]])
    print(f"書き込むもの：辞書 {n_dict} 件、行の手直し {n_voice} 件")
    if not args.write:
        print("（--write を付けると書き込みます）")
        return
    if n_dict:
        print(f"  辞書のバックアップ → {backup_dict(entries, proj.script)}")
        if args.dict_file:
            vt.save_dict(new_dict)
        else:  # AK Studio は辞書のファイルを見張っているので、中継が書けば読み直す
            vt.save_relay_dict(new_dict, args.port)
    if n_voice:
        if proj.voice_file.exists():
            print(f"  _voice.json のバックアップ → {backup(proj.voice_file, proj.script)}")
        ak.save_voice(proj.script, new_voice)
    print("書き込みました。AK Studio が変更を自動で読み込みます（変わった行は AK Studio で作り直してください）")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    b = sub.add_parser("build", help="draft.json から確認画面用の proposal.json を作る")
    b.add_argument("draft")
    b.add_argument("--project", default=".")
    b.add_argument("--cache", action="append", default=[], help="ほかに探す voice_cache")
    a = sub.add_parser("apply", help="確認の結果（result.json）の採用分を書き込む")
    a.add_argument("--project", default=".")
    a.add_argument("--write", action="store_true")
    for p in (b, a):
        p.add_argument("--port", type=int, help="声の中継のポート（無ければ AK_VOICE_PORT・8765）")
        p.add_argument("--dict-file", action="store_true", help="中継を通さず、辞書のファイルを直接読み書きする")
    args = ap.parse_args()
    sys.stdout.reconfigure(errors="replace")
    try:
        build(args) if args.cmd == "build" else apply(args)
    except RuntimeError as e:  # 台本・_voice.json が読めないとき
        sys.exit(str(e))


if __name__ == "__main__":
    main()
