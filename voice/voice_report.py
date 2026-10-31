"""台本の全行について、VoiSona の解析結果（単語・読み・高低・アクセント句）を一覧にする。

辞書の登録案やアクセントの手直し案を考えるための下調べ用。音声は作らず、voice_cache/ にある解析結果を読むだけ。
先に AK Studio の台本タブの「音声を一括作成」で音声を作っておくこと（解析結果は音声を作るときに voice_cache に入る）。台本は script/*.yaml（古い .md の回は AK Studio で開くと移る）。

    python voice_report.py --project E:\\video\\an\\an389_shoheiken            # → <回>/voice_report.md
    python voice_report.py --project ... -o report.md --words             # 単語の一覧（重複なし）も付ける
    python voice_report.py --project ... --cache <別の voice_cache>       # サーバーを別のフォルダで起動していたとき

表記の見方（1行目が `#番号 行の id` と本文、2行目が解析結果）：
    ／ はアクセント句の区切り、・ は同じ句の中の単語の区切り
    読みは高いモーラを ［ ］ で囲む（例：ア［ーク］ナイツ ＝ 低高高低低）
    ⚠ は気になる所（読めなかった言葉、句の中の高低が「低→高→低」になっていない、句の中が全部低い）

どの解析結果を使うかはエディタと同じ（話者＋話者の声（script/_voice_speakers.json）＋読み上げ用の本文＋_voice.json の手直し＋今の辞書で決まるキャッシュ）。
辞書を変えたあとでまだ解析していない行は、同じ本文の一番新しいキャッシュを使い「(古い解析)」と書く。
"""
import argparse
import json
import re
import sys
import xml.etree.ElementTree as ET
from collections import defaultdict
from pathlib import Path

TOOLS = Path(__file__).resolve().parent
sys.path.insert(0, str(TOOLS))
import akscript as ak  # noqa: E402
import voisona_tts as vt  # noqa: E402
from voice_server import cache_key  # noqa: E402

SMALL = set("ァィゥェォャュョヮ")


tts_text = ak.tts_text


def read_blocks(script_dir):
    """台本（script/*.yaml）を読む。akscript.read_blocks と同じ（ほかの道具から vr.read_blocks で呼ぶ用）。"""
    return ak.read_blocks(script_dir)


def speaker_overrides(script_dir):
    """回の話者の声の調整（script/_voice_speakers.json）。無い・空なら {}。読めなければ RuntimeError。
    中継（voice_server.speaker_overrides）と同じ読み方。"""
    path = script_dir / "_voice_speakers.json"
    if not path.exists():
        return {}
    text = path.read_text(encoding="utf-8-sig").strip()
    if not text:
        return {}
    try:
        d = json.loads(text)
    except ValueError:
        raise RuntimeError(f"{path} が読めません（書きかけかもしれません）") from None
    return d if isinstance(d, dict) else {}


def episode_presets(script_dir):
    """共通のプリセットに回の話者の声を重ねたもの（中継の鍵と同じ）。"""
    return vt.apply_speakers(vt.load_presets(), speaker_overrides(script_dir))


def index_cache(cache_dir):
    """(話者, 本文) → 一番新しいキャッシュの中身。辞書を変えた後で未解析の行のため。"""
    latest = {}
    for f in sorted(cache_dir.glob("*.json"), key=lambda f: f.stat().st_mtime):
        try:
            d = json.loads(f.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            continue
        k = (d.get("speaker"), d.get("text"))
        latest[k] = d
    return latest


def morae(pron, n):
    """読み（カタカナ）をモーラに分ける。数が音素と合わなければ None。"""
    out = []
    for c in pron.replace("’", ""):
        if c in SMALL and out:
            out[-1] += c
        else:
            out.append(c)
    return out if len(out) == n else None


def parse(xml):
    """解析結果 → アクセント句のリスト。句は単語 dict のリスト。"""
    phrases = []
    for ap in ET.fromstring(xml).iter("acoustic_phrase"):
        cur = None
        for w in ap.iter("word"):
            text = w.text or w.get("original") or ""
            phon = w.get("phoneme") or ""
            if not phon:  # 記号・読めなかった言葉
                unread = bool(re.search(r"\w", text))
                if cur is not None:
                    cur.append({"text": text, "sym": True, "unread": unread})
                else:
                    phrases.append([{"text": text, "sym": True, "unread": unread}])
                continue
            n = len(phon.split("|"))
            pron = w.get("pronunciation", "")
            hl = w.get("hl") or ""
            word = {"text": text, "sym": False, "pron": pron.replace("’", ""), "hl": hl, "pos": w.get("pos", ""),
                    "mora": morae(pron, n) or phon.split("|"), "chain": w.get("chain") == "1"}
            if word["chain"] and cur is not None:
                cur.append(word)
            else:
                cur = [word]
                phrases.append(cur)
    return phrases


def phrase_problem(ph):
    hl = "".join(w["hl"] for w in ph if not w["sym"])
    if not hl:
        return None
    if not re.fullmatch(r"l*h*l*", hl):
        return f"高低が低→高→低でない({hl})"
    if "h" not in hl and len(hl) > 1:
        return f"句の中が全部低い({hl})"
    return None


def render(phrases):
    parts, notes = [], []
    for ph in phrases:
        if all(w["sym"] for w in ph):
            for w in ph:
                if w["unread"]:
                    notes.append(f"読めない「{w['text']}」")
            parts.append("".join(w["text"] for w in ph))
            continue
        ws = []
        for w in ph:
            if w["sym"]:
                if w["unread"]:
                    notes.append(f"読めない「{w['text']}」")
                ws.append(w["text"])
                continue
            s = ""
            for m, h in zip(w["mora"], w["hl"] or "?" * len(w["mora"])):
                s += f"［{m}］" if h == "h" else m
            ws.append(s.replace("］［", ""))
        p = phrase_problem(ph)
        if p:
            notes.append(p)
        parts.append("・".join(ws))
    return " ／ ".join(parts).replace(" ／ 、", "、").replace(" ／ 。", "。"), notes


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--project", default=".", help="回のフォルダ（script/ と voice_cache/ がある所）")
    ap.add_argument("-o", "--out", help="書き出す .md（省略時は <回>/voice_report.md）")
    ap.add_argument("--words", action="store_true", help="単語の一覧（表記・読み・高低・回数）も付ける")
    ap.add_argument("--cache", action="append", default=[],
                    help="ほかに探す voice_cache（サーバーを別のフォルダで起動していたとき。何回でも指定できる）")
    args = ap.parse_args()
    proj = Path(args.project).resolve()
    script_dir = proj / "script"
    caches = [proj / "voice_cache"] + [Path(c).resolve() for c in args.cache]
    entries = vt.load_dict()
    try:
        presets = episode_presets(script_dir)
        blocks = ak.read_blocks(script_dir)
        voice = ak.load_voice(script_dir)
        params = ak.load_params(script_dir)  # 行ごとの抑揚・音素の長さ（AK Studio の「長さ」タブ）
    except RuntimeError as e:
        sys.exit(str(e))
    latest = {}
    for c in reversed(caches):
        latest.update(index_cache(c))

    out = [f"# 解析一覧：{proj.name}", "",
           "／＝アクセント句の区切り、・＝同じ句の単語の区切り、［ ］＝高いモーラ、⚠＝気になる所", ""]
    words = defaultdict(lambda: [0, set()])
    stats = defaultdict(int)
    for blk in blocks:
        name, title = blk["file"], blk["title"]
        out += [f"## {title}（{name}）", ""]
        for ln in blk["lines"]:
            no, sp, raw = ln["no"], ln["sp"], ln["text"]
            text = tts_text(raw)
            if not text:
                continue
            stats["lines"] += 1
            edited = ak.active_voice(voice, ln)  # 話者と文が今の行と同じ手直しだけ
            d, tag = None, ""
            if sp in presets["speakers"]:
                key = cache_key(sp, text, edited, presets, entries, ak.active_params(params, ln))
                f = next((c / f"{key}.json" for c in caches if (c / f"{key}.json").exists()), None)
                if f:
                    d = json.loads(f.read_text(encoding="utf-8"))
            if d is None and (sp, text) in latest:
                d, tag = latest[(sp, text)], " (古い解析)"
                stats["old"] += 1
            if edited:
                tag += " (手直し済)"
            out.append(f"- **{no}** `{ln['id']}` {sp}{tag}：{raw}")
            if d is None:
                out.append("  - 未解析")
                stats["none"] += 1
                continue
            phrases = parse(edited or d["analyzed_text"])
            line, notes = render(phrases)
            out.append(f"  - {line}")
            if notes:
                out.append(f"  - ⚠ {'、'.join(notes)}")
                stats["warn"] += 1
            for ph in phrases:
                for w in ph:
                    if not w["sym"]:
                        k = (w["text"], w["pron"], w["hl"], w["pos"])
                        words[k][0] += 1
                        words[k][1].add(f"{title}#{no}")
        out.append("")

    if args.words:
        out += ["## 単語の一覧", "", "| 表記 | 読み | 高低 | 品詞 | 回数 | 出てくる行 |", "|---|---|---|---|---|---|"]
        for (t, p, hl, pos), (n, where) in sorted(words.items(), key=lambda kv: (kv[0][0], -kv[1][0])):
            out.append(f"| {t} | {p} | {hl} | {pos} | {n} | {'、'.join(sorted(where)[:4])} |")
        out.append("")

    path = Path(args.out) if args.out else proj / "voice_report.md"
    path.write_text("\n".join(out), encoding="utf-8")
    sys.stdout.reconfigure(errors="replace")
    print(f"{stats['lines']} 行（未解析 {stats['none']}、古い解析 {stats['old']}、⚠ {stats['warn']}）→ {path}")


if __name__ == "__main__":
    main()
