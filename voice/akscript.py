"""台本（script/*.yaml、v2 の形式）と、行ごとの声の調整（_voice.json・_voice_params.json）を読む共通の部品。

形式は E:\\video\\an\\an_template\\studio\\docs\\script-format.md を見る。voice_report.py・voice_review.py はこれを使う。
    python akscript.py --selftest      動作確認（一時フォルダで試す）
"""
import json
import os
import re
import sys
from pathlib import Path

import yaml

RUBY = re.compile(r"｜([^｜《》]+)《([^《》]+)》")
LEGACY_MSG = "この回の台本は古い形式（.md）です。AK Studio でこの回を開くと新しい形式に移ります"


# 前後の空白の削り方は Rust の str::trim（char::is_whitespace）に合わせる。str.strip() は U+001C〜U+001F も削るので使わない
RS_WS = ("\t\n\x0b\x0c\r \u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a"
         "\u2028\u2029\u202f\u205f\u3000")


def tts_text(t):
    """読み上げ用の本文（ルビ → 読み）を前後の空白で削ったもの。手直しの text・声の鍵（Rust の tts_key_text と同じ）。"""
    return RUBY.sub(r"\2", t).strip(RS_WS)


def _natural(s):
    return [int(x) if x.isdigit() else x for x in re.split(r"(\d+)", s)]


def _script_files(script_dir, ext):
    return [p.name for p in script_dir.glob("*" + ext) if not p.name.startswith("_")]


def _s(v):
    return "" if v is None else str(v)


def read_blocks(script_dir):
    """_order.json の順（そのあと残りの *.yaml を自然順）に台本を読む。

    [{"file", "title", "notes", "lines": [{"id", "no"（ブロック内で 1 から）, "sp", "text", "start", "memo"}]}]
    """
    script_dir = Path(script_dir)
    present = _script_files(script_dir, ".yaml")
    if not present:
        if _script_files(script_dir, ".md"):
            raise RuntimeError(LEGACY_MSG)
        return []
    order_f = script_dir / "_order.json"
    names = json.loads(order_f.read_text(encoding="utf-8")) if order_f.exists() else []
    names = [n for n in names if isinstance(n, str)]
    names += sorted((n for n in present if n not in names), key=_natural)
    blocks, seen = [], {}
    for name in names:
        f = script_dir / name
        if name.startswith("_") or not name.endswith(".yaml") or not f.exists():
            continue
        try:
            doc = yaml.safe_load(f.read_text(encoding="utf-8")) or {}
        except yaml.YAMLError as e:
            raise RuntimeError(f"台本 {name} を YAML として読めません：{e}") from e
        if not isinstance(doc, dict):
            raise RuntimeError(f"台本 {name} の形が違います（title / notes / lines のある辞書にしてください）")
        lines = []
        for i, raw in enumerate(doc.get("lines") or [], 1):
            if not isinstance(raw, dict):
                raise RuntimeError(f"台本 {name} の {i} 行目の形が違います")
            lid = _s(raw.get("id"))
            if not lid:
                raise RuntimeError(f"台本 {name} の {i} 行目に id がありません（AK Studio で開くと付きます）")
            if lid in seen:
                raise RuntimeError(f"id「{lid}」が重なっています：{seen[lid]} と {name}（AK Studio で開くと直ります）")
            seen[lid] = name
            st = raw.get("start")
            lines.append({"id": lid, "no": i, "sp": _s(raw.get("sp")), "text": _s(raw.get("text")),
                          "start": float(st) if isinstance(st, (int, float)) and not isinstance(st, bool) else None,
                          "memo": _s(raw.get("memo"))})
        blocks.append({"file": name, "title": _s(doc.get("title")) or name, "notes": _s(doc.get("notes")),
                       "lines": lines})
    return blocks


def _load_json(path, what):
    if not path.exists():
        return {}
    try:
        d = json.loads(path.read_text(encoding="utf-8"))
    except ValueError as e:  # 書きかけのファイルを空として扱わない
        raise RuntimeError(f"{path.name} を JSON として読めません（書き込み中かもしれません）：{e}") from e
    if not isinstance(d, dict):
        raise RuntimeError(f"{path.name} の形が違います")
    for k, v in d.items():
        if "\u0001" in k or not isinstance(v, dict):
            raise RuntimeError(f"{path.name} は古い形です（{what}）。AK Studio でこの回を開くと新しい形式に移ります")
    return d


def load_voice(script_dir):
    """_voice.json → {行の id: {"sp", "text", "analyzed_text"}}。壊れていれば例外。"""
    return _load_json(Path(script_dir) / "_voice.json", "アクセントの手直し")


def load_params(script_dir):
    """_voice_params.json → {行の id: {"sp", "text", "params"}}。壊れていれば例外。"""
    return _load_json(Path(script_dir) / "_voice_params.json", "抑揚・音素の長さ")


def _active(entry, line):
    if isinstance(entry, dict) and entry.get("sp") == line["sp"] and entry.get("text") == tts_text(line["text"]):
        return entry
    return None


def active_voice(voice, line):
    """その行に今効いているアクセントの手直し（analyzed_text）。話者か文が違えば None。"""
    e = _active(voice.get(line["id"]), line)
    return (e.get("analyzed_text") or None) if e else None


def active_params(params, line):
    """その行に今効いている抑揚・音素の長さ（params）。話者か文が違えば None。"""
    e = _active(params.get(line["id"]), line)
    return (e.get("params") or None) if e else None


def _atomic_write(path, text):
    tmp = path.with_name(path.name + ".tmp")
    tmp.write_text(text, encoding="utf-8")
    os.replace(tmp, path)


def save_voice(script_dir, voice):
    """_voice.json を新しい形で書く（一時ファイルに書いてから置き換える）。"""
    _atomic_write(Path(script_dir) / "_voice.json", json.dumps(voice, ensure_ascii=False))


def selftest():
    import tempfile
    with tempfile.TemporaryDirectory() as td:
        sd = Path(td)
        # 古い形式だけ → エラー
        (sd / "a.md").write_text("# a\n| # | セリフ | 話者 |\n|---|---|---|\n| 1 | x | y |\n", encoding="utf-8")
        try:
            read_blocks(sd)
            raise AssertionError("legacy md should raise")
        except RuntimeError as e:
            assert "古い形式" in str(e)
        (sd / "a.md").unlink()
        (sd / "b2.yaml").write_text('title: 二つ目\nlines:\n  - id: l3b\n    sp: トーク1\n    text: "yes"\n', encoding="utf-8")
        (sd / "b10.yaml").write_text("title: 十\nlines: []\n", encoding="utf-8")
        (sd / "a.yaml").write_text(
            "title: 一つ目\nnotes: |-\n  メモ1\n  メモ2\nlines:\n"
            "  - id: l1a\n    sp: トーク2\n    start: 209.45\n    text: \"1:20\"\n    memo: |-\n      一行目\n      二行目\n"
            "  - id: l2a\n    sp: トーク1\n    text: ｜銀杏《いちょう》ハル。\n", encoding="utf-8")
        (sd / "_order.json").write_text(json.dumps(["a.yaml"]), encoding="utf-8")
        bl = read_blocks(sd)
        assert [b["file"] for b in bl] == ["a.yaml", "b2.yaml", "b10.yaml"], bl
        a = bl[0]
        assert a["notes"] == "メモ1\nメモ2" and a["lines"][0]["memo"] == "一行目\n二行目"
        assert a["lines"][0]["text"] == "1:20" and a["lines"][0]["start"] == 209.45 and a["lines"][1]["start"] is None
        assert bl[1]["lines"][0]["text"] == "yes" and bl[1]["lines"][0]["no"] == 1
        assert [l["no"] for l in a["lines"]] == [1, 2]
        l2 = a["lines"][1]
        assert tts_text(l2["text"]) == "いちょうハル。"
        # 声の調整：効く／効かない
        assert load_voice(sd) == {} and load_params(sd) == {}
        save_voice(sd, {"l2a": {"sp": "トーク1", "text": "いちょうハル。", "analyzed_text": "<x/>"},
                        "l1a": {"sp": "トーク2", "text": "古い文", "analyzed_text": "<old/>"}})
        v = load_voice(sd)
        assert active_voice(v, l2) == "<x/>"
        assert active_voice(v, a["lines"][0]) is None  # 文が違う → 使わない（ファイルには残る）
        assert "l1a" in v and not list(sd.glob("*.tmp"))
        (sd / "_voice_params.json").write_text(
            json.dumps({"l2a": {"sp": "トーク1", "text": "いちょうハル。", "params": {"intonation": 1.2}},
                        "l1a": {"sp": "トーク1", "text": "1:20", "params": {"intonation": 2}}}), encoding="utf-8")
        p = load_params(sd)
        assert active_params(p, l2) == {"intonation": 1.2}
        assert active_params(p, a["lines"][0]) is None  # 話者が違う
        # 壊れた JSON は例外
        (sd / "_voice.json").write_text('{"l2a": {"sp": ', encoding="utf-8")
        try:
            load_voice(sd)
            raise AssertionError("broken json should raise")
        except RuntimeError:
            pass
        # 古い形の鍵
        (sd / "_voice.json").write_text(json.dumps({"トーク1\u0001文": "<x/>"}), encoding="utf-8")
        try:
            load_voice(sd)
            raise AssertionError("legacy voice should raise")
        except RuntimeError:
            pass
    print("selftest OK")


if __name__ == "__main__":
    sys.stdout.reconfigure(errors="replace")
    if "--selftest" in sys.argv:
        selftest()
    else:
        print(__doc__)
