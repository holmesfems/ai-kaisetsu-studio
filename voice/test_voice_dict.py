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
    print("ok")


if __name__ == "__main__":
    main()
