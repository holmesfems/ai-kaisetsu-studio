"""声のエンジンの約束 v1（voice_server.load_engine・effective・cache_key・ping）のテスト。声のエンジンは使わない。

    python test_voice_engine.py
"""
import json
import sys
import tempfile
import wave
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import voice_server as vs  # noqa: E402

PRESETS = {"pauses": {"default": 0.2, "、": 0.15}, "speakers": {
    "トーク1": {"voice_name": "a", "global_parameters": {"speed": 1.29, "intonation": 1.0}}}}

FAKE = '''
import wave
ENGINE_API = {api}
NAME = "fake"
LABEL = "偽のエンジン"
FEATURES = set()
PRESETS = "fake_presets.json"
RANGES = {{"speed": (0.5, 2.0)}}
CALLS = []

def check():
    pass

def voice_info(preset):
    return {{}}

def synthesize(speaker, text, out_path, presets, analyzed_text, entries, params):
    CALLS.append(dict(speaker=speaker, text=text, analyzed_text=analyzed_text, entries=entries, params=params))
    with wave.open(str(out_path), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(16000)
        w.writeframes(bytes(16000))
    return {{"duration": 0.5, "unread": [], "analyzed_text": "", "phonemes": None,
            "phoneme_durations": None, "durations_ignored": False}}
'''


def fail_reason(fn):
    """sys.exit(2) で止まることを確かめて、stderr に出た理由を返す。"""
    import contextlib
    import io
    err = io.StringIO()
    with contextlib.redirect_stderr(err):
        try:
            fn()
        except SystemExit as e:
            assert e.code == 2, e.code
            return err.getvalue()
    raise AssertionError("止まらなかった")


class Swap:
    """テストの間だけ、中継の ENGINE・プロジェクトを差し替える。"""

    def __enter__(self):
        self.saved = (vs.ENGINE, vs.ENGINE_DIR, vs.ENGINE_PATH, vs.PROJECT, vs.CACHE, vs.OUT)
        return self

    def __exit__(self, *a):
        vs.ENGINE, vs.ENGINE_DIR, vs.ENGINE_PATH, vs.PROJECT, vs.CACHE, vs.OUT = self.saved


def write_engine(folder, api=1, presets=True, body=None):
    py = Path(folder) / "fake_tts.py"
    py.write_text(body or FAKE.format(api=api), encoding="utf-8")
    if presets:
        (Path(folder) / "fake_presets.json").write_text(json.dumps(PRESETS), encoding="utf-8")
    return py


def test_load_voisona():
    with Swap():
        m = vs.load_engine("voisona")
        assert m.NAME == "voisona" and m.ENGINE_API == 1 and m.PRESETS == "voisona_presets.json"
        assert m.FEATURES == {"accent", "durations", "intonation", "styles"}
        assert m.RANGES is m.GLOBAL_RANGE
        assert vs.ENGINE is m and vs.ENGINE_PATH is None
        assert set(vs.load_presets()["speakers"]) >= {"トーク1", "トーク2"}


def test_load_engine_failures():
    with tempfile.TemporaryDirectory() as d, Swap():
        # 入口の欠けたエンジン
        py = write_engine(d, body="ENGINE_API = 1\nNAME = 'x'\n")
        assert "約束の入口がありません" in fail_reason(lambda: vs.load_engine(str(py)))
        # 約束の版が違う
        py = write_engine(d, api=2)
        assert "ENGINE_API=2" in fail_reason(lambda: vs.load_engine(str(py)))
        # プリセットが無い
        py = write_engine(Path(d), presets=False)
        (Path(d) / "fake_presets.json").unlink(missing_ok=True)
        assert "fake_presets.json" in fail_reason(lambda: vs.load_engine(str(py)))
        # ファイルが無い・名前が変・知らない名前
        assert "ファイルがありません" in fail_reason(lambda: vs.load_engine(str(Path(d) / "none.py")))
        assert "正しくありません" in fail_reason(lambda: vs.load_engine("../x"))
        assert "import できません" in fail_reason(lambda: vs.load_engine("nonexistent_engine"))
        # 失敗しても、読んでいたエンジンはそのまま
        assert vs.ENGINE.NAME == "voisona"


def test_load_engine_path_and_ping():
    with tempfile.TemporaryDirectory() as d, Swap():
        py = write_engine(d)
        vs.load_engine(str(py))
        assert vs.ENGINE.NAME == "fake" and vs.ENGINE_DIR == Path(d).resolve()
        proj = Path(d) / "ep"
        (proj / "script").mkdir(parents=True)
        vs.PROJECT = proj
        info = vs.ping()["engine"]
        assert info == {"name": "fake", "label": "偽のエンジン", "features": [], "api": 1, "path": str(py.resolve())}
        assert vs.speakers_info()["range"] == {"speed": (0.5, 2.0)}
        vs.load_engine("voisona")
        info = vs.ping()["engine"]
        assert info["name"] == "voisona" and "path" not in info and info["api"] == 1
        assert info["features"] == ["accent", "durations", "intonation", "styles"]


def test_effective_and_synth_drop_unsupported():
    with tempfile.TemporaryDirectory() as d, Swap():
        py = write_engine(d)
        mod = vs.load_engine(str(py))
        proj = Path(d) / "ep"
        (proj / "script").mkdir(parents=True)
        vs.set_project(proj)
        body = {"speaker": "トーク1", "text": "あ", "analyzed_text": "<x/>",
                "params": {"intonation": 1.4, "durations": {"n": 2, "set": {"0": 0.1}}}}
        assert vs.effective(body) == (None, None)
        body["speaker_params"] = {"speed": 1.0, "style_weights": [1, 0]}
        r = vs.synth(body)
        call = mod.CALLS[-1]
        assert call["analyzed_text"] is None and call["params"] is None
        assert r["duration"] == 0.5 and (vs.CACHE / f"{r['id']}.wav").exists()
        # 作ったものは lookup（同じ effective）で見つかる
        out = vs.lookup({"items": [body]})["results"]
        assert out[0] and out[0]["id"] == r["id"]
        # 2 回目はキャッシュ（エンジンを呼ばない）
        n = len(mod.CALLS)
        vs.synth(body)
        assert len(mod.CALLS) == n
        # エンジンの名前が鍵に入る
        P = vs.load_presets()
        k_fake = vs.cache_key("トーク1", "あ", None, P, [])
        vs.load_engine("voisona")
        assert vs.cache_key("トーク1", "あ", None, P, []) != k_fake
        # styles が無いエンジンでは style_weights の調整は重ねない
        vs.load_engine(str(py))
        p = vs.apply_speakers(P, {"トーク1": {"style_weights": [1, 0]}})
        assert p["speakers"] == P["speakers"]
    # すべて使えるエンジンでは、そのまま通る
    with Swap():
        vs.load_engine("voisona")
        body = {"analyzed_text": "<x/>", "params": {"intonation": 1.4, "durations": {"n": 2, "set": {"0": 0.1}}}}
        assert vs.effective(body) == ("<x/>", body["params"])
        assert vs.effective({}) == (None, None)


def test_voisona_cache_key_pinned():
    """VoiSona の鍵は、この分け方の前の実装で出した値のまま（今の声のキャッシュをそのまま使うため）。"""
    with Swap():
        vs.load_engine("voisona")
        P = {"pauses": {"default": 0.2, "、": 0.15}, "speakers": {
            "トーク1": {"voice_name": "a", "global_parameters": {"speed": 1.29, "intonation": 1.0}}}}
        E = [{"surface": "AK", "reading": "エーケー"}]
        assert vs.cache_key("トーク1", "あ", None, P, []) == "bcfb45502110e849"
        assert vs.cache_key("トーク1", "AKです", None, P, E) == "e10d42c23e0a6d9f"
        params = {"intonation": 1.3, "durations": {"n": 3, "set": {"1": 0.1}}}
        assert vs.cache_key("トーク1", "AKです", "<x/>", P, E, params) == "190a641e17fee46a"


def test_check_raises_runtime_error():
    import os
    import voisona_tts as vt
    saved = {k: os.environ.pop(k, None) for k in ("VOISONA_USER", "VOISONA_PASS")}
    try:
        try:
            vt.check()
        except RuntimeError as e:
            assert "VOISONA_USER" in str(e)
        else:
            raise AssertionError("RuntimeError にならなかった")
    finally:
        for k, v in saved.items():
            if v is not None:
                os.environ[k] = v


def test_common_ranges():
    import voicecommon as vc
    r = {"speed": (0.5, 2.0)}
    assert vc.clean_global({"speed": 9, "pitch": 3}, None, r) == {"speed": 2.0}
    assert vc.clean_global({"speed": 9}) is None  # 範囲を渡さなければ項目は無い
    d = Path(tempfile.mkdtemp()) / "a.wav"
    with wave.open(str(d), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(8000)
        w.writeframes(bytes(8000))
    assert vc.wav_duration(d) == 0.5


if __name__ == "__main__":
    for name, fn in list(globals().items()):
        if name.startswith("test_") and callable(fn):
            fn()
            print("OK", name)
