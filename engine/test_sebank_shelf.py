"""攻略動画の操作の効果音（棚の種類）。回の se/bank.json に無い種類は、アプリの効果音の棚から読み、棚にも無ければ合成の音で鳴る。
python engine/test_sebank_shelf.py で確かめる（fxkit の本物の fxse を使う）。"""
import json
import os
import shutil
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.join(os.path.dirname(HERE), "fxkit"))
import numpy as np  # noqa: E402
import akfx_worker as w  # noqa: E402
import fxse  # noqa: E402

KINDS = ("guide_pre", "guide_done", "guide_precise_pre", "guide_precise_done")
assert tuple(fxse.SHELF_KINDS) == KINDS

d = tempfile.mkdtemp(prefix="aks-shelf-")
try:
    ep = os.path.join(d, "ep", "fx")         # 回の fx/
    lib = os.path.join(d, "lib")             # アプリの効果音の棚
    os.makedirs(os.path.join(ep, "se"))
    for k in KINDS:
        os.makedirs(os.path.join(lib, k))
    tone = np.sin(2 * np.pi * 880 * np.arange(4800) / 48000) * 0.5
    fxse.write_wav(os.path.join(lib, "guide_done", "a.wav"), np.stack([tone, tone], 1))
    fxse.write_wav(os.path.join(lib, "guide_done", "b.wav"), np.stack([tone, tone], 1))
    with open(os.path.join(lib, "guide_pre", "float.wav"), "wb") as f:
        f.write(b"RIFFxxxxWAVEnot-a-wav")   # 読めない wav（候補には出るが bad）

    # 4 つの種類がある（種類・名前・合成の音・音量・優先度）
    for k in KINDS:
        assert k in fxse.LEVEL and k in fxse.PRIO and k in fxse.SYNTH, k
    if w.get_kit().NAME != "basic":                  # 名前は kit の se_labels()（素の kit は種類の名前のまま）
        for k in KINDS:
            assert k in w.get_kit().se_labels(), k
        assert [w.get_kit().se_labels()[k] for k in KINDS] == ["操作の予備", "操作の完了", "精密操作の予備", "精密操作の完了"]

    # 棚が分からない（lib_dir が無い）ときは棚を使わない（合成の音）
    w._shelf_set({})
    ctx, _ = w._se_ctx(ep)
    bank, level = fxse.build_bank(ctx)
    assert not any(k in bank for k in KINDS)
    assert not w.se_bank_info(fx_dir=ep)["kinds"][0].get("shelf")

    # 棚の選び（settings.json の guide_se）→ その音を棚から読む。音量は候補の音量として LEVEL に足される
    w._shelf_set({"lib_dir": lib, "guide_se": {"guide_done": {"src": "guide_done/a.wav", "db": -3}}})
    ctx, _ = w._se_ctx(ep)
    bank, level = fxse.build_bank(ctx)
    assert "guide_done" in bank and len(bank["guide_done"]) == 1
    assert bank["guide_done"][0][0] == "a"
    assert abs(level["guide_done"] - (fxse.LEVEL["guide_done"] - 3)) < 1e-9, level["guide_done"]
    assert "guide_pre" not in bank, "選んでいない種類は合成の音"
    assert ctx.shelf_warn == []

    # 一覧（se_bank）: 棚の音が候補に並ぶ・選びが use・音量は選んだ音の分・合成の音も候補
    info = w.se_bank_info(fx_dir=ep)
    kd = {x["kind"]: x for x in info["kinds"]}
    assert info["shelf_dir"] == os.path.abspath(lib)
    g = kd["guide_done"]
    assert g["shelf"] and g["use"] == ["guide_done/a.wav"] and g["explicit"]
    srcs = [c["src"] for c in g["cands"]]
    assert srcs == ["synth", "guide_done/a.wav", "guide_done/b.wav"], srcs
    assert [c["db"] for c in g["cands"]] == [0.0, -3.0, 0.0]
    assert all(c["exists"] and c["bad"] is None for c in g["cands"])
    p = kd["guide_pre"]
    assert p["shelf"] and p["use"] == ["synth"] and not p["explicit"]
    bad = [c for c in p["cands"] if c["src"] == "guide_pre/float.wav"][0]
    assert bad["exists"] and bad["bad"], "読めない wav は bad"

    # 回の bank.json にその種類があれば回のほうが勝つ（棚の選びは使わない）
    with open(os.path.join(ep, "se", "bank.json"), "w", encoding="utf-8") as f:
        json.dump({"guide_done": ["synth"]}, f)
    bank, level = fxse.build_bank(ctx)
    assert list(bank["guide_done"]) == ["synth"]
    assert abs(level["guide_done"] - fxse.LEVEL["guide_done"]) < 1e-9
    g = {x["kind"]: x for x in w.se_bank_info(fx_dir=ep)["kinds"]}["guide_done"]
    assert not g["shelf"] and g["use"] == ["synth"]
    os.remove(os.path.join(ep, "se", "bank.json"))

    # 選んだ音が棚に無い・読めない → 合成の音で鳴らして警告に出す（黙って無音にしない）
    w._shelf_set({"lib_dir": lib, "guide_se": {"guide_pre": {"src": "guide_pre/none.wav", "db": 2},
                                               "guide_precise_pre": {"src": "guide_pre/float.wav"},
                                               "guide_precise_done": {"src": "../x.wav"}}})
    ctx, _ = w._se_ctx(ep)
    bank, level = fxse.build_bank(ctx)
    assert not any(k in bank for k in ("guide_pre", "guide_precise_pre", "guide_precise_done"))
    assert len(ctx.shelf_warn) == 3 and all("合成の音で鳴らします" in m for m in ctx.shelf_warn), ctx.shelf_warn
    assert abs(level["guide_pre"] - fxse.LEVEL["guide_pre"]) < 1e-9, "使えない音の音量は付けない"
    p = {x["kind"]: x for x in w.se_bank_info(fx_dir=ep)["kinds"]}["guide_pre"]
    assert p["use"] == ["guide_pre/none.wav"] and not p["cands"][1]["exists"], "画面に無いことが出る"

    # 鳴らさない種類は棚から読まない・警告も出さない（攻略動画でない回で、棚の wav が無くても警告しない）
    w._shelf_set({"lib_dir": lib, "guide_se": {"guide_pre": {"src": "guide_pre/none.wav", "db": 2}}})
    ctx, _ = w._se_ctx(ep)
    bank, level = fxse.build_bank(ctx, used={"slam"})
    assert not any(k in bank for k in KINDS) and ctx.shelf_warn == []
    bank, level = fxse.build_bank(ctx, used={"guide_pre"})
    assert len(ctx.shelf_warn) == 1, ctx.shelf_warn

    # 合成の音の音量（src が synth の db）
    w._shelf_set({"lib_dir": lib, "guide_se": {"guide_done": {"src": "synth", "db": 4}}})
    ctx, _ = w._se_ctx(ep)
    bank, level = fxse.build_bank(ctx)
    assert "guide_done" not in bank and abs(level["guide_done"] - (fxse.LEVEL["guide_done"] + 4)) < 1e-9

    # 試聴: 棚の音は棚から読む。棚の外・別の種類・読めない音は断る
    w._shelf_set({"lib_dir": lib, "guide_se": {}})
    out = os.path.join(d, "aud", "a.wav")
    r = w.se_audition("guide_done", "guide_done/a.wav", out, fx_dir=ep)
    assert os.path.exists(out) and r["src"] == "guide_done/a.wav"
    for src in ("../a.wav", "guide_pre/float.wav", "guide_done/none.wav"):
        try:
            w.se_audition("guide_done", src, out, fx_dir=ep)
        except w.SeError:
            pass
        else:
            raise AssertionError(f"断るはず: {src}")
    # 合成の音の試聴・どの種類の合成の音も鳴らせる
    for k in KINDS:
        o2 = os.path.join(d, "aud", f"{k}.wav")
        w.se_audition(k, "synth", o2, fx_dir=ep)
        assert os.path.getsize(o2) > 1000, k

    # 書き出し: 棚の音・合成の音で、4 つの種類の候補を鳴らして wav になる（棚の音は鳴らした音の名前が src に入る）
    w._shelf_set({"lib_dir": lib, "guide_se": {"guide_done": {"src": "guide_done/b.wav"}}})
    ctx, _ = w._se_ctx(ep)
    bank, level = fxse.build_bank(ctx)
    ev = [dict(t=1.0 + i, kind=k, x=1680, key=(k, 1.0 + i, "1680")) for i, k in enumerate(KINDS)]
    wav = os.path.join(d, "out.wav")
    with fxse._LevelPatch(level):
        sel = fxse.render_se(ev, 6.0, wav, "full", bank)
    assert [s["kind"] for s in sel] == list(KINDS)
    assert {s["kind"]: s["src"] for s in sel}["guide_done"] == "b"
    assert all(s["src"] == "synth" for s in sel if s["kind"] != "guide_done")
    assert os.path.getsize(wav) > 10000
    print("ok")
finally:
    shutil.rmtree(d, ignore_errors=True)
