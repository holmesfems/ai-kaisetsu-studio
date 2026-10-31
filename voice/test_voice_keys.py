"""声の鍵（読み上げの文と空白の削り方）が Rust・TS と同じ結果になるかを、共通の表で確かめる。VoiSona は使わない。

    python test_voice_keys.py
"""
import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import akscript as ak  # noqa: E402

TABLE = HERE.parent / "src-tauri" / "tests" / "fixtures" / "voice_keys.json"


def load():
    cases = json.loads(TABLE.read_text(encoding="utf-8"))
    assert len(cases) >= 20
    return cases


def test_table():
    for c in load():
        assert ak.tts_text(c["text"]) == c["tts"], c["name"]
        assert f"{c['sp']}\x01{ak.tts_text(c['text'])}" == c["key"], c["name"]


def test_active_voice():
    # 表の tts を text に持つ手直しは、同じ行で効く。話者が違えば効かない
    for c in load():
        line = {"id": "l1", "sp": c["sp"], "text": c["text"]}
        voice = {"l1": {"sp": c["sp"], "text": c["tts"], "analyzed_text": "<x/>"}}
        assert ak.active_voice(voice, line) == "<x/>", c["name"]
        other = {"l1": {"sp": c["sp"] + "x", "text": c["tts"], "analyzed_text": "<x/>"}}
        assert ak.active_voice(other, line) is None, c["name"]


if __name__ == "__main__":
    n = 0
    for name, fn in list(globals().items()):
        if name.startswith("test_") and callable(fn):
            fn()
            n += 1
    print(f"ok ({n} tests)")
