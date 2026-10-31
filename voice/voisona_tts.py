"""VoiSona Talk の REST API で、台本のセリフを音声ファイルにする。

- キャストと声の調整は tools/voisona_presets.json（トーク1＝ささら、トーク2＝つづみ）
- 句読点の間は presets の "pauses"（、0.15秒／。0.2秒／その他0.2秒）に合わせて作り直す
- 認証は環境変数 VOISONA_USER（サインインのメールアドレス）と VOISONA_PASS（API パスワード）

使い方（1行だけ試す）:
    python voisona_tts.py トーク1 "まずは、先鋒です。" out.wav
"""
import base64
import json
import os
import re
import sys
import time
import urllib.error
import urllib.request
import xml.etree.ElementTree as ET
from pathlib import Path

API = os.environ.get("VOISONA_API", "http://localhost:32766/api/talk/v1")
PRESETS = Path(__file__).with_name("voisona_presets.json")


def _auth():
    user, pw = os.environ.get("VOISONA_USER"), os.environ.get("VOISONA_PASS")
    if not user or not pw:
        sys.exit("環境変数 VOISONA_USER と VOISONA_PASS を設定してください")
    return "Basic " + base64.b64encode(f"{user}:{pw}".encode()).decode()


def call(method, path, body=None):
    req = urllib.request.Request(
        API + path, method=method,
        headers={"Authorization": _auth(), "Content-Type": "application/json"},
        data=json.dumps(body, ensure_ascii=False).encode("utf-8") if body is not None else None)
    try:
        with urllib.request.urlopen(req) as r:
            return json.loads(r.read() or b"null")
    except urllib.error.HTTPError as e:
        raise RuntimeError(f"{method} {path} → {e.code} {e.read().decode('utf-8', 'replace')}") from None


def purge():
    """VoiSona に溜まった、終わった依頼を消す（溜まりすぎると新しい依頼が 409 で断られる）。"""
    for it in call("GET", "/speech-syntheses").get("items", []):
        if it.get("state") in ("succeeded", "failed"):
            try:
                call("DELETE", f"/speech-syntheses/{it['uuid']}")
            except RuntimeError:
                pass


def request(body):
    """音声の依頼を出す。キューがいっぱいなら、終わった依頼を消してからやり直す。"""
    for attempt in range(5):
        try:
            return call("POST", "/speech-syntheses", body)["uuid"]
        except RuntimeError as e:
            if "409" not in str(e) or attempt == 4:
                raise
            purge()
            time.sleep(0.5 * (attempt + 1))


def wait(uid, timeout=120):
    end = time.time() + timeout
    try:
        while time.time() < end:
            d = call("GET", f"/speech-syntheses/{uid}")
            if d.get("state") == "succeeded":
                return d
            if d.get("state") == "failed":
                raise RuntimeError(f"音声の生成に失敗しました: {d.get('text')}")
            time.sleep(0.2)
        raise TimeoutError("音声の生成が終わりませんでした")
    finally:
        try:  # 結果は受け取ったので、VoiSona 側の依頼は消しておく
            call("DELETE", f"/speech-syntheses/{uid}")
        except RuntimeError:
            pass


def load_presets():
    return json.load(open(PRESETS, encoding="utf-8"))


def pause_groups(analyzed_text):
    """解析結果から、ポーズになる記号のまとまりを順に取り出す。読めなかった言葉も返す。

    文末の記号（句点など）は、ポーズではなく末尾の無音になるので数に入れない。
    """
    root = ET.fromstring(analyzed_text)
    groups, unread, cur = [], [], ""
    for w in root.iter("word"):
        if w.get("phoneme"):
            if cur:
                groups.append(cur)
                cur = ""
            continue
        orig = w.text or w.get("original") or ""
        cur += orig
        if re.search(r"\w", orig):  # 文字なのに音素が無い＝読めなかった
            unread.append(orig)
    return groups, unread


# ---------- 辞書（表記 → 読み＋アクセント） ----------
def dict_path():
    """辞書の場所。環境変数 AK_VOICE_DICT → 無ければ %LOCALAPPDATA% の ak-studio フォルダの voice_dict.json（呼ぶたびに決める）。"""
    env = os.environ.get("AK_VOICE_DICT")
    if env:
        return Path(env)
    base = os.environ.get("LOCALAPPDATA") or str(Path.home() / "AppData" / "Local")
    return Path(base) / "ak-studio" / "voice_dict.json"


def load_dict():
    try:
        return json.load(open(dict_path(), encoding="utf-8")).get("entries", [])
    except FileNotFoundError:
        return []


def save_dict(entries):
    """一時ファイルに書いてから置き換える（書いている途中のファイルを読まれても、落ちても壊れない）。"""
    p = dict_path()
    p.parent.mkdir(parents=True, exist_ok=True)
    tmp = p.with_name(f".{p.name}.tmp-{os.getpid()}")
    try:
        with open(tmp, "w", encoding="utf-8") as f:
            json.dump({"entries": entries}, f, ensure_ascii=False, indent=1)
        os.replace(tmp, p)
    finally:
        if tmp.exists():
            tmp.unlink()


def kata(s):
    """ひらがなをカタカナにし、無声化の記号などを除く。"""
    return re.sub(r"[^ァ-ヴー]", "", "".join(chr(ord(c) + 0x60) if "ぁ" <= c <= "ゖ" else c for c in s))


def relevant(entries, text):
    return [e for e in entries if e.get("surface") and e.get("reading") and e["surface"] in text]


def apply_reading(text, entries):
    """表記を読み（カタカナ）に置き換えてから VoiSona に渡す。長い表記から順に。"""
    for e in sorted(entries, key=lambda e: -len(e["surface"])):
        text = text.replace(e["surface"], kata(e["reading"]))
    return text


ACCENT_RULE = 2  # apply_accent の当て方の版（変えたら上げる。辞書が関わる行のキャッシュが作り直しになる）


def apply_accent(xml, entries):
    """読みが辞書と一致する単語の並びを1語にまとめ、辞書のアクセント（hl）にする。"""
    targets = [(kata(e["reading"]), e) for e in entries if e.get("accent")]
    if not targets:
        return xml
    root = ET.fromstring(xml)
    changed = False
    for ph in root.iter("acoustic_phrase"):
        words = list(ph)
        i = 0
        while i < len(words):
            hit = None
            for reading, e in sorted(targets, key=lambda t: -len(t[0])):
                acc, j = "", i
                while j < len(words) and words[j].get("phoneme") and len(acc) < len(reading):
                    acc += kata(words[j].get("pronunciation", ""))
                    j += 1
                if acc == reading:
                    hit = (j, e)
                    break
            if not hit:
                i += 1
                continue
            j, e = hit
            run = words[i:j]
            phon = "|".join(w.get("phoneme") for w in run)
            if len(phon.split("|")) != len(e["accent"]):
                i = j
                continue
            first = run[0]
            first.text = "".join(w.text or "" for w in run)
            first.set("original", first.text)
            first.set("pronunciation", "".join(w.get("pronunciation", "") for w in run))
            first.set("phoneme", phon)
            first.set("hl", e["accent"])
            first.set("pos", "名詞")
            for k in ("cform", "ctype", "pos_group1", "pos_group2", "pos_group3"):
                first.attrib.pop(k, None)
            for w in run[1:]:
                ph.remove(w)
            words = list(ph)
            changed = True
            i += 1
            if "hl" in e["accent"]:  # 途中で下がるなら、同じ句の後ろの単語（助詞など）は低いまま
                for w in words[i:]:
                    if not w.get("phoneme"):
                        continue
                    if w.get("chain") != "1":
                        break
                    w.set("hl", "l" * len(w.get("phoneme").split("|")))
    return ET.tostring(root, encoding="unicode") if changed else xml


def clean_params(params):
    """行ごとの調整（抑揚・音素の長さ）を、決まった形にそろえる。何も無ければ None。

    {"intonation": 数 or なし, "durations": {"n": 音素の数, "set": {"音素の番号": 秒}}}
    """
    params = params or {}
    out = {}
    if params.get("intonation") is not None:
        out["intonation"] = round(min(2.0, max(0.0, float(params["intonation"]))), 3)
    dur = params.get("durations") or {}
    if dur.get("set"):
        out["durations"] = {"n": int(dur["n"]), "set": {str(int(k)): round(min(10.0, max(0.005, float(v))), 4)
                                                       for k, v in sorted(dur["set"].items(), key=lambda kv: int(kv[0]))}}
    return out or None


# 話者ごとの声（global_parameters）の範囲。VoiSona Talk が受け付ける範囲（外の値は VoiSona が端に丸める）
GLOBAL_RANGE = {"speed": (0.2, 5.0), "volume": (-8.0, 8.0), "pitch": (-600.0, 600.0),
                "intonation": (0.0, 2.0), "alp": (-1.0, 1.0), "huskiness": (-20.0, 20.0)}


def clean_global(over, n_styles=None):
    """回ごとの話者の声の調整（_voice_speakers.json の 1 人分）を、決まった形にそろえる。何も無ければ None。

    {"speed": 数, ..., "style_weights": [0〜1 の数 × スタイルの数]}。知らない項目・数でない値は捨てる。
    スタイルの重みは、数が n_styles と違うとき（声のライブラリが変わったときなど）は使わない。
    """
    out = {}
    for k, (lo, hi) in GLOBAL_RANGE.items():
        v = (over or {}).get(k)
        if isinstance(v, (int, float)) and not isinstance(v, bool):
            out[k] = round(min(hi, max(lo, float(v))), 3)
    w = (over or {}).get("style_weights")
    if isinstance(w, list) and w and all(isinstance(x, (int, float)) and not isinstance(x, bool) for x in w) \
            and (n_styles is None or len(w) == n_styles):
        out["style_weights"] = [round(min(1.0, max(0.0, float(x))), 3) for x in w]
    return out or None


def apply_speakers(presets, over):
    """プリセットに、回ごとの話者の声の調整を重ねる（元の presets は変えない）。

    調整の無い話者は元のまま（音声キャッシュの鍵も変わらない）。
    """
    if not over:
        return presets
    speakers = dict(presets["speakers"])
    for sp, o in over.items():
        base = speakers.get(sp)
        if not base or not isinstance(o, dict):
            continue
        gp = base.get("global_parameters", {})
        n = len(gp["style_weights"]) if isinstance(gp.get("style_weights"), list) else None
        # プリセットと同じ値（0 と 0.0 など）は重ねない（音声キャッシュの鍵が変わって作り直しになるので）
        g = {k: v for k, v in (clean_global(o, n) or {}).items() if v != gp.get(k)}
        if g:
            speakers[sp] = dict(base, global_parameters={**gp, **g})
    return dict(presets, speakers=speakers)


def synthesize(speaker, text, out_path, presets=None, analyzed_text=None, entries=None, params=None):
    """1行を音声にする。

    analyzed_text を渡すと、その解析結果（アクセントを手直ししたものなど）で作る。
    entries（辞書）を渡すと、読みとアクセントを当てはめる（analyzed_text があるときは読みの置き換えだけ）。
    params（clean_params の形）を渡すと、抑揚（intonation）と音素ごとの長さを変える。
      音素の長さは、音素の数が n と同じときだけ使う（読みが変わって音素の並びがずれたら使わない）。
    戻り値：{"duration": 秒, "unread": 読めなかった言葉, "analyzed_text": 使った解析結果,
            "durations_ignored": 長さの調整を使わなかったか, ...}
    """
    presets = presets or load_presets()
    p = presets["speakers"][speaker]
    pauses = presets["pauses"]
    entries = relevant(entries or [], text)
    params = clean_params(params) or {}
    gp = dict(p["global_parameters"])
    if "intonation" in params:
        gp["intonation"] = params["intonation"]
    base = {"language": "ja_JP", "text": apply_reading(text, entries), "voice_name": p["voice_name"],
            "global_parameters": gp, "destination": "file",
            "output_file_path": str(Path(out_path).resolve()), "can_overwrite_file": True}
    if analyzed_text:
        base["analyzed_text"] = analyzed_text

    # 1回目：解析と、既定の音素の長さを得る
    d = wait(request(base))
    used = d["analyzed_text"]
    if not analyzed_text and entries:  # 辞書のアクセントを当てはめて作り直す
        fixed = apply_accent(used, entries)
        if fixed != used:
            used = fixed
            d = wait(request(dict(base, analyzed_text=used)))
            used = d["analyzed_text"]
    phonemes, durs = d["phonemes"], list(d["phoneme_durations"])
    groups, unread = pause_groups(used)
    idx = [i for i, ph in enumerate(phonemes) if ph == "pau"]
    dur = params.get("durations")
    own = {int(k): v for k, v in dur["set"].items()} if dur and dur["n"] == len(phonemes) else {}

    def result(d):
        return {"duration": d["duration"], "unread": unread, "analyzed_text": used,
                "phonemes": d["phonemes"], "phoneme_durations": d["phoneme_durations"],
                "durations_ignored": bool(dur) and not own}

    if not idx and not own:
        return result(d)

    # 2回目：句読点ごとのポーズの長さに置き換えて作り直す
    def length(g):
        if "。" in g:
            return pauses.get("。", pauses["default"])
        for ch, sec in pauses.items():
            if ch != "default" and ch in g:
                return sec
        return pauses["default"]

    if len(groups) == len(idx):
        for i, g in zip(idx, groups):
            durs[i] = length(g)
    else:  # 数が合わないときは、読点の長さでそろえる
        for i in idx:
            durs[i] = pauses.get("、", pauses["default"])
    for i, sec in own.items():  # 手で決めた長さ（句読点の間も含む）が一番強い
        if i < len(durs):
            durs[i] = sec
    d = wait(request(dict(base, analyzed_text=used, phoneme_durations=durs)))
    return result(d)


if __name__ == "__main__":
    sys.stdout.reconfigure(errors="replace")
    if len(sys.argv) != 4:
        sys.exit(__doc__)
    r = synthesize(sys.argv[1], sys.argv[2], sys.argv[3])
    print(f"{r['duration']:.2f} 秒 → {sys.argv[3]}")
    if r["unread"]:
        print("※ 読めなかった言葉:", "、".join(r["unread"]))
