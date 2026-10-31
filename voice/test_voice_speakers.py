"""回ごとの話者の声（_voice_speakers.json）を重ねる処理のテスト。VoiSona は使わない。

    python test_voice_speakers.py
"""
import json
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import voice_report as vr  # noqa: E402
import voice_server as vs  # noqa: E402
import voisona_tts as vt  # noqa: E402

PRESETS = {"pauses": {"default": 0.2}, "speakers": {
    "トーク1": {"voice_name": "a", "global_parameters": {"speed": 1.29, "pitch": 0, "intonation": 1.0,
                                                        "style_weights": [0.5, 0.5, 0.0, 0.0]}},
    "トーク2": {"voice_name": "b", "global_parameters": {"speed": 1.52, "intonation": 1.12}}}}


def test_clean_global():
    assert vt.clean_global(None) is None
    assert vt.clean_global({}) is None
    assert vt.clean_global({"speed": 9, "pitch": -1000, "x": 1, "volume": "3", "alp": True}) == \
        {"speed": 5.0, "pitch": -600.0}
    assert vt.clean_global({"style_weights": [1.5, -1, 0.25]}, 3) == {"style_weights": [1.0, 0.0, 0.25]}
    assert vt.clean_global({"style_weights": [1, 0]}, 3) is None  # スタイルの数が違えば使わない
    assert vt.clean_global({"style_weights": []}) is None


def test_apply_speakers():
    assert vt.apply_speakers(PRESETS, {}) is PRESETS
    p = vt.apply_speakers(PRESETS, {"トーク1": {"speed": 1.0, "style_weights": [1, 0, 0, 0]},
                                    "トーク3": {"speed": 2}, "トーク2": {"bad": 1}})
    g1 = p["speakers"]["トーク1"]["global_parameters"]
    assert g1 == {"speed": 1.0, "pitch": 0, "intonation": 1.0, "style_weights": [1.0, 0.0, 0.0, 0.0]}
    assert p["speakers"]["トーク2"] is PRESETS["speakers"]["トーク2"]  # 調整の無い話者は元のまま
    assert "トーク3" not in p["speakers"]
    assert PRESETS["speakers"]["トーク1"]["global_parameters"]["speed"] == 1.29  # 元は変えない
    # プリセットと同じ値だけの調整は、重ねない（キャッシュの鍵も変わらない）
    p = vt.apply_speakers(PRESETS, {"トーク1": {"pitch": 0.0, "speed": 1.29}})
    assert p["speakers"]["トーク1"] is PRESETS["speakers"]["トーク1"]


def test_cache_key_and_file():
    with tempfile.TemporaryDirectory() as d:
        vs.PROJECT = Path(d)  # set_project は last_project.txt を書くので使わない
        (Path(d) / "script").mkdir()
        assert vs.speaker_overrides() == {}
        f = Path(d) / "script" / vs.SPEAKERS
        key = lambda p: vs.cache_key("トーク1", "あ", None, p, [])  # noqa: E731
        k0 = key(vt.apply_speakers(PRESETS, vs.speaker_overrides()))
        assert k0 == key(PRESETS)  # 調整が無ければ鍵は今までと同じ
        f.write_text(json.dumps({"トーク1": {"speed": 1.0}}), encoding="utf-8")
        k1 = key(vt.apply_speakers(PRESETS, vs.speaker_overrides()))
        assert k1 != k0  # 調整を変えたら作り直す
        assert key(vt.apply_speakers(PRESETS, {"トーク2": {"speed": 1.0}})) == k0  # ほかの話者の調整は関係ない
        # AK Studio が送る調整は、ファイルより優先する（保存の前に作っても、送った値で作る）
        p, gp = vs.presets_for({"speaker": "トーク1", "speaker_params": {"speed": 2.0}}, PRESETS)
        assert gp["speed"] == 2.0 and key(p) not in (k0, k1)
        p, gp = vs.presets_for({"speaker": "トーク1", "speaker_params": None}, PRESETS)
        assert key(p) == k0 and gp["speed"] == 1.29  # 空の調整は、ファイルがあっても共通のプリセットのまま
        p, gp = vs.presets_for({"speaker": "トーク1"}, PRESETS)
        assert key(p) == k1 and gp["speed"] == 1.0  # 送らなければ（AK Studio）ファイルの調整
        f.write_text('{"トーク1": ', encoding="utf-8")
        try:
            vs.speaker_overrides()
            raise AssertionError("書きかけのファイルはエラーにする")
        except ValueError:
            pass


def test_report_key_matches_relay():
    """話者の声のある回で、voice_report の作る鍵が中継の lookup と同じになる。"""
    with tempfile.TemporaryDirectory() as d:
        sd = Path(d) / "script"
        sd.mkdir()
        assert vr.speaker_overrides(sd) == {}
        (sd / vs.SPEAKERS).write_text(json.dumps({"トーク1": {"speed": 1.0}}), encoding="utf-8")
        item = {"speaker": "トーク1", "speaker_params": {"speed": 1.0}}
        relay, _ = vs.presets_for(item, PRESETS)
        report = vt.apply_speakers(PRESETS, vr.speaker_overrides(sd))
        key = lambda p: vs.cache_key("トーク1", "あ", None, p, [])  # noqa: E731
        assert key(report) == key(relay)
        assert key(report) != key(PRESETS)  # 共通のプリセットのままだと鍵が合わない
        (sd / vs.SPEAKERS).write_text('{"トーク1": ', encoding="utf-8")
        try:
            vr.speaker_overrides(sd)
            raise AssertionError("書きかけのファイルはエラーにする")
        except RuntimeError:
            pass


if __name__ == "__main__":
    for name, fn in list(globals().items()):
        if name.startswith("test_"):
            fn()
            print("ok", name)
