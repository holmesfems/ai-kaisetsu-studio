"""声のエンジンで共通に使う部品（辞書・プリセット・話者の声の調整・WAV の長さ）。標準ライブラリだけ。

エンジンのモジュール（voisona_tts.py など）と中継（voice_server.py）が使う。
エンジンの約束 v1 は docs/spec/issues-8-12.md（のちに docs/voice-engine.md）を見る。
"""
import json
import os
import re
import wave
from pathlib import Path

__all__ = ["dict_path", "load_dict", "save_dict", "kata", "relevant", "apply_reading", "load_presets",
           "clean_params", "clean_global", "apply_speakers", "wav_duration"]


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
    """表記を読み（カタカナ）に置き換えてから声のエンジンに渡す。長い表記から順に。"""
    for e in sorted(entries, key=lambda e: -len(e["surface"])):
        text = text.replace(e["surface"], kata(e["reading"]))
    return text


# ---------- プリセット・話者の声 ----------
def load_presets(path):
    """話者のプリセット（エンジンの PRESETS のファイル）を読む。"""
    with open(path, encoding="utf-8") as f:
        return json.load(f)


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


def clean_global(over, n_styles=None, ranges=None):
    """回ごとの話者の声の調整（_voice_speakers.json の 1 人分）を、決まった形にそろえる。何も無ければ None。

    {"speed": 数, ..., "style_weights": [0〜1 の数 × スタイルの数]}。ranges（エンジンの RANGES）に無い項目・数でない値は捨てる。
    スタイルの重みは、数が n_styles と違うとき（声のライブラリが変わったときなど）は使わない。
    """
    out = {}
    for k, (lo, hi) in (ranges or {}).items():
        v = (over or {}).get(k)
        if isinstance(v, (int, float)) and not isinstance(v, bool):
            out[k] = round(min(hi, max(lo, float(v))), 3)
    w = (over or {}).get("style_weights")
    if isinstance(w, list) and w and all(isinstance(x, (int, float)) and not isinstance(x, bool) for x in w) \
            and (n_styles is None or len(w) == n_styles):
        out["style_weights"] = [round(min(1.0, max(0.0, float(x))), 3) for x in w]
    return out or None


def apply_speakers(presets, over, ranges=None):
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
        g = {k: v for k, v in (clean_global(o, n, ranges) or {}).items() if v != gp.get(k)}
        if g:
            speakers[sp] = dict(base, global_parameters={**gp, **g})
    return dict(presets, speakers=speakers)


def wav_duration(path):
    """WAV の長さ（秒）。"""
    with wave.open(str(path)) as w:
        return w.getnframes() / w.getframerate()
