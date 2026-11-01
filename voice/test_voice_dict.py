"""辞書の場所（AK_VOICE_DICT）と書き込みの原子性のテスト。素の assert: python voice/test_voice_dict.py"""
import json
import os
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import voisona_tts as vt  # noqa: E402


def main():
    with tempfile.TemporaryDirectory() as td:
        td = Path(td)
        # 環境変数で場所が変わる（呼ぶたびに見る）
        a, b = td / "a" / "d.json", td / "b.json"
        os.environ["AK_VOICE_DICT"] = str(a)
        assert vt.dict_path() == a
        assert vt.load_dict() == []  # 無ければ空（エラーにしない）
        entries = [{"surface": "博士", "reading": "ハカセ", "accent": "lhh"}]
        vt.save_dict(entries)  # 親フォルダが無くても作る
        assert json.loads(a.read_text(encoding="utf-8")) == {"entries": entries}
        assert vt.load_dict() == entries
        os.environ["AK_VOICE_DICT"] = str(b)
        assert vt.dict_path() == b and vt.load_dict() == []
        vt.save_dict([])
        assert a.exists() and b.exists()

        # 原子的: 書いたあとに一時ファイルが残らない。書き込みに失敗したら元の辞書はそのまま
        left = [p.name for p in a.parent.iterdir()]
        assert left == ["d.json"], left
        bad = [{"surface": object()}]
        os.environ["AK_VOICE_DICT"] = str(a)
        try:
            vt.save_dict(bad)
            raise AssertionError("書けない中身で例外にならなかった")
        except TypeError:
            pass
        assert vt.load_dict() == entries, "失敗したのに元の辞書が壊れた"
        assert [p.name for p in a.parent.iterdir()] == ["d.json"], "失敗後に一時ファイルが残った"

        # 環境変数が無ければ LOCALAPPDATA の ak-studio フォルダ
        del os.environ["AK_VOICE_DICT"]
        os.environ["LOCALAPPDATA"] = str(td / "local")
        assert vt.dict_path() == td / "local" / "ak-studio" / "voice_dict.json"

    # 古い場所からの移し替えは 旧#173 でやめた。新しい場所に辞書が無ければ空（古い場所は見ない）
    with tempfile.TemporaryDirectory() as td:
        td = Path(td)
        os.environ["AK_VOICE_DICT"] = str(td / "new" / "voice_dict.json")
        os.environ["AK_VOICE_DICT_OLD"] = str(td / "old.json")  # 設定しても読まない
        (td / "old.json").write_text(json.dumps({"entries": [{"surface": "古い語"}]}), encoding="utf-8")
        assert vt.load_dict() == [] and not (td / "new").exists()
        del os.environ["AK_VOICE_DICT_OLD"]
        del os.environ["AK_VOICE_DICT"]
    relay()
    print("ok")


def relay():
    """道具は辞書を中継（/api/dict）から読み書きする。中継が無ければ None・書けなければ RuntimeError。"""
    import threading
    from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

    store = {"entries": [{"surface": "博士", "reading": "ハカセ", "accent": "lhh"}], "fail": False}

    class H(BaseHTTPRequestHandler):
        def log_message(self, *a):
            pass

        def reply(self, code, obj):
            b = json.dumps(obj, ensure_ascii=False).encode("utf-8")
            self.send_response(code)
            self.send_header("Content-Type", "application/json; charset=utf-8")
            self.send_header("Content-Length", str(len(b)))
            self.end_headers()
            self.wfile.write(b)

        def do_GET(self):
            if store["fail"]:
                return self.reply(500, {"error": "壊れた辞書"})
            self.reply(200, {"entries": store["entries"]})

        def do_POST(self):
            body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            if store["fail"]:
                return self.reply(500, {"error": "書けない"})
            store["entries"] = body["entries"]
            self.reply(200, {"entries": store["entries"]})

    srv = ThreadingHTTPServer(("127.0.0.1", 0), H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    port = srv.server_address[1]
    try:
        assert vt.relay_base(port) == f"http://127.0.0.1:{port}"
        os.environ["AK_VOICE_PORT"] = str(port)
        assert vt.relay_base() == f"http://127.0.0.1:{port}"  # 引数が無ければ環境変数
        del os.environ["AK_VOICE_PORT"]
        assert vt.relay_dict(port) == store["entries"]
        new = store["entries"] + [{"surface": "聖聆", "reading": "セーレー", "accent": "lhhh"}]
        assert vt.save_relay_dict(new, port) == new and store["entries"] == new

        # 中継がエラーを返したら、None（中継なし）ではなく RuntimeError にする（黙ってファイルへ回らない）
        store["fail"] = True
        for f in (lambda: vt.relay_dict(port), lambda: vt.save_relay_dict([], port)):
            try:
                f()
                raise AssertionError("中継のエラーで例外にならなかった")
            except RuntimeError as e:
                assert "辞書" in str(e), e
        assert store["entries"] == new
    finally:
        srv.shutdown()
        srv.server_close()

    # つながらない: 読みは None、書きは RuntimeError
    assert vt.relay_dict(port, timeout=1) is None
    try:
        vt.save_relay_dict([], port, timeout=1)
        raise AssertionError("中継が無いのに例外にならなかった")
    except RuntimeError as e:
        assert "届きません" in str(e), e

    # voice_review は中継につながらなければ止まる（写しの辞書を黙って読み書きしない）。--dict-file ならファイル
    import argparse
    import voice_review as vrv
    try:
        vrv.read_dict(argparse.Namespace(dict_file=False, port=port))
        raise AssertionError("中継が無いのに止まらなかった")
    except SystemExit as e:
        assert "--dict-file" in str(e.code), e.code
    with tempfile.TemporaryDirectory() as td:
        os.environ["AK_VOICE_DICT"] = str(Path(td) / "d.json")
        vt.save_dict([{"surface": "x", "reading": "エックス"}])
        assert vrv.read_dict(argparse.Namespace(dict_file=True, port=port)) == [{"surface": "x", "reading": "エックス"}]
        del os.environ["AK_VOICE_DICT"]


if __name__ == "__main__":
    main()
