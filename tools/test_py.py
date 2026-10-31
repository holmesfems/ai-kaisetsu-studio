#!/usr/bin/env python3
"""Python のテストを 1 本ずつ回して、結果を一覧にする（標準ライブラリだけ）。

    python tools/test_py.py                          # engine/・fxkit/・voice/ の test_*.py を全部
    python tools/test_py.py fxkit                    # フォルダで絞る
    python tools/test_py.py voice/test_voice_keys.py # ファイルで絞る

引数が無いときは、非公開の kit（fx-private/kit/。環境変数 AK_FX_PRIVATE があればそのフォルダ）が
あれば、その中の test_*.py も回す（旧#172）。kit の中のテストは、kit と公開の fxkit/ を sys.path に入れて回す。

環境変数 AK_PUBLIC_TEST=1（npm run test:public）のときは、pip install -e で入れた kit が
sys.path に混ざって「公開側だけ」の確かめにならないよう、kit のファイル（fxparts.py など）を
持つ sys.path の項目を、テストの Python の起動時に外す（sitecustomize）。kit のテストは回さない。

テストは pytest ではなく素の assert で、ファイルごとに別のプロセスで回す。
1 本でも落ちたら終了コード 1。
"""
import os
import subprocess
import sys
import tempfile
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DIRS = ["engine", "fxkit", "voice"]
TAIL_LINES = 25
# kit だけが持つファイル。公開側だけの確かめ（AK_PUBLIC_TEST）では、これを持つ sys.path の項目を外す
KIT_ONLY = ("fxparts.py", "ak_kit.py", "fxkit.py", "fxsound.py")
SITECUSTOMIZE = """import os, sys
_root = os.path.normcase(os.path.abspath(os.environ["AK_PUBLIC_ROOT"]))
_kit = %r
def _keep(p):
    try:
        d = os.path.abspath(p or ".")
    except Exception:
        return True
    if os.path.normcase(d).startswith(_root):
        return True
    return not any(os.path.exists(os.path.join(d, f)) for f in _kit)
sys.path[:] = [p for p in sys.path if _keep(p)]
""" % (KIT_ONLY,)


def kit_dirs():
    """非公開の kit のフォルダ（あれば）。fx-private/kit と、環境変数 AK_FX_PRIVATE"""
    c = [os.path.join(ROOT, "fx-private", "kit")]
    v = os.environ.get("AK_FX_PRIVATE")
    if v:
        c.append(v)
    out = []
    for d in c:
        d = os.path.abspath(d)
        if os.path.isdir(d) and d not in out:
            out.append(d)
    return out


def collect(args):
    """引数（フォルダかファイル）から、回すテストのパス（リポジトリ直下からの相対）を集める。"""
    targets = list(args or DIRS)
    if not args and not os.environ.get("AK_PUBLIC_TEST"):
        targets += kit_dirs()
    found = []
    for t in targets:
        p = t if os.path.isabs(t) else os.path.join(ROOT, t)
        if os.path.isdir(p):
            for name in sorted(os.listdir(p)):
                if name.startswith("test_") and name.endswith(".py"):
                    found.append(os.path.join(p, name))
        elif os.path.isfile(p):
            found.append(p)
        else:
            print(f"見つかりません: {t}", file=sys.stderr)
            sys.exit(2)
    return [os.path.relpath(f, ROOT).replace("\\", "/") if os.path.commonpath([ROOT, f]) == ROOT else f for f in found]


def main():
    for s in (sys.stdout, sys.stderr):
        if not s.isatty():
            s.reconfigure(encoding="utf-8", errors="replace")  # パイプ越し（Git Bash も）でも文字化けさせない
    files = collect(sys.argv[1:])
    if not files:
        print("テストが見つかりません", file=sys.stderr)
        return 2
    env = dict(os.environ, PYTHONUTF8="1")
    tmp = None
    if os.environ.get("AK_PUBLIC_TEST"):
        tmp = tempfile.mkdtemp(prefix="ak-sitecustomize-")
        with open(os.path.join(tmp, "sitecustomize.py"), "w", encoding="utf-8") as f:
            f.write(SITECUSTOMIZE)
        env["PYTHONPATH"] = os.pathsep.join(x for x in (tmp, env.get("PYTHONPATH")) if x)
        env["AK_PUBLIC_ROOT"] = ROOT
    failed = []
    for rel in files:
        t0 = time.time()
        r = subprocess.run(
            [sys.executable, rel], cwd=ROOT, env=env,
            capture_output=True, text=True, encoding="utf-8", errors="replace",
        )
        dt = time.time() - t0
        ok = r.returncode == 0
        print(f"{'OK' if ok else 'NG'}  {rel}  ({dt:.1f}s)")
        if not ok:
            failed.append(rel)
            out = (r.stdout + r.stderr).strip().splitlines()
            for line in out[-TAIL_LINES:]:
                print("    " + line)
    print(f"\n{len(files)} 本中 {len(files) - len(failed)} 本が成功、{len(failed)} 本が失敗")
    if failed:
        print("失敗: " + " ".join(failed))
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
