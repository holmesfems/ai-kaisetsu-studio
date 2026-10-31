"""VOICEVOX（ローカルの VOICEVOX ENGINE の HTTP API）で、台本のセリフを音声ファイルにする。声のエンジンの約束 v1。標準ライブラリだけ。

- 話者のプリセットは voicevox_presets.json（トーク1＝ずんだもん、トーク2＝四国めたん。style_id で声のスタイルを選ぶ）
- 場所は環境変数 VOICEVOX_URL（無ければ http://127.0.0.1:50021）
- できること: 行ごとの抑揚（intonation）。アクセントの手直し・音素の長さ・スタイルの混ぜ具合は使わない
- VOICEVOX は声ごとに利用規約があり、動画に「VOICEVOX:ずんだもん」などの表記が要る
"""
import json
import os
import sys
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from voicecommon import apply_reading, clean_params, relevant, wav_duration  # noqa: E402

# ---------- 声のエンジンの約束 v1（voice_server.py が読む） ----------
ENGINE_API = 1
NAME = "voicevox"
LABEL = "VOICEVOX"
FEATURES = {"intonation"}
PRESETS = "voicevox_presets.json"  # 話者のプリセット（このファイルと同じフォルダ）
RANGES = {"speed": (0.5, 2.0), "pitch": (-0.15, 0.15), "intonation": (0.0, 2.0), "volume": (0.0, 2.0)}
# 話者の声の項目 → AudioQuery の項目
_SCALE = {"speed": "speedScale", "pitch": "pitchScale", "intonation": "intonationScale", "volume": "volumeScale"}


def base_url():
    """呼ぶたびに決める（環境変数を後から変えても効く）。"""
    return (os.environ.get("VOICEVOX_URL") or "http://127.0.0.1:50021").rstrip("/")


def _call(method, path, body=None, timeout=120):
    """VOICEVOX を呼んで、返りのバイト列を返す。失敗は RuntimeError。"""
    data = json.dumps(body, ensure_ascii=False).encode("utf-8") if body is not None else (b"" if method == "POST" else None)
    headers = {"Content-Type": "application/json"} if body is not None else {}
    req = urllib.request.Request(base_url() + path, method=method, data=data, headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return r.read()
    except urllib.error.HTTPError as e:
        raise RuntimeError(f"VOICEVOX: {method} {path.split('?')[0]} → {e.code} {e.read().decode('utf-8', 'replace')}") from None
    except (urllib.error.URLError, OSError) as e:
        raise RuntimeError(f"VOICEVOX: {method} {path.split('?')[0]} につながりません（{base_url()}）: {e}") from None


def check():
    """起動時に 1 回。つながらなければ RuntimeError。"""
    try:
        _call("GET", "/version", timeout=5)
    except RuntimeError as e:
        raise RuntimeError(f"VOICEVOX につながりません（{base_url()}）。VOICEVOX を起動してください（{e}）") from None


def voice_info(preset):
    """スタイルは話者のプリセットの style_id で 1 つ選ぶので、声の情報は無い。"""
    return {}


def synthesize(speaker, text, out_path, presets, analyzed_text=None, entries=None, params=None):
    """1 行を 16bit モノラルの WAV にして out_path に書く（一時ファイル → rename）。"""
    p = presets["speakers"][speaker]
    pauses = presets["pauses"]
    style = int(p["style_id"])
    entries = relevant(entries or [], text)
    params = clean_params(params) or {}
    gp = dict(p.get("global_parameters", {}))
    if "intonation" in params:
        gp["intonation"] = params["intonation"]

    q = urllib.parse.urlencode({"text": apply_reading(text, entries), "speaker": style})
    query = json.loads(_call("POST", f"/audio_query?{q}"))
    for key, scale in _SCALE.items():
        if gp.get(key) is not None:
            query[scale] = float(gp[key])
    # 句読点の間: 読点などの後ろの無音（pause_mora）を、プリセットの「、」の長さにそろえる
    pause = pauses.get("、", pauses.get("default", 0.2))
    for ap in query.get("accent_phrases") or []:
        if ap.get("pause_mora"):
            ap["pause_mora"]["vowel_length"] = pause

    wav = _call("POST", f"/synthesis?speaker={style}", query)
    out = Path(out_path)
    out.parent.mkdir(parents=True, exist_ok=True)
    tmp = out.with_name(f".{out.name}.tmp-{os.getpid()}")
    try:
        tmp.write_bytes(wav)
        os.replace(tmp, out)
    finally:
        if tmp.exists():
            tmp.unlink()
    return {"duration": wav_duration(out), "unread": [], "analyzed_text": "", "phonemes": None,
            "phoneme_durations": None, "durations_ignored": False}
