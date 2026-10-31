"""効果音の設定（se/bank.json）の読み書き（fxkit/fxse.py）。python engine/test_sebank.py で確かめる。

- 書き込みが外の変更を消さないこと（旧#34）
- 空の bank（素の kit。既定の棚が無い）でも一覧が開き、自分の wav を足して鳴らせること（旧#170）
- kit の既定の棚（ak_kit.SE_DIR）の bank を、回に bank.json が無いときの元にすること（回の bank.json が勝つ）
- 返す形が見本 src-tauri/tests/fixtures/se_bank.json（Rust の型 ak_proto::SeBank が読むもの）と同じ形であること（旧#142）
- fxse が fxcore だけを読むこと
"""
import json
import os
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
FXKIT = os.path.join(HERE, "..", "fxkit")
sys.path.insert(0, FXKIT)
sys.path.insert(0, HERE)
import numpy as np  # noqa: E402
import akfx_worker as w  # noqa: E402
import fxse  # noqa: E402

FIXTURE = os.path.join(HERE, "..", "src-tauri", "tests", "fixtures", "se_bank.json")


def tone(path):
    t = np.sin(2 * np.pi * 880 * np.arange(4800) / 48000) * 0.5
    fxse.write_wav(path, np.stack([t, t], 1))


def shape(v):
    """値の形（辞書は項目の名前と中身の形、配列は最初の要素の形、数は number）。"""
    if isinstance(v, dict):
        return {k: shape(x) for k, x in v.items()}
    if isinstance(v, (list, tuple)):
        return [shape(v[0])] if v else []
    if isinstance(v, bool):
        return "bool"
    if isinstance(v, (int, float)):
        return "number"
    return "null" if v is None else "str"


def same_shape(a, b, path=""):
    """a と b の形が同じか（null は何にでも合う。配列の中身は代表の 1 つで比べる）。違いの場所を返す。"""
    if a == "null" or b == "null":
        return None
    if isinstance(a, dict) and isinstance(b, dict):
        if set(a) != set(b):
            return f"{path}: 項目が違います {sorted(set(a) ^ set(b))}"
        for k in a:
            r = same_shape(a[k], b[k], f"{path}.{k}")
            if r:
                return r
        return None
    if isinstance(a, list) and isinstance(b, list):
        return same_shape(a[0], b[0], path + "[]") if a and b else None
    return None if a == b else f"{path}: {a} と {b}"


d = tempfile.mkdtemp(prefix="aks-sebank-")
try:
    ep = os.path.join(d, "ep", "fx")
    os.makedirs(ep)
    ctx = fxse.SeCtx(ep)
    p = ctx.bank_path

    # 無い状態で読んで、そのまま無ければ書ける
    js, seen = fxse.bank_load(ctx)
    assert (js, seen) == ({}, None)
    fxse.bank_write(ctx, {"slam": ["a.wav"]}, seen)
    with open(p, encoding="utf-8") as f:
        assert json.load(f) == {"slam": ["a.wav"]}

    # 読んだときと同じなら書く
    js, seen = fxse.bank_load(ctx)
    js["tick"] = ["b.wav"]
    fxse.bank_write(ctx, js, seen)
    assert fxse.bank_read(ctx) == {"slam": ["a.wav"], "tick": ["b.wav"]}

    # 読んだ後に外で書き換えられていたら、書かずに SeError（ファイルは外の版のまま・一時ファイルも残さない）
    js, seen = fxse.bank_load(ctx)
    outside = '{"slam": ["outside.wav"]}'
    with open(p, "w", encoding="utf-8") as f:
        f.write(outside)
    try:
        fxse.bank_write(ctx, {"slam": ["mine.wav"]}, seen)
        raise AssertionError("SeError になるはず")
    except fxse.SeError as e:
        assert "外で変更" in str(e), e
    with open(p, encoding="utf-8") as f:
        assert f.read() == outside
    assert not os.path.exists(p + ".tmp")

    # 読んだときは無かったのに、外で作られていたら書かない
    os.remove(p)
    _, seen = fxse.bank_load(ctx)
    with open(p, "w", encoding="utf-8") as f:
        f.write(outside)
    try:
        fxse.bank_write(ctx, {}, seen)
        raise AssertionError("SeError になるはず")
    except fxse.SeError:
        pass

    # bank_set は読み直した版に頼まれた項目だけを足すので、外で直したほかの種類・項目は残る
    with open(p, "w", encoding="utf-8") as f:
        json.dump({"tick": ["synth"], "x_outside": 1}, f)
    info = fxse.bank_set(ctx, {"slam": {"db": {"synth": 2}}})
    got = fxse.bank_read(ctx)
    assert got["x_outside"] == 1 and got["tick"] == ["synth"], got
    assert got["_studio"]["db"] == {"slam": {"synth": 2.0}}, got
    assert {k["kind"] for k in info["kinds"]} == set(fxse.LEVEL), info

    # ワーカー経由でも誤りは SeError（画面へ返せる形）
    try:
        w.se_bank_set({"nonexistent": {}}, fx_dir=ep)
        raise AssertionError("SeError になるはず")
    except w.SeError as e:
        assert "不明な効果音の種類" in str(e), e

    # 種類の表示名（kit の se_labels）: 表にある種類は表の名前。表に無い種類・素の kit は種類の名前
    lab = {k["kind"]: k["label"] for k in w.se_bank_info(fx_dir=ep)["kinds"]}
    kit = w.get_kit()
    want_lab = {k: kit.se_labels().get(k, k) for k in lab}
    assert lab == want_lab, lab
    if kit.NAME != "basic":   # 手元の kit（非公開）では、表の名前が出る
        assert lab["chapter"] == "章タイトル" and lab["glitch_s"] == "グリッチ（小）", lab
    assert {k["kind"]: k["label"] for k in fxse.bank_info(ctx)["kinds"]}["chapter"] == "chapter"

    # 返す形は見本と同じ（Rust の ak_proto::SeBank が読む形。見本の _studio 付きの bank でも同じ）
    with open(FIXTURE, encoding="utf-8") as f:
        fx = json.load(f)
    got = w.se_bank_info(fx_dir=ep)
    assert set(got) == set(fx), (set(got) ^ set(fx))
    for k in ("path", "exists", "files", "shelf_dir", "db_range"):
        r = same_shape(shape(got[k]), shape(fx[k]), k)
        assert r is None, r
    fx_kind, fx_cand = fx["kinds"][0], fx["kinds"][0]["cands"][0]
    for x in got["kinds"]:
        r = same_shape(shape({k: v for k, v in x.items() if k != "cands"}), shape({k: v for k, v in fx_kind.items() if k != "cands"}), x["kind"])
        assert r is None, r
        for c in x["cands"]:
            r = same_shape(shape(c), shape(fx_cand), x["kind"] + ".cands")
            assert r is None, r
    assert got["db_range"] == fx["db_range"]

    # ---- 空の bank（素の kit）: 候補なし・音なしで動き、自分の wav を足して鳴らせる
    ep2 = os.path.join(d, "ep2", "fx")
    os.makedirs(ep2)
    ctx2 = fxse.SeCtx(ep2)                  # default_dir なし
    info = fxse.bank_info(ctx2)
    assert not info["exists"] and info["files"] == [], info
    assert all(k["use"] == ["synth"] and not k["explicit"] and [c["src"] for c in k["cands"]] == ["synth"] for k in info["kinds"])
    # 取り込んだ wav（se/<種類>/ に置く）が候補に出る → 鳴らす音に選べる → 試聴できる → 書き出しに使われる
    os.makedirs(os.path.join(ep2, "se", "slam"))
    tone(os.path.join(ep2, "se", "slam", "mine.wav"))
    info = fxse.bank_set(ctx2, {"slam": {"use": ["slam/mine.wav"], "db": {"slam/mine.wav": -2}}})
    sl = {k["kind"]: k for k in info["kinds"]}["slam"]
    assert sl["use"] == ["slam/mine.wav"] and sl["explicit"] and info["exists"], sl
    assert [c["src"] for c in sl["cands"]] == ["synth", "slam/mine.wav"] and sl["cands"][1]["folder"] and sl["cands"][1]["db"] == -2.0
    out = os.path.join(d, "aud", "mine.wav")
    fxse.audition(ctx2, "slam", "slam/mine.wav", out)
    assert os.path.getsize(out) > 10000
    ev = [dict(t=0.5, kind="slam", x=fxse.STAGE_CX, key=("slam", 0.5, "x"))]
    r = fxse.render(ctx2, ev, 3.0, os.path.join(d, "out2"), "ep")
    assert r["count"] == 1 and r["events"] == [{"t": 0.5, "kind": "slam"}] and os.path.getsize(r["wav"]) > 10000, r
    # 候補が 1 つも無い回でも作り直しはできる（無音の wav）
    r = fxse.render(fxse.SeCtx(ep2), [], 2.0, os.path.join(d, "out3"), "ep")
    assert r["count"] == 0 and os.path.getsize(r["wav"]) > 100, r

    # ---- kit の既定の棚（SE_DIR）: 回に bank.json が無ければその bank と wav を使う。回の bank.json があれば回が勝つ
    kit_se = os.path.join(d, "kit", "se")
    os.makedirs(kit_se)
    tone(os.path.join(kit_se, "K1.wav"))
    tone(os.path.join(kit_se, "K2.wav"))
    with open(os.path.join(kit_se, "bank.json"), "w", encoding="utf-8") as f:
        json.dump({"slam": ["synth", "K1.wav"], "guide_done": ["K1.wav"], "_sources": {"K1.wav": "テスト"}}, f)
    ep3 = os.path.join(d, "ep3", "fx")
    os.makedirs(ep3)
    ctx3 = fxse.SeCtx(ep3, default_dir=kit_se, shelf_dir=os.path.join(d, "lib"))
    info = fxse.bank_info(ctx3)
    kd = {k["kind"]: k for k in info["kinds"]}
    assert info["exists"] and info["files"] == ["K1.wav", "K2.wav"] and info["path"] == ctx3.bank_path, info
    assert kd["slam"]["use"] == ["synth", "K1.wav"] and kd["slam"]["explicit"]
    assert [c["src"] for c in kd["slam"]["cands"]] == ["synth", "K1.wav"] and kd["slam"]["cands"][1]["exists"]
    assert kd["slam"]["cands"][1]["source"] == "テスト"
    assert kd["guide_done"]["shelf"] and kd["guide_done"]["use"] == ["synth"], "棚の種類の並びは既定の棚から持ち込まない"
    bank, level = fxse.build_bank(ctx3)
    assert [x if isinstance(x, str) else x[0] for x in bank["slam"]] == ["synth", "K1"] and "guide_done" not in bank
    # 変えると回の bank.json に書く（既定の棚は書き換えない）。既定の棚の wav はそのまま選べる
    info = fxse.bank_set(ctx3, {"slam": {"use": ["K2.wav"]}})
    assert os.path.exists(ctx3.bank_path) and json.load(open(ctx3.bank_path, encoding="utf-8"))["slam"] == ["K2.wav"]
    assert json.load(open(os.path.join(kit_se, "bank.json"), encoding="utf-8"))["slam"] == ["synth", "K1.wav"]
    assert {k["kind"]: k for k in info["kinds"]}["slam"]["use"] == ["K2.wav"], "回の bank.json が勝つ"
    # 既定の棚の bank.json が壊れていたら、理由を出す
    with open(os.path.join(kit_se, "bank.json"), "w", encoding="utf-8") as f:
        f.write("{broken")
    os.remove(ctx3.bank_path)
    try:
        fxse.bank_info(ctx3)
        raise AssertionError("SeError になるはず")
    except fxse.SeError as e:
        assert "既定の効果音の設定" in str(e), e

    # ---- fxse は芯（fxcore）だけを読む。部品・kit・fxsound を読まない
    code = ("import sys; import fxse; "
            "bad = [m for m in ('fxparts', 'fxkit', 'fxsound', 'ak_kit', 'ak_kit_basic') if m in sys.modules]; "
            "sys.exit(1 if bad else 0)")
    r = subprocess.run([sys.executable, "-c", code], cwd=FXKIT, env={**os.environ, "PYTHONPATH": FXKIT},
                       capture_output=True, text=True)
    assert r.returncode == 0, r.stderr or "fxse が fxparts・fxkit・fxsound・ak_kit を読んでいます"

    # ---- ak_kit.se_labels: 回の fxsound.py（LABEL 付き）が先に見つかればその表。LABEL が無い古い版（fxfreeze で固定した回）なら kit の表
    code = "import ak_kit; print(ak_kit.se_labels()['slam'])"
    for body, want in (("LABEL = {'slam': '回のハンコ'}" + chr(10), "回のハンコ"), ("LEVEL = {}" + chr(10), "ハンコ（叩きつけ）")):
        fxd = os.path.join(d, "frozen")
        os.makedirs(fxd, exist_ok=True)
        with open(os.path.join(fxd, "fxsound.py"), "w", encoding="utf-8") as f:
            f.write(body)
        r = subprocess.run([sys.executable, "-c", code], cwd=d, env={**os.environ, "PYTHONPATH": os.pathsep.join([fxd, FXKIT]),
                           "PYTHONIOENCODING": "utf-8"}, capture_output=True, text=True, encoding="utf-8")
        assert r.returncode == 0 and r.stdout.strip() == want, (r.stdout, r.stderr)
finally:
    shutil.rmtree(d, ignore_errors=True)

print("ok")
