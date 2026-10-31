#!/usr/bin/env python3
"""
回の fxkit を固定する道具。

    python -m fxfreeze <回のフォルダ> [--force]

今見つかる fxkit（回の fx/ のコピーを除く。pip install -e した fxkit/ など）の
芯（fxcore・fxse）と kit（ak_kit・fxkit・fxparts・fxsound）のうち見つかったものと、その回の *_fx.py が import しているテンプレートを、回の fx/ に写す。
写した後は、その回は studio の fxkit/ が新しくなっても、写した版で描ける。

- すでにあるファイルが同じ中身なら何もしない。違うときは「違います」と出して、何も写さずに止まる（終了コード 1）。
- --force を付けると上書きする。上書きする前の中身は fx/_fxkit_backup_<日時>/ に残す。
"""
import argparse
import ast
import datetime
import glob
import importlib.util
import os
import shutil
import sys

# どの回にも写す、芯（公開）と kit。見つかったものだけ写す（公開版だけなら kit は無い）
CORE = ("fxcore", "fxse", "ak_kit", "ak_kit_basic", "fxkit", "fxparts", "fxsound")
# 消したモジュール（旧#98 の fxcli）。回の *_fx.py がまだ import していると、新しい fxkit を写した途端に開けなくなる
RETIRED = {"fxcli": "import fxcli の行と、if __name__ == \"__main__\": の fxcli.run(...) を消してください（確認は AK Studio で）"}


def find_source_dir(exclude_dir=None):
    """今 import できる fxcore.py（無ければ fxkit.py）のあるフォルダ（exclude_dir＝回の fx/ は除く）。見つからなければ None"""
    ex = os.path.normcase(os.path.abspath(exclude_dir)) if exclude_dir else None
    saved = sys.path[:]
    try:
        if ex:
            sys.path[:] = [p for p in sys.path if os.path.normcase(os.path.abspath(p or ".")) != ex]
        spec = None
        for name in ("fxcore", "fxkit"):
            sys.modules.pop(name, None)
            spec = importlib.util.find_spec(name)
            if spec is not None and spec.origin:
                break
    finally:
        sys.path[:] = saved
    if spec is None or not spec.origin:
        return None
    return os.path.dirname(os.path.abspath(spec.origin))


def imports(path):
    """path の .py が import しているモジュール名（いちばん上の名前）の集合"""
    try:
        tree = ast.parse(open(path, encoding="utf-8").read())
    except (OSError, SyntaxError, UnicodeDecodeError) as e:
        raise RuntimeError(f"{path} を読めませんでした: {e}")
    names = set()
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            names.update(a.name.split(".")[0] for a in node.names)
        elif isinstance(node, ast.ImportFrom) and node.module and node.level == 0:
            names.add(node.module.split(".")[0])
    return names


def local_imports(path, src_dir):
    """path の .py が import している、src_dir にある .py のモジュール名の集合"""
    return {n for n in imports(path) if os.path.isfile(os.path.join(src_dir, n + ".py"))}


def check_retired(fx_dir):
    """回の *_fx.py が消したモジュールを import していたら、何も写す前に止める（写すと開けなくなる）"""
    bad = []
    for f in sorted(glob.glob(os.path.join(fx_dir, "*_fx.py"))):
        for m in sorted(imports(f) & set(RETIRED)):
            bad.append(f"{os.path.basename(f)}: {m} は無くなりました。{RETIRED[m]}")
    if bad:
        raise RuntimeError("この回は今の fxkit に固定できません（何も写していません）:\n  " + "\n  ".join(bad))


def wanted_modules(fx_dir, src_dir):
    """写すモジュール名（CORE のうち src_dir にあるもの ＋ 回の *_fx.py が使うテンプレートなど。その import も辿る）"""
    todo = [m for m in CORE if os.path.isfile(os.path.join(src_dir, m + ".py"))]
    for f in sorted(glob.glob(os.path.join(fx_dir, "*_fx.py"))):
        todo.extend(sorted(local_imports(f, src_dir)))
    seen = []
    while todo:
        m = todo.pop(0)
        if m in seen:
            continue
        p = os.path.join(src_dir, m + ".py")
        if not os.path.isfile(p):
            raise RuntimeError(f"{m}.py が {src_dir} にありません")
        seen.append(m)
        todo.extend(sorted(local_imports(p, src_dir)))
    return seen


def same(a, b):
    with open(a, "rb") as fa, open(b, "rb") as fb:
        return fa.read() == fb.read()


def freeze(episode, force=False, src_dir=None, now=None):
    """回の fx/ に fxkit を写す。返り値: {"copied": [...], "same": [...], "differ": [...], "backup": フォルダ or None}
    違うファイルがあって force でなければ、何も写さず differ に入れて返す"""
    fx_dir = os.path.join(os.path.abspath(episode), "fx")
    if not os.path.isdir(fx_dir):
        raise RuntimeError(f"{fx_dir} がありません。回のフォルダを指定してください")
    check_retired(fx_dir)
    src_dir = src_dir or find_source_dir(fx_dir)
    if not src_dir:
        raise RuntimeError("fxcore（fxkit）が見つかりません。python -m pip install -e fxkit で入れてください")
    res = {"copied": [], "same": [], "differ": [], "backup": None, "source": src_dir}
    plan = []
    for m in wanted_modules(fx_dir, src_dir):
        s, d = os.path.join(src_dir, m + ".py"), os.path.join(fx_dir, m + ".py")
        if not os.path.exists(d):
            plan.append((s, d, False))
        elif same(s, d):
            res["same"].append(m + ".py")
        else:
            res["differ"].append(m + ".py")
            plan.append((s, d, True))
    if res["differ"] and not force:
        return res
    if res["differ"]:
        stamp = (now or datetime.datetime.now()).strftime("%Y%m%d_%H%M%S")
        bk = os.path.join(fx_dir, "_fxkit_backup_" + stamp)
        os.makedirs(bk, exist_ok=True)
        res["backup"] = bk
    names = [os.path.basename(d) for _, d, _ in plan]
    tmps = []
    done = []  # os.replace まで済んだ (d, exists)
    try:
        # 1. 控え（上書きする物）と、置き換え前の一時ファイルを全部作る。ここで失敗しても fx/ の本物には触れていない
        for s_, d, exists in plan:
            if exists:
                shutil.copy2(d, os.path.join(res["backup"], os.path.basename(d)))
        for s_, d, exists in plan:
            t = f"{d}.tmp-{os.getpid()}"
            tmps.append(t)
            shutil.copy2(s_, t)
        # 2. 置き換える。途中で失敗したら、置き換え済みの分を元に戻す
        for (s_, d, exists), t in zip(plan, tmps):
            os.replace(t, d)
            done.append((d, exists))
    except OSError as e:
        for t in tmps:
            try:
                if os.path.exists(t):
                    os.remove(t)
            except OSError:
                pass
        stuck = []  # 元に戻せなかった物
        for d, exists in reversed(done):
            try:
                if exists:
                    shutil.copy2(os.path.join(res["backup"], os.path.basename(d)), d)
                else:
                    os.remove(d)
            except OSError:
                stuck.append(os.path.basename(d))
        copied_names = [os.path.basename(d) for d, _ in done] if stuck else []
        raise FreezeError(
            f"{e}",
            not_copied=[n for n in names if n not in copied_names],
            copied=copied_names,
            stuck=stuck,
            backup=res["backup"],
        ) from e
    res["copied"] = names
    return res


class FreezeError(RuntimeError):
    """写している途中で失敗した。ふつうは何も写されていない状態に戻してある（戻せなかった物は stuck）"""

    def __init__(self, msg, not_copied, copied=(), stuck=(), backup=None):
        super().__init__(msg)
        self.not_copied, self.copied, self.stuck, self.backup = list(not_copied), list(copied), list(stuck), backup


def main(argv=None):
    ap = argparse.ArgumentParser(prog="fxfreeze", description="回の fx/ に今の fxkit を写して固定する")
    ap.add_argument("episode", help="回のフォルダ（中に fx/ がある）")
    ap.add_argument("--force", action="store_true", help="違うファイルがあっても上書きする（控えを fx/_fxkit_backup_<日時>/ に残す）")
    a = ap.parse_args(argv)
    # cp932 のコンソール（Git Bash など）でも文字化けしないように
    for st in (sys.stdout, sys.stderr):
        try:
            st.reconfigure(encoding="utf-8", errors="replace")
        except (AttributeError, ValueError):
            pass
    try:
        r = freeze(a.episode, a.force)
    except FreezeError as e:
        print(f"固定できませんでした: {e}", file=sys.stderr)
        if e.copied:
            print("写してあるファイル: " + ", ".join(e.copied), file=sys.stderr)
        print("写していないファイル: " + (", ".join(e.not_copied) or "なし"), file=sys.stderr)
        if e.stuck:
            print("元に戻せなかったファイル（控えから戻してください）: " + ", ".join(e.stuck), file=sys.stderr)
        if e.backup:
            print(f"上書き前の控え: {e.backup}", file=sys.stderr)
        return 1
    except (RuntimeError, OSError) as e:
        print(f"固定できませんでした: {e}", file=sys.stderr)
        return 1
    print(f"写し元: {r['source']}")
    if r["differ"] and not a.force:
        print("回の fx/ のファイルと中身が違います: " + ", ".join(r["differ"]), file=sys.stderr)
        print("何も写していません。上書きするには --force を付けてください（控えを残します）", file=sys.stderr)
        return 1
    for n in r["copied"]:
        print(f"写しました: {n}")
    for n in r["same"]:
        print(f"同じです: {n}")
    if r["backup"]:
        print(f"上書き前の控え: {r['backup']}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
