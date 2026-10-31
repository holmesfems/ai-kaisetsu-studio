"""VOICEVOX の声のエンジン（voicevox_tts.py）のテスト。偽の VOICEVOX（http.server）を立てる。本物は使わない。

    python test_voicevox.py
"""
import io
import json
import os
import socket
import sys
import tempfile
import threading
import wave
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlparse

sys.path.insert(0, str(Path(__file__).resolve().parent))
import voice_server as vs  # noqa: E402
import voicevox_tts as vv  # noqa: E402

LOG = []  # 偽のサーバーが受けた要求


def wav_bytes(sec=0.5, rate=24000):
    b = io.BytesIO()
    with wave.open(b, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(rate)
        w.writeframes(bytes(int(rate * sec) * 2))
    return b.getvalue()


class Fake(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def _send(self, code, body, ctype="application/json"):
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _body(self):
        return self.rfile.read(int(self.headers.get("Content-Length") or 0))

    def do_GET(self):
        LOG.append(("GET", self.path, None))
        if self.path == "/version":
            self._send(200, json.dumps("0.99.0").encode())
        else:
            self._send(404, b"{}")

    def do_POST(self):
        u = urlparse(self.path)
        q = {k: v[0] for k, v in parse_qs(u.query).items()}
        body = self._body()
        if u.path == "/audio_query":
            LOG.append(("POST", u.path, q))
            query = {"accent_phrases": [
                {"moras": [], "accent": 1, "is_interrogative": False,
                 "pause_mora": {"text": "、", "consonant": None, "consonant_length": None,
                                "vowel": "pau", "vowel_length": 0.4, "pitch": 0.0}},
                {"moras": [], "accent": 1, "pause_mora": None, "is_interrogative": False}],
                "speedScale": 1.0, "pitchScale": 0.0, "intonationScale": 1.0, "volumeScale": 1.0,
                "prePhonemeLength": 0.1, "postPhonemeLength": 0.1, "outputSamplingRate": 24000,
                "outputStereo": False, "kana": q["text"]}
            self._send(200, json.dumps(query).encode())
        elif u.path == "/synthesis":
            assert self.headers.get("Content-Type") == "application/json"
            LOG.append(("POST", u.path, q, json.loads(body)))
            self._send(200, wav_bytes(0.5), "audio/wav")
        elif u.path == "/fail":
            self._send(500, "boom".encode())
        else:
            self._send(404, b"{}")


def restore(saved):
    if saved is None:
        os.environ.pop("VOICEVOX_URL", None)
    else:
        os.environ["VOICEVOX_URL"] = saved


class Server:
    def __enter__(self):
        self.srv = HTTPServer(("127.0.0.1", 0), Fake)
        threading.Thread(target=self.srv.serve_forever, daemon=True).start()
        self.saved = os.environ.get("VOICEVOX_URL")
        os.environ["VOICEVOX_URL"] = f"http://127.0.0.1:{self.srv.server_port}"
        LOG.clear()
        return self

    def __exit__(self, *a):
        self.srv.shutdown()
        self.srv.server_close()
        restore(self.saved)


def presets():
    return json.load(open(Path(__file__).resolve().parent / "voicevox_presets.json", encoding="utf-8"))


def test_presets_file():
    p = presets()
    assert p["speakers"]["トーク1"]["style_id"] == 3 and p["speakers"]["トーク2"]["style_id"] == 2
    assert "VOICEVOX:" in p["_説明"]
    assert p["pauses"]["、"] == 0.15


def test_synthesize_request_shape():
    P = presets()
    P["speakers"]["トーク1"]["global_parameters"] = {"speed": 1.3, "pitch": 0.05, "intonation": 1.1, "volume": 0.8}
    with tempfile.TemporaryDirectory() as d, Server():
        out = Path(d) / "a.wav"
        r = vv.synthesize("トーク1", "AKです、はい", out, P, None,
                          [{"surface": "AK", "reading": "えーけー"}], {"intonation": 1.6})
        aq = [x for x in LOG if x[1] == "/audio_query"][0]
        assert aq[2]["speaker"] == "3" and aq[2]["text"] == "エーケーです、はい", aq
        sy = [x for x in LOG if x[1] == "/synthesis"][0]
        assert sy[2]["speaker"] == "3"
        q = sy[3]
        assert q["speedScale"] == 1.3 and q["pitchScale"] == 0.05 and q["volumeScale"] == 0.8
        assert q["intonationScale"] == 1.6  # 行ごとの抑揚が話者の値より強い
        assert q["accent_phrases"][0]["pause_mora"]["vowel_length"] == 0.15  # pauses["、"]
        assert q["accent_phrases"][1]["pause_mora"] is None
        assert out.exists() and not list(Path(d).glob(".*tmp*"))
        assert abs(r["duration"] - 0.5) < 1e-3 and r["unread"] == [] and r["analyzed_text"] == ""
        assert r["phonemes"] is None and r["phoneme_durations"] is None and r["durations_ignored"] is False
        # トーク2 は style 2。抑揚の指定が無ければ話者の値、「、」が無ければ default
        P["pauses"].pop("、")
        vv.synthesize("トーク2", "こんにちは", out, P, None, [], None)
        sy = [x for x in LOG if x[1] == "/synthesis"][-1]
        assert sy[2]["speaker"] == "2" and sy[3]["intonationScale"] == 1.0
        assert sy[3]["accent_phrases"][0]["pause_mora"]["vowel_length"] == 0.2


def test_http_error_message():
    with Server():
        try:
            vv._call("POST", "/fail")
        except RuntimeError as e:
            assert str(e).startswith("VOICEVOX: POST /fail → 500 boom"), str(e)
        else:
            raise AssertionError("RuntimeError にならなかった")


def test_check():
    with Server():
        vv.check()  # 偽のサーバーにはつながる
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()  # 誰も聞いていないポート
    saved = os.environ.get("VOICEVOX_URL")
    os.environ["VOICEVOX_URL"] = f"http://127.0.0.1:{port}"
    try:
        try:
            vv.check()
        except RuntimeError as e:
            assert "VOICEVOX につながりません" in str(e) and str(port) in str(e)
        else:
            raise AssertionError("RuntimeError にならなかった")
    finally:
        restore(saved)


def test_relay_loads_voicevox():
    saved = (vs.ENGINE, vs.ENGINE_DIR, vs.ENGINE_PATH, vs.PROJECT, vs.CACHE, vs.OUT)
    try:
        with tempfile.TemporaryDirectory() as d:
            proj = Path(d) / "ep"
            (proj / "script").mkdir(parents=True)
            vs.set_project(proj)
            mod = vs.load_engine("voicevox")
            assert mod is vv
            info = vs.ping()["engine"]
            assert info == {"name": "voicevox", "label": "VOICEVOX", "features": ["intonation"], "api": 1}, info
            assert vs.speakers_info()["range"] == vv.RANGES
            assert set(vs.load_presets()["speakers"]) == {"トーク1", "トーク2"}
            # 中継の synth 経由でも作れる
            with Server():
                r = vs.synth({"speaker": "トーク1", "text": "テストです、よ。"})
                assert abs(r["duration"] - 0.5) < 1e-3 and (vs.CACHE / f"{r['id']}.wav").exists()
    finally:
        vs.ENGINE, vs.ENGINE_DIR, vs.ENGINE_PATH, vs.PROJECT, vs.CACHE, vs.OUT = saved


if __name__ == "__main__":
    for name, fn in list(globals().items()):
        if name.startswith("test_") and callable(fn):
            fn()
            print("OK", name)
