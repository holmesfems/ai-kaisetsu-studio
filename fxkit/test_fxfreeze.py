"""fxfreeze（回の fx/ に fxkit を写す）。python fxkit/test_fxfreeze.py で確かめる。"""
import datetime
import os
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import fxfreeze  # noqa: E402


def write(p, s):
    os.makedirs(os.path.dirname(p), exist_ok=True)
    open(p, "w", encoding="utf-8", newline="\n").write(s)


def read(p):
    return open(p, encoding="utf-8").read()


tmp = tempfile.mkdtemp()
src = os.path.join(tmp, "src")
for m in ("fxcore", "ak_kit", "fxkit", "fxparts", "fxsound", "risei_template", "other_template"):
    write(os.path.join(src, m + ".py"), f"# {m} v1\n")
write(os.path.join(src, "risei_template.py"), "# risei\nimport other_template\n")


def episode(name):
    ep = os.path.join(tmp, name)
    write(os.path.join(ep, "fx", "x_fx.py"), "from fxkit import *\nimport risei_template\nimport numpy\n")
    return ep


# 1. 写せる: 芯と kit のうち見つかった物（fxse は src に無いので写さない）＋使っているテンプレート（その import も）。使っていない物・外部モジュールは写さない
ep = episode("ep1")
r = fxfreeze.freeze(ep, src_dir=src)
assert sorted(r["copied"]) == ["ak_kit.py", "fxcore.py", "fxkit.py", "fxparts.py", "fxsound.py", "other_template.py", "risei_template.py"], r
assert r["differ"] == [] and r["backup"] is None
assert read(os.path.join(ep, "fx", "fxkit.py")) == "# fxkit v1\n"
assert not os.path.exists(os.path.join(ep, "fx", "numpy.py"))
assert not os.path.exists(os.path.join(ep, "fx", "fxse.py"))

# 2. 2 回目は同じなので何もしない
r = fxfreeze.freeze(ep, src_dir=src)
assert r["copied"] == [] and len(r["same"]) == 7, r

# 3. 違うと --force 無しでは何も写さず止まる（ほかのファイルも写さない）
write(os.path.join(ep, "fx", "fxkit.py"), "# 回で直した版\n")
write(os.path.join(src, "fxparts.py"), "# fxparts v2\n")
r = fxfreeze.freeze(ep, src_dir=src)
assert r["differ"] == ["fxkit.py", "fxparts.py"] and r["copied"] == [], r
assert read(os.path.join(ep, "fx", "fxkit.py")) == "# 回で直した版\n"
assert read(os.path.join(ep, "fx", "fxparts.py")) == "# fxparts v1\n"

# 4. --force で上書きし、控えが残る
r = fxfreeze.freeze(ep, force=True, src_dir=src, now=datetime.datetime(2026, 1, 2, 3, 4, 5))
bk = r["backup"]
assert os.path.basename(bk) == "_fxkit_backup_20260102_030405", bk
assert read(os.path.join(bk, "fxkit.py")) == "# 回で直した版\n"
assert read(os.path.join(bk, "fxparts.py")) == "# fxparts v1\n"
assert read(os.path.join(ep, "fx", "fxkit.py")) == "# fxkit v1\n"
assert read(os.path.join(ep, "fx", "fxparts.py")) == "# fxparts v2\n"

# 5. fx/ が無い回はエラー
try:
    fxfreeze.freeze(os.path.join(tmp, "nothing"), src_dir=src)
    assert False
except RuntimeError:
    pass

# 6. 途中で失敗したら全部か無しに: 2 つ目の os.replace で落とすと、1 つ目も元に戻り、一時ファイルが残らない
ep = episode("ep6")
fxfreeze.freeze(ep, src_dir=src)
write(os.path.join(src, "fxkit.py"), "# fxkit v3\n")
write(os.path.join(src, "fxparts.py"), "# fxparts v3\n")
write(os.path.join(ep, "fx", "fxparts.py"), "# 回の fxparts\n")
real_replace = os.replace
calls = []


def flaky(a, b):
    calls.append(b)
    if len(calls) == 2:
        raise PermissionError("使用中です")
    return real_replace(a, b)


os.replace = flaky
try:
    try:
        fxfreeze.freeze(ep, force=True, src_dir=src)
        assert False, "失敗しなかった"
    except fxfreeze.FreezeError as e:
        assert e.copied == [] and e.stuck == [], (e.copied, e.stuck)
        assert sorted(e.not_copied) == ["fxkit.py", "fxparts.py"], e.not_copied
finally:
    os.replace = real_replace
assert read(os.path.join(ep, "fx", "fxkit.py")) == "# fxkit v1\n", "1 つ目が元に戻っていない"
assert read(os.path.join(ep, "fx", "fxparts.py")) == "# 回の fxparts\n"
assert not [n for n in os.listdir(os.path.join(ep, "fx")) if ".tmp-" in n], "一時ファイルが残った"

# 7. main は失敗を出して 0 以外で終わる（どれが写ったか写らなかったかを出す）
import io  # noqa: E402
calls.clear()
os.replace = flaky
err = io.StringIO()
real_err = sys.stderr
sys.stderr = err
try:
    code = fxfreeze.main([ep, "--force"])
finally:
    sys.stderr = real_err
    os.replace = real_replace
assert code == 1 and "写していないファイル" in err.getvalue(), err.getvalue()

# 8. 回の *_fx.py がまだ import fxcli（旧#98 で消した）していると、何も写さずに止まる（写すと開けなくなる）
ep8 = episode("ep8")
write(os.path.join(ep8, "fx", "old_fx.py"), "from fxkit import *\nimport fxcli\n")
write(os.path.join(ep8, "fx", "fxcli.py"), "# 固定した古い fxcli\n")
try:
    fxfreeze.freeze(ep8, force=True, src_dir=src)
    raise AssertionError("止まらなかった")
except RuntimeError as e:
    assert "old_fx.py: fxcli は無くなりました" in str(e), e
assert not os.path.exists(os.path.join(ep8, "fx", "fxkit.py"))

# 9. 写し元は fxcore のあるフォルダ（kit の fxkit.py が無い公開版だけでも見つかる）。回の fx/ のコピーは除く
pub = os.path.join(tmp, "pub")
write(os.path.join(pub, "fxcore.py"), "# fxcore\n")
ep9 = os.path.join(tmp, "ep9")
write(os.path.join(ep9, "fx", "fxcore.py"), "# 回のコピー\n")
saved = sys.path[:]
sys.path[:0] = [os.path.join(ep9, "fx"), pub]
try:
    assert os.path.normcase(fxfreeze.find_source_dir(os.path.join(ep9, "fx"))) == os.path.normcase(pub)
finally:
    sys.path[:] = saved
r = fxfreeze.freeze(ep9, src_dir=pub)           # kit が無ければ芯だけ写す
assert r["differ"] == ["fxcore.py"], r

print("test_fxfreeze: ok")
