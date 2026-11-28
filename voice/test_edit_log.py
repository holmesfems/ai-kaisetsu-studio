"""手直しの履歴（<回>/studio/edit_log.jsonl）を voice_review.py apply --write が書くかを、一時フォルダの回で確かめる。VoiSona は使わない。

    python test_edit_log.py
"""
import json
import os
import sys
import tempfile
from argparse import Namespace
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import akscript as ak  # noqa: E402
import voice_review as vrv  # noqa: E402


def _rows(proj):
    p = Path(proj) / "studio" / "edit_log.jsonl"
    return [json.loads(x) for x in p.read_text(encoding="utf-8").splitlines()] if p.exists() else []


def test_append_only():
    with tempfile.TemporaryDirectory() as td:
        e = {"kind": "text", "line_id": "l1", "block_file": "a.yaml", "before": "プラマニS2", "after": "プラマニのS2"}
        assert ak.append_edit_log(td, [e]) == 1
        assert ak.append_edit_log(td, [{**e, "before": "x", "after": "x"}]) == 0
        assert ak.append_edit_log(td, [{**e, "source": "user", "note": "読みを直す"}]) == 1
        rows = _rows(td)
        assert len(rows) == 2, rows
        assert rows[0]["source"] == "voice_review_apply" and rows[1]["source"] == "user", rows
        assert rows[1]["note"] == "読みを直す" and "note" not in rows[0], rows
        assert set(rows[0]) == {"ts", "kind", "line_id", "block_file", "before", "after", "source"}, rows[0]


def test_apply_write_logs_accent():
    with tempfile.TemporaryDirectory() as td:
        proj = Path(td)
        (proj / "script").mkdir()
        (proj / "script" / "01.yaml").write_text(
            "title: 一\nlines:\n  - id: l1\n    sp: トーク1\n    text: プラマニのS2\n  - id: l2\n    sp: トーク2\n    text: そう\n",
            encoding="utf-8")
        old = {"sp": "トーク2", "text": "そう", "analyzed_text": "<tsml><word>そう</word></tsml>"}
        ak.save_voice(proj / "script", {"l2": old})
        (proj / "voice_review").mkdir()
        new_xml = "<tsml><word hl=\"lhh\">プラマニの</word></tsml>"
        (proj / "voice_review" / "result.json").write_text(json.dumps({
            "saved": "now",
            "dict": [],
            "lines": [
                {"id": "l1", "status": "ok", "speaker": "トーク1", "text": "プラマニのS2", "analyzed_text": new_xml, "note": "プラマニは頭高"},
                {"id": "l2", "status": "ng", "speaker": "トーク2", "text": "そう", "analyzed_text": "<tsml/>"},
            ]}, ensure_ascii=False), encoding="utf-8")
        os.environ["AK_VOICE_DICT"] = str(proj / "voice_dict.json")
        (proj / "voice_dict.json").write_text(json.dumps({"entries": []}), encoding="utf-8")
        vrv.apply(Namespace(project=str(proj), write=True, port=None, dict_file=True))
        rows = _rows(proj)
        assert len(rows) == 1, rows
        r = rows[0]
        assert (r["kind"], r["line_id"], r["block_file"], r["source"]) == ("accent", "l1", "01.yaml", "voice_review_apply"), r
        assert r["before"] is None and r["after"]["analyzed_text"] == new_xml, r
        assert r["note"] == "プラマニは頭高", r
        # 書かなかったとき（--write なし）は履歴も増えない
        vrv.apply(Namespace(project=str(proj), write=False, port=None, dict_file=True))
        assert len(_rows(proj)) == 1


if __name__ == "__main__":
    sys.stdout.reconfigure(errors="replace")
    test_append_only()
    test_apply_write_logs_accent()
    print("OK")
