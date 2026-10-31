"""Timeline が cue の表（script_cues.json）と手で動かした同期点（sync_manual.json）を読む。
python fxkit/test_timeline.py で確かめる。見本は Rust 側のテスト（export/talk.rs・syncman.rs）が縛っている
src-tauri/tests/fixtures/ の script_cues.json・sync_manual.json（Rust と Python が同じ形を読み書きしている証拠）。"""
import json
import os
import shutil
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import fxcore  # noqa: E402

FIX = os.path.join(HERE, "..", "src-tauri", "tests", "fixtures")


def read_json(p):
    with open(p, encoding="utf-8") as f:
        return json.load(f)


table = read_json(os.path.join(FIX, "script_cues.json"))["cues"]
manual = read_json(os.path.join(FIX, "sync_manual.json"))["sync"]
assert len(table) >= 3 and len(manual) >= 3

d = tempfile.mkdtemp(prefix="aks-timeline-")
cues_p = os.path.join(d, "script_cues.json")
shutil.copy(os.path.join(FIX, "script_cues.json"), cues_p)

# --- 表どおりに読める
tl = fxcore.Timeline(cues_p)
assert sorted(tl.cues) == [c["n"] for c in table]
for c in table:
    assert tl.c(c["n"]) == c["start"] and tl.ce(c["n"]) == c["end"], c
    assert tl.cues[c["n"]]["speaker"] == c["speaker"] and tl.cues[c["n"]]["text"] == c["text"], c
assert fxcore.load_cues(cues_p)[1]["text"] == table[0]["text"]

# --- .srt のパスを渡しても、隣の script_cues.json を読む（srt の中身は読まない。無くてもよい）
tl2 = fxcore.Timeline(os.path.join(d, "script.srt"))
assert tl2.cues == tl.cues
open(os.path.join(d, "script.srt"), "w", encoding="utf-8").write("でたらめ\n")
assert fxcore.Timeline(os.path.join(d, "script.srt")).cues == tl.cues

# --- 同期点: ふつうは文字位置の割合。sync_manual.json は sync_points.json（無くてよい）の隣から読む
sync_p = os.path.join(d, "sync_points.json")
c0 = table[0]
kw = c0["text"].split(" ")[0]
tl3 = fxcore.Timeline(cues_p, sync=sync_p)
tl3.s(c0["n"], kw)
assert tl3.sync_used[-1][4] == "ratio"
shutil.copy(os.path.join(FIX, "sync_manual.json"), os.path.join(d, "sync_manual.json"))
tl4 = fxcore.Timeline(cues_p, sync=sync_p)
for r in manual:
    assert tl4.s(r["cue"], r["keyword"]) == r["t"], r
    assert tl4.s(r["cue"], r["keyword"], end=True) == r["t_end"], r
    assert tl4.sync_used[-1][4] == r.get("method", "manual"), (r, tl4.sync_used[-1])
assert any("method" not in r for r in manual), "method が無い見本が要る"

# --- 表が無いと、書き出すよう案内するエラー（srt を読みに戻らない）
empty = tempfile.mkdtemp(prefix="aks-timeline-empty-")
open(os.path.join(empty, "script.srt"), "w", encoding="utf-8").write("1\n00:00:00,000 --> 00:00:01,000\n[A] あ\n\n")
for p in (os.path.join(empty, "script_cues.json"), os.path.join(empty, "script.srt")):
    try:
        fxcore.Timeline(p)
    except FileNotFoundError as e:
        assert "書き出して" in str(e), e
    else:
        raise AssertionError("FileNotFoundError になるはず: " + p)
assert not hasattr(fxcore, "parse_srt")
print("ok")
