#!/usr/bin/env python3
"""
AK Studio の演出ワーカー。各回の fx/<名前>_fx.py（fxkit / fxparts のタイムライン）を読み込み、
Rust 側（AK Studio）からの依頼でフレームを描いて返す。

- 1 回の add(...) を 1 つの「クリップ」（group）として扱う。キーは「部品名@cue番号#その cue の中で何番目か」
  （例 telop@c15#1）。コードの行番号が変わってもキーは変わらない。
- 手動の微調整（fx/studio_overrides.json）はここでかける。タイムラインのコードは書き換えない。
    {"telop@c15#1": {"shift": 0.2, "end": -0.5, "dx": 30, "dy": -10, "scale": 1.1, "opacity": 0.9,
                     "pivot": [1240, 450], "hidden": false, "z": 25}}
  RecWindow（録画の窓）には、画面で差し込んだ録画（ネスト）が入っている区間を Rust 側が書く:
    {"RecWindow@c70#1": {"no_placeholder": [[412.3, 418.9]]}}   その区間は仮表示（ファイル名・説明）を描かない（true なら全部）
- 画面から足した簡単な部品（fx/studio_clips.json：テロップ・注釈・ハンコ・画像）もここで作る。
- 字幕の絵（op: subtitle / subtitle_measure）とフォントの一覧（op: fonts）も描く。こちらは演出の読み込みが要らない
  （Rust の subs.rs が、字幕だけのワーカーとして別に起こして使う）。

通信（標準入出力）:
  入力  1 行 1 つの JSON（{"id": 1, "op": "frame", ...}）
  出力  4 バイト（LE）のヘッダ長 + ヘッダ JSON（UTF-8）+ ヘッダの "nbytes" ぶんの生データ

単体で試すとき:
  python akfx_worker.py --describe <回のフォルダ>/fx/<名前>_fx.py
  python akfx_worker.py --still E:/.../shoheiken_fx.py 30.5 out.png
"""

import hashlib
import importlib.util
import inspect
import io
import json
import math
import os
import re
import struct
import sys
import time
import traceback
import types


def add_fxkit_fallback(environ=None):
    """アプリに埋め込んだ fxkit（環境変数 AK_FXKIT_FALLBACK）を sys.path の末尾に足す。
    回の fx/（load が先頭に入れる）と pip install -e した fxkit/ が先に見つかり、どちらも無いときだけ使われる"""
    d = (environ if environ is not None else os.environ).get("AK_FXKIT_FALLBACK")
    if d and d not in sys.path:
        sys.path.append(d)
    return d


add_fxkit_fallback()

# タイムラインの print が通信を壊さないよう、標準出力は stderr に回す
_OUT = sys.stdout.buffer
sys.stdout = sys.stderr

ENGINE_VERSION = "1"


def send(header, payload=b""):
    header = dict(header, nbytes=len(payload))
    hb = json.dumps(header, ensure_ascii=False).encode("utf-8")
    _OUT.write(struct.pack("<I", len(hb)))
    _OUT.write(hb)
    if payload:
        _OUT.write(payload)
    _OUT.flush()


# ======================================================================= 指紋（キャッシュの鍵）
_img_fp = {}


def _h(*parts):
    h = hashlib.blake2b(digest_size=10)
    for p in parts:
        h.update(p if isinstance(p, bytes) else str(p).encode("utf-8", "replace"))
        h.update(b"\x00")
    return h.hexdigest()


_cls_fp = {}


def class_fp(cls):
    """クラスのソース（El までの継承を含む）。描き方のコードを直したらキャッシュを捨てるため。"""
    if cls in _cls_fp:
        return _cls_fp[cls]
    parts = []
    for c in cls.__mro__:
        if c is object:
            continue
        try:
            parts.append(inspect.getsource(c))
        except (OSError, TypeError):
            parts.append(c.__qualname__)
    _cls_fp[cls] = _h(*parts)
    return _cls_fp[cls]


def fp(obj, depth=0, seen=None):
    """要素の中身の指紋。画像はピクセル、関数はコードとクロージャの中身まで見る。"""
    from PIL import Image
    if seen is None:
        seen = set()
    if obj is None or isinstance(obj, (bool, int, str)):
        return repr(obj)
    if isinstance(obj, float):
        return f"{obj:.6g}"
    if depth > 6:
        return type(obj).__name__
    oid = id(obj)
    if oid in seen:
        return "<cycle>"
    if isinstance(obj, Image.Image):
        k = _img_fp.get(oid)
        if k is None or k[0] is not obj:
            k = (obj, _h(obj.mode, obj.size, obj.tobytes()))
            _img_fp[oid] = k
        return "img:" + k[1]
    seen = seen | {oid}
    if isinstance(obj, (list, tuple, set, frozenset)):
        items = list(obj)
        if isinstance(obj, (set, frozenset)):
            items = sorted(items, key=repr)
        return "[" + ",".join(fp(x, depth + 1, seen) for x in items) + "]"
    if isinstance(obj, dict):
        return "{" + ",".join(f"{fp(k, depth + 1, seen)}:{fp(v, depth + 1, seen)}"
                              for k, v in sorted(obj.items(), key=lambda kv: repr(kv[0]))) + "}"
    try:
        import numpy as np
        if isinstance(obj, np.ndarray):
            return "nd:" + _h(obj.dtype, obj.shape, obj.tobytes())
        if isinstance(obj, np.generic):
            return repr(obj.item())
    except ImportError:
        pass
    if isinstance(obj, (types.FunctionType, types.MethodType)):
        f = obj.__func__ if isinstance(obj, types.MethodType) else obj
        code = f.__code__
        cl = [c.cell_contents for c in (f.__closure__ or ()) if _cell_ok(c)]
        return "fn:" + _h(code.co_code, repr(code.co_consts), code.co_names,
                          fp(cl, depth + 1, seen), fp(f.__defaults__, depth + 1, seen))
    if isinstance(obj, types.BuiltinFunctionType) or isinstance(obj, type):
        return getattr(obj, "__qualname__", repr(obj))
    if hasattr(obj, "func") and hasattr(obj, "args") and hasattr(obj, "keywords"):  # functools.partial
        return "partial:" + fp((obj.func, obj.args, obj.keywords), depth + 1, seen)
    if hasattr(obj, "__dict__"):
        d = {k: v for k, v in vars(obj).items() if not k.startswith("_ak")}
        return type(obj).__name__ + ":" + class_fp(type(obj)) + fp(d, depth + 1, seen)
    return type(obj).__name__  # フォントなど（中身は変わらない前提）


def _cell_ok(c):
    try:
        c.cell_contents
        return True
    except ValueError:
        return False


# ======================================================================= タイムラインの読み込み
class Group:
    """1 回の add(...) で入った要素のまとまり（画面ではクリップ 1 つ）。"""

    def __init__(self, gid, line, func, src, label):
        self.gid, self.line, self.func, self.src, self.label = gid, line, func, src, label
        self.els = []
        self.key = None


def _stmt_index(source):
    """モジュールのソースから、行番号 → (文の開始行, 文のソース) の対応を作る。"""
    import ast
    tree = ast.parse(source)
    lines = source.splitlines()
    idx = {}
    for node in ast.walk(tree):
        if isinstance(node, ast.stmt) and not isinstance(node, (ast.FunctionDef, ast.ClassDef, ast.For, ast.While,
                                                                ast.If, ast.With, ast.Try, ast.AsyncFunctionDef)):
            a, b = node.lineno, getattr(node, "end_lineno", node.lineno)
            src = "\n".join(lines[a - 1:b])
            for ln in range(a, b + 1):
                # 内側の文（小さい方）を優先
                if ln not in idx or (idx[ln][1] - idx[ln][0]) > (b - a):
                    idx[ln] = (a, b, src)
    return idx


_LABEL_RE = re.compile(r"\badd\s*\(\s*\*?\s*([A-Za-z_][\w\.]*)\s*\(")
_ADD_RE = re.compile(r"\b(?:tl\.)?add\s*\(")


def _label(src, func):
    m = _LABEL_RE.search(src)
    if m:
        return m.group(1).split(".")[-1]
    m = re.search(r"([A-Za-z_]\w*)\s*\(", src.replace("add(", "", 1))
    if func and func != "build":
        return func
    return m.group(1) if m else "clip"


# ======================================================================= 演出の kit
# kit の約束（版 1）は docs/spec/issues-167.md の 3 節。入口は ak_kit.py の 1 つで、無ければ ak_kit_basic（素の kit）。
# 部品の絵（シーン札・画面から足す演出・録画の窓）と効果音の当て方は kit が持ち、ワーカーは芯（fxcore）だけを直接使う。
SUPPORTED_KIT_API = {1}
KIT_ENTRIES = ("NAME", "scene_label", "studio_clip", "RecWindow", "se_collect", "se_labels", "SE_DIR", "FEATURES")
KIT_OPTIONAL = ("COLORS",)   # 画面から足す演出の色の名前の表。無ければ素の kit の表（警告は出さない）

_kit = None


def _kit_reset():
    """kit を読み直す（ワーカーの読み直しで、kit のモジュールを捨てたとき）。"""
    global _kit
    _kit = None


def _fxmod():
    """演出エンジンのモジュール（Timeline・El・Sprite・put・W/H など）。回が使っている fxkit があればそれ
    （fxfreeze で固定した古い fxkit.py は芯を自分で持っているので、回の Timeline と同じ物になる）、無ければ fxcore。"""
    try:
        import fxkit
        return fxkit
    except ModuleNotFoundError as e:
        if e.name != "fxkit":
            raise
    import fxcore
    return fxcore


def resolve_kit():
    """kit を決める。返り値は SimpleNamespace(name, api, features, path, warnings, 入口…)。
    warnings は [(key, message, level)]。kit が無い・版が合わない・入口が欠けているときは、その分だけ素の kit で描く。"""
    import importlib
    warns = []
    try:
        basic = importlib.import_module("ak_kit_basic")
    except Exception as e:
        # 回に固定した古い fxcore と合わないなど。ak_kit がそろっていれば、それだけで描く（足りない入口はその場で失敗して画面に出る）
        print("[kit] 素の kit を読み込めませんでした:", file=sys.stderr)
        traceback.print_exc()
        warns.append(("kit-basic", f"素の kit を読み込めませんでした: {type(e).__name__}: {e}", None))
        basic = None
    mod = None
    try:
        mod = importlib.import_module("ak_kit")
    except ModuleNotFoundError as e:
        if e.name == "ak_kit":
            warns.append(("kit-missing", "演出の kit がありません。素の見た目で描いています", "info"))
        else:
            warns.append(("kit-load", f"演出の kit を読み込めませんでした（{e.name} がありません）。素の見た目で描いています", None))
    except Exception as e:
        print("[kit] 演出の kit を読み込めませんでした:", file=sys.stderr)
        traceback.print_exc()
        warns.append(("kit-load", f"演出の kit を読み込めませんでした: {type(e).__name__}: {e}。素の見た目で描いています", None))
    if mod is not None:
        api = getattr(mod, "KIT_API", None)
        if not isinstance(api, int) or isinstance(api, bool) or api not in SUPPORTED_KIT_API:
            warns.append(("kit-api", f"演出の kit の版が合いません（kit: {'なし' if api is None else api}、"
                                     f"アプリ: {max(SUPPORTED_KIT_API)}）。kit かアプリを更新してください。素の見た目で描いています", None))
            mod = None
    if mod is None and basic is None:
        raise RuntimeError("演出の kit も素の kit も読み込めませんでした: " + "／".join(w[1] for w in warns))
    ns = types.SimpleNamespace(warnings=warns)
    for name in KIT_ENTRIES + KIT_OPTIONAL:
        if mod is not None and hasattr(mod, name):
            setattr(ns, name, getattr(mod, name))
        else:
            setattr(ns, name, getattr(basic, name, None))
            if mod is not None and name in KIT_ENTRIES:
                warns.append((f"kit-entry:{name}", f"演出の kit に {name} がありません。素の見た目で描いています", None))
    ns.name = str(ns.NAME)
    ns.api = getattr(mod, "KIT_API", None) if mod is not None else basic.KIT_API
    ns.features = set(ns.FEATURES or ())
    ns.path = getattr(mod if mod is not None else basic, "__file__", None)
    return ns


def get_kit():
    global _kit
    if _kit is None:
        _kit = resolve_kit()
    return _kit


class Loaded:
    def __init__(self):
        self.mod = None
        self.tl = None
        self.groups = []
        self.by_key = {}
        self.path = None
        self.error = None
        self.overrides = {}
        self.meta = {}
        self.build_sec = 0
        self.sync_path = None
        # 読み込みで起きた、止めるほどではない失敗（画面から追加した演出を作成できなかった など）。[{key, message}]
        self.warnings = []


L = Loaded()


def add_warning(lst, key, message, level=None):
    """警告を 1 件足す（同じ key は 1 回だけ）。画面は key と message で、前の読み込みから変わったかを見る。
    level は "info"（赤くしない知らせ）のときだけ付ける。無ければふつうの警告。"""
    if not any(w["key"] == key for w in lst):
        w = {"key": key, "message": message}
        if level:
            w["level"] = level
        lst.append(w)


def _fx_modules(fx_dir):
    fx_dir = os.path.normcase(os.path.abspath(fx_dir))
    out = []
    for name, m in list(sys.modules.items()):
        f = getattr(m, "__file__", None)
        if f and os.path.normcase(os.path.dirname(os.path.abspath(f))) == fx_dir:
            out.append(name)
    return out


_mtimes = {}


_cur_fx_dir = None


def _dir_changed(d):
    """d の中の .py が前に見たときから変わったか（初めて見たフォルダは変わっていない扱い）。"""
    changed = False
    try:
        names = os.listdir(d)
    except OSError:
        return False
    for fn in names:
        if fn.endswith(".py"):
            p = os.path.join(d, fn)
            try:
                mt = os.path.getmtime(p)
            except OSError:
                continue
            if _mtimes.get(p) not in (None, mt):
                changed = True
            _mtimes[p] = mt
    return changed


# 回をまたいだ読み直しの判断で見るモジュール（芯・演出の kit の入口・見た目つきの入口）
KIT_MODULES = ("fxcore", "ak_kit", "ak_kit_basic", "fxkit")
_seen_found = {}


def _fxkit_dir_loaded(name="fxkit"):
    """今読み込んである name（fxcore・ak_kit・fxkit）のフォルダ（読んでいなければ None）。"""
    m = sys.modules.get(name)
    f = getattr(m, "__file__", None) if m else None
    return os.path.dirname(os.path.abspath(f)) if f else None


def _fxkit_dir_found(name="fxkit"):
    """今の sys.path で import したら見つかる name のフォルダ。"""
    import importlib.machinery
    spec = importlib.machinery.PathFinder.find_spec(name, sys.path)
    return os.path.dirname(os.path.abspath(spec.origin)) if spec and spec.origin else None


def _purge_if_changed(fx_dir):
    """fx/ や fxkit（fxcore・ak_kit・fxkit）の .py が変わった・別の回に移った・見つかる場所が変わったときに、読み込み直す。
    ワーカーは回をまたいで使い回すので、前の回の fxkit（や pip で入れた版）を使い続けないようにする。"""
    global _cur_fx_dir
    fx_dir = os.path.abspath(fx_dir)
    dirs = {fx_dir}
    changed = _dir_changed(fx_dir)
    if _cur_fx_dir and os.path.normcase(_cur_fx_dir) != os.path.normcase(fx_dir):
        # 別の回: 前の回の fx/ を探す先から外し、そこから読んだものを捨てる
        while _cur_fx_dir in sys.path:
            sys.path.remove(_cur_fx_dir)
        dirs.add(_cur_fx_dir)
        changed = True
    _cur_fx_dir = fx_dir
    if not _seen_found and _kit is not None:
        changed = True  # 初めての load より前に（シーン札などで）kit を決めていた: 回の fx/ を探す先に入れて決め直す
    for mod in KIT_MODULES:
        loaded = _fxkit_dir_loaded(mod)
        found = _fxkit_dir_found(mod)
        if found and _dir_changed(found):  # 初めて見るときは今の時刻を覚えるだけ
            changed = True  # pip で入れた fxkit/ を直した
        if mod in _seen_found and _seen_found[mod] != found:
            changed = True  # kit が増えた・減った・見つかる場所が変わった
        _seen_found[mod] = found
        if loaded:
            dirs.add(loaded)
            if _dir_changed(loaded):
                changed = True
            if found and os.path.normcase(found) != os.path.normcase(loaded):
                changed = True  # 回の fx/ に fxkit を置いた（fxfreeze）・消した・別の回に移った
    if changed:
        for d in dirs:
            for name in _fx_modules(d):
                del sys.modules[name]
        _cls_fp.clear()
        _img_fp.clear()
        _kit_reset()
    return changed


def sync_arg_path(args, kwargs):
    """Timeline.__init__(self 以外の引数, キーワード) から sync（2 番目の位置引数 or sync=）の絶対パス。無ければ None"""
    v = kwargs["sync"] if "sync" in kwargs else (args[1] if len(args) > 1 else None)
    if isinstance(v, (str, os.PathLike)) and str(v):
        return os.path.abspath(os.fspath(v))
    return None


def load(path, overrides_path=None, clips_path=None, placeholder=True):
    t_start = time.time()
    path = os.path.abspath(path)
    fx_dir = os.path.dirname(path)
    if fx_dir not in sys.path:
        sys.path.insert(0, fx_dir)
    os.chdir(fx_dir)
    if placeholder:
        os.environ.pop("FX_NO_PLACEHOLDER", None)
    else:
        os.environ["FX_NO_PLACEHOLDER"] = "1"
    _purge_if_changed(fx_dir)
    source = open(path, encoding="utf-8").read()
    stmts = _stmt_index(source)

    name = "akfx_timeline_" + _h(path, time.time())
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    sys.modules[name] = mod
    spec.loader.exec_module(mod)

    TL = _fxmod().Timeline
    groups = []
    state = {"depth": 0}
    orig_add = TL.add
    mod_file = os.path.normcase(path)

    def add(self, *els):
        if state["depth"] > 0:
            return orig_add(self, *els)
        # 呼び出し元（タイムラインのファイルの中で一番内側の行）を探す
        f = sys._getframe(1)
        line, func = None, None
        while f is not None:
            if os.path.normcase(f.f_code.co_filename) == mod_file:
                line, func = f.f_lineno, f.f_code.co_name
                break
            f = f.f_back
        a, b, src = stmts.get(line, (line, line, "")) if line else (0, 0, "")
        g = Group(len(groups), a, func, src.strip(), _label(src, func))
        before = len(self.els)
        state["depth"] += 1
        try:
            r = orig_add(self, *els)
        finally:
            state["depth"] -= 1
        g.els = self.els[before:]
        # 部品が名前を持っていれば、それをクリップの名前にする（テンプレートの中から add されると、どれも同じ行の名前になるため）
        names = {getattr(e, "ak_label", None) for e in g.els}
        if len(names) == 1 and isinstance(next(iter(names)), str):
            g.label = next(iter(names))
        if g.els:
            groups.append(g)
        return r

    # Timeline(SRT, SYNC) に渡された同期点のファイルの場所を控える（画面で同期点を動かすとき、同じ場所の sync_manual.json に書く）
    # （build() の中で別の Timeline を作ることもあるので、インスタンスごとに控えて、返ってきた tl のものを使う）
    orig_init = TL.__init__

    def init(self, *a, **kw):
        self._ak_sync_path = sync_arg_path(a, kw)
        return orig_init(self, *a, **kw)

    TL.add = add
    TL.__init__ = init
    try:
        tl = mod.build()
    finally:
        TL.add = orig_add
        TL.__init__ = orig_init

    # 要素がリストの外で直接 tl.els に入れられた場合（まれ）
    known = {id(e) for g in groups for e in g.els}
    stray = [e for e in tl.els if id(e) not in known]
    if stray:
        g = Group(len(groups), 0, "build", "", "misc")
        g.els = stray
        groups.append(g)

    # 画面から足した部品
    warnings = []
    if clips_path and os.path.exists(clips_path):
        for g in gui_clips(tl, json.load(open(clips_path, encoding="utf-8")), warnings):
            g.gid = len(groups)
            groups.append(g)

    # キー: 部品名@c<cue>#<順番>
    starts = sorted((c["start"], n) for n, c in tl.cues.items())

    def cue_at(t):
        n0 = starts[0][1] if starts else 0
        for st, n in starts:
            if st <= t + 1e-6:
                n0 = n
            else:
                break
        return n0

    count = {}
    for g in groups:
        e0 = min(e.t0 for e in g.els)
        if g.key is None:
            base = f"{g.label}@c{cue_at(e0)}"
            count[base] = count.get(base, 0) + 1
            g.key = f"{base}#{count[base]}"
        for e in g.els:
            e._akg = g
            e._ak_t1 = e.t1
            if _is_rec(e):
                e._ak_ph = e.ph
    end = tl.end or max((e.t1 or 0) + e.out for e in tl.els)
    tl.end = end

    # 調整を先に確かめる（読めなければ、新しいタイムラインを L に入れる前に失敗する）
    ov = json.load(open(overrides_path, encoding="utf-8")) if overrides_path and os.path.exists(overrides_path) else {}
    _ov_plan(ov or {}, groups)
    L.mod, L.tl, L.groups, L.path, L.error = mod, tl, groups, path, None
    L.warnings = warnings
    L.sync_path = getattr(tl, "_ak_sync_path", None)
    L.by_key = {g.key: g for g in groups}
    L.meta = {k: getattr(mod, k) for k in ("NAME", "CUE_A", "CUE_B", "PREMIERE_T0", "PREMIERE_FPS", "BG_LOOP",
                                             "REC_VIDEO", "BG_FRAME") if isinstance(getattr(mod, k, None),
                                                                                   (str, int, float))}
    L.meta["art"] = "art=False" not in source
    set_overrides(ov)
    L.build_sec = round(time.time() - t_start, 2)
    return describe()


# 調整の数の項目。値が空（0・""・False など）なら既定とみなすもの（frame の `or 既定` と同じ）と、None だけを無しとみなすもの
_OV_NUM_FALSY = ("end", "shift", "dx", "dy", "scale")
_OV_NUM_NONE = ("z", "opacity")


def _ov_num(key, o, field):
    """調整の数の項目（無ければ None）。数でなければ ValueError（どのクリップのどの項目かを添える）"""
    v = o.get(field)
    if v is None or (field in _OV_NUM_FALSY and not v):
        return None
    try:
        f = float(v)
    except (TypeError, ValueError):
        f = None
    if f is None or not math.isfinite(f):
        raise ValueError(f"{key} の {field} が数ではありません: {v!r}")
    return f


def _ov_plan(ov, groups):
    """調整を確かめて、クリップごとの (グループ, end の量, z) を返す。読めない値があれば ValueError（旧#141）。
    frame・効果音が毎回 float() で読む項目（shift・dx・dy・scale・opacity）もここで確かめる"""
    if not isinstance(ov, dict):
        raise ValueError("調整は {クリップのキー: {...}} の形にしてください")
    plan = []
    for g in groups:
        o = ov.get(g.key) or {}
        if not isinstance(o, dict):
            raise ValueError(f"{g.key} の調整が {{...}} の形ではありません: {o!r}")
        for f in _OV_NUM_FALSY + _OV_NUM_NONE:
            _ov_num(g.key, o, f)
        plan.append((g, _ov_num(g.key, o, "end") or 0.0, _ov_num(g.key, o, "z")))
    return plan


def set_overrides(ov):
    """手動の微調整を入れ替える（作り直しは要らない）。end（終わりの調整）だけは要素の t1 を書き換える。
    読めない値があれば ValueError で、何も変えない（半分だけ入れ替えた調整で描かないため、旧#141）。"""
    ov = ov or {}
    plan = _ov_plan(ov, L.groups)
    L.overrides = ov
    for g, de, z in plan:
        for e in g.els:
            e.t1 = None if e._ak_t1 is None else max(e.t0 + 0.05, e._ak_t1 + de)
            if z is not None:
                if not hasattr(e, "_ak_z"):
                    e._ak_z = e.z
                e.z = z
            elif hasattr(e, "_ak_z"):
                e.z = e._ak_z


def _is_rec(e):
    """RecWindow（fxcore.RecWindowBase を継いだ物。とその仲間）か。窓の位置 box・区切り segs・仮表示 ph を持つもの。"""
    return hasattr(e, "segs") and hasattr(e, "box") and hasattr(e, "ph")


def _no_placeholder(o, t):
    """微調整の no_placeholder（true か [[開始, 終わり], …]、タイムラインの時刻）で、t に仮表示を消すか。"""
    v = o.get("no_placeholder")
    if v is True:
        return True
    if isinstance(v, list):
        for r in v:
            try:
                if float(r[0]) <= t < float(r[1]):
                    return True
            except (TypeError, ValueError, IndexError):
                continue
    return False


def rec_info(g):
    """RecWindow の窓の位置と区切り（録画を差し込む画面で使う）。"""
    out = []
    for e in g.els:
        if not _is_rec(e):
            continue
        try:
            segs = [[round(float(s[0]), 4)] + [str(x) for x in list(s[1:4])] for s in e.segs]
            # 入り・抜けは RecWindow の ENTER・ENTER_EASE・out・OUT_EASE。fxfreeze で止めた古い fxparts の RecWindow には
            # ENTER 等が無いので、当時の値（入り 0.35 秒の ease_out、抜け 0.25 秒の ease_in_out）をここで補う。
            # 今の値は fxcore.RecWindowBase（新しい fxcore・kit を使う回では、そちらの値が優先される）
            fade = {"in": float(getattr(e, "ENTER", 0.35)), "in_ease": str(getattr(e, "ENTER_EASE", "ease_out")),
                    "out": float(getattr(e, "out", 0.25)), "out_ease": str(getattr(e, "OUT_EASE", "ease_in_out"))}
            out.append({"box": [float(v) for v in e.box], "segs": segs,
                        "t1": None if e._ak_t1 is None else round(float(e._ak_t1), 4), "fade": fade})
        except Exception:
            traceback.print_exc()
    return out


def group_range(g):
    """オーバーライド前の、見えている区間（入り〜抜け終わり）。"""
    a = min(e.t0 for e in g.els)
    b = max((e._ak_t1 if e._ak_t1 is not None else L.tl.end) + getattr(e, "out", 0) for e in g.els)
    return a, b


_se_errors = set()


def se_of(els, warnings=None):
    """els に付く効果音。調べられないときは [] にして、warnings（あれば）に理由を足す。"""
    try:
        return get_kit().se_collect(els)
    except Exception as e:
        # 効果音が付かない理由を、演出エンジンのログ（stderr）に 1 回だけ出す
        msg = f"{type(e).__name__}: {e}"
        if msg not in _se_errors:
            _se_errors.add(msg)
            print(f"[se] 効果音の付け方を調べられません: {msg}", file=sys.stderr)
            traceback.print_exc()
        if warnings is not None:
            add_warning(warnings, "se", f"効果音の付け方を調べられませんでした（効果音は付きません）: {msg}")
        return []


def describe():
    tl = L.tl
    groups = []
    warnings = list(L.warnings)
    for g in L.groups:
        a, b = group_range(g)
        kinds = sorted({type(e).__name__ for e in g.els})
        try:
            body = fp([g.els], 0)
        except Exception as ex:  # 指紋が取れない要素は毎回描き直す
            body = f"nofp:{time.time()}:{ex}"
        z = min(getattr(e, "_ak_z", e.z) for e in g.els)
        x0, y0, x1, y1 = bbox_hint(g)
        groups.append({
            "key": g.key, "label": g.label, "func": g.func, "line": g.line, "src": g.src[:600],
            "t0": round(a, 4), "t1": round(b, 4), "n": len(g.els), "kinds": kinds, "z": z,
            "fp": _h(ENGINE_VERSION, body), "gui": g.key.startswith("gui:"),
            "se": [{"t": round(ev["t"], 3), "kind": ev["kind"]} for ev in se_of(g.els, warnings)],
            "bbox": [x0, y0, x1, y1] if x1 > x0 else None,
            "rec": rec_info(g),
        })
    cues = [{"n": n, "start": c["start"], "end": c["end"], "text": c["text"], "speaker": c["speaker"]}
            for n, c in sorted(tl.cues.items())]
    sync = [{"cue": k[0], "keyword": k[1], "t": r.get("t"), "t_end": r.get("t_end"),
             "method": r.get("method", "whisper")} for k, r in tl.sync.items()]
    used = [{"cue": n, "keyword": kw, "end": e, "t": t, "method": m} for n, kw, e, t, m in tl.sync_used]
    fx = _fxmod()
    kit = get_kit()
    for key, message, level in kit.warnings:
        add_warning(warnings, key, message, level)
    return {"ok": True, "path": L.path, "fxkit": getattr(fx, "__file__", None), "end": tl.end, "fps": getattr(tl, "fps", None) or fx.FPS, "w": fx.W, "h": fx.H,
            "groups": groups, "cues": cues, "sync": sync, "sync_used": used, "sync_path": L.sync_path, "meta": L.meta,
            "build_sec": L.build_sec, "stage": list(fx.STAGE), "rec_box": list(fx.REC_BOX),
            "kit": {"name": kit.name, "api": kit.api, "features": sorted(kit.features), "path": kit.path},
            "warnings": warnings}


def bbox_hint(g):
    """（x0, y0, x1, y1）の左上と右下。静止画の部品（Sprite）から、だいたいの位置（クリップを画面で選ぶときの当たり判定の目安）。"""
    xs0, ys0, xs1, ys1 = [], [], [], []
    for e in g.els:
        im = getattr(e, "im", None)
        if im is not None and hasattr(e, "x") and hasattr(im, "width"):
            xs0.append(e.x)
            ys0.append(e.y)
            xs1.append(e.x + im.width)
            ys1.append(e.y + im.height)
    if not xs0:
        return 0, 0, 0, 0
    return int(min(xs0)), int(min(ys0)), int(max(xs1)), int(max(ys1))


# ======================================================================= 描画
def frame(t, hide=(), solo=None, scale=1.0):
    fx = _fxmod()
    from PIL import Image
    W, H = fx.W, fx.H
    fr = fx.new(W, H)
    hide = set(hide or ())
    ov = L.overrides
    alive = []
    for e in L.tl.els:
        g = e._akg
        if solo is not None and g.key != solo:
            continue
        if g.key in hide:
            continue
        o = ov.get(g.key) or {}
        if o.get("hidden"):
            continue
        te = t - float(o.get("shift", 0) or 0)
        if e.alive(te):
            alive.append((e, g, o, te))
    alive.sort(key=lambda x: x[0].z)
    # 録画を差し込んだ区間の RecWindow は仮表示を描かない（RecWindow.draw は描くときに self.ph を見る）
    touched = []
    for e, g, o, te in alive:
        if getattr(e, "_ak_ph", False) and _no_placeholder(o, t):
            e.ph = False
            touched.append(e)
    try:
        done = set()
        for e, g, o, te in alive:
            tf = _transform(o)
            if tf is None:
                e.draw(fr, te)
                continue
            if g.key in done:
                continue
            done.add(g.key)
            lay = fx.new(W, H)
            for e2, g2, _, te2 in alive:
                if g2 is g:
                    e2.draw(lay, te2)
            _composite(fr, lay, tf)
    finally:
        for e in touched:   # 指紋（describe）が描いた後の状態に左右されないよう戻す
            e.ph = e._ak_ph
    if scale != 1.0:
        sw, sh = max(1, round(W * scale)), max(1, round(H * scale))
        if abs(scale - 0.5) < 1e-6:
            fr = fr.reduce(2)
        else:
            fr = fr.resize((sw, sh), Image.BILINEAR)
    return fr


def _transform(o):
    dx, dy = float(o.get("dx", 0) or 0), float(o.get("dy", 0) or 0)
    s = float(o.get("scale", 1) or 1)
    a = float(o.get("opacity", 1) if o.get("opacity") is not None else 1)
    if abs(dx) < 0.01 and abs(dy) < 0.01 and abs(s - 1) < 1e-4 and a >= 0.999:
        return None
    return dx, dy, s, a, o.get("pivot")


def _composite(fr, lay, tf):
    from PIL import Image
    dx, dy, s, a, pivot = tf
    bb = lay.getbbox()
    if not bb:
        return
    im = lay.crop(bb)
    x, y = bb[0], bb[1]
    if abs(s - 1) > 1e-4:
        px, py = pivot if pivot else ((bb[0] + bb[2]) / 2, (bb[1] + bb[3]) / 2)
        nw, nh = max(1, round(im.width * s)), max(1, round(im.height * s))
        im = im.resize((nw, nh), Image.BICUBIC if s > 1 else Image.LANCZOS)
        x = px + (bb[0] - px) * s
        y = py + (bb[1] - py) * s
    _fxmod().put(fr, im, x + dx, y + dy, a)


def frame_bytes(img):
    """透明でない所だけを切り出して返す（ほとんど透明なので軽くなる）。ヘッダの rect は [x, y, 幅, 高さ]。"""
    bb = img.getbbox()
    if not bb:
        return {"w": img.width, "h": img.height, "rect": None}, b""
    crop = img.crop(bb)
    return {"w": img.width, "h": img.height, "rect": [bb[0], bb[1], bb[2] - bb[0], bb[3] - bb[1]]}, crop.tobytes()


def scene_label_render(sid):
    """シーン番号の札（「SCENE 57-2」）の絵。番号は Rust が決める。subtitle と同じ形（ヘッダに w・h・rect、本体に生の RGBA）で返す"""
    return frame_bytes(get_kit().scene_label(str(sid)))


# ======================================================================= 効果音
# 効果音の設定（se/bank.json）の管理・試聴・書き出しは公開の fxkit/fxse.py（仕組み）。ワーカーは前提（SeCtx）を作って呼ぶだけ。
# どの動きにどの音を当てるか（se_collect）・種類の表示名（se_labels）・既定の棚（SE_DIR）は kit の入口（get_kit()）から。
# 素の kit は se_collect が [] で SE_DIR が None なので、空の bank で動く（自分の wav を足して鳴らせる）。
class SeError(ValueError):
    """効果音の設定の誤り・字幕の依頼の誤り（トレースバックを付けずに画面へ返す）。"""


def _fxse(fx_dir=None):
    """効果音の仕組みのモジュール（回の fx/ に固定した版があればそれ。無ければ kit・公開の fxkit/ の版）。"""
    if fx_dir and fx_dir not in sys.path:
        sys.path.insert(0, fx_dir)
    import fxse
    return fxse


def _se_fx_dir(fx_dir=None):
    if fx_dir:
        return os.path.abspath(fx_dir)
    if L.path:
        return os.path.dirname(L.path)
    raise RuntimeError("演出がまだ読み込まれていません（fx_dir を指定してください）")


# アプリの効果音の棚（全部の回で使う。攻略動画の操作の 4 つのグループ = fxse.SHELF_KINDS）。
# 棚の場所と、種類ごとの選び（settings.json の guide_se: {種類: {"src": "<種類>/<名前>.wav" か "synth", "db": 数}}）は
# アプリが se / se_bank / se_audition の要求に lib_dir・guide_se で渡す。回の se/bank.json にその種類があれば回のほうが勝つ。
SHELF = {"dir": None, "sel": {}}


def _shelf_set(req):
    """要求に付いてきた棚の場所と選びを覚える（無ければ棚は使わない）。"""
    d = req.get("lib_dir")
    SHELF["dir"] = os.path.abspath(d) if d else None
    sel = req.get("guide_se")
    SHELF["sel"] = sel if isinstance(sel, dict) else {}


def _se_events():
    """今の演出から鳴らす候補（kit の se_collect。微調整のずらし・隠すを入れる）。"""
    kit = get_kit()
    ev = []
    for g in L.groups:
        o = L.overrides.get(g.key) or {}
        if o.get("hidden"):
            continue
        sh = float(o.get("shift", 0) or 0)
        for d in kit.se_collect(g.els):
            d = dict(d)
            d["t"] += sh
            d["key"] = (d["kind"], round(d["t"], 3), str(d.get("x")))
            ev.append(d)
    return ev


def _se_ctx(fx_dir=None):
    """効果音の設定を扱う前提（fxse.SeCtx）と、fxse のモジュール。"""
    fx_dir = _se_fx_dir(fx_dir)
    fxse = _fxse(fx_dir)
    kit = get_kit()
    labels = []   # 1 回だけ kit に聞く

    def label(kind):
        if not labels:
            labels.append(dict(kit.se_labels()))
        return labels[0].get(kind, kind)

    def events():
        # この回で鳴る時刻（読み込んでいる演出が、この fx_dir のものであるときだけ）
        if L.tl is not None and L.path and os.path.normcase(os.path.dirname(L.path)) == os.path.normcase(fx_dir):
            return _se_events()
        return None

    ctx = fxse.SeCtx(fx_dir, default_dir=kit.SE_DIR, shelf_dir=SHELF["dir"], shelf_sel=SHELF["sel"], label=label, events=events)
    return ctx, fxse


def _se_call(fxse, fn, *args):
    """fxse の関数を呼ぶ。設定の誤り（fxse.SeError）は、画面へ返せるワーカーの SeError にする。"""
    try:
        return fn(*args)
    except fxse.SeError as e:
        raise SeError(str(e)) from None


def se_render(out_dir, name, profile="full"):
    ctx, fxse = _se_ctx()
    ev = _se_events()
    return _se_call(fxse, fxse.render, ctx, ev, L.tl.end, out_dir, name, profile)


def se_bank_info(fx_dir=None):
    """効果音の設定の一覧: 種類ごとの候補・鳴らす音・音量・この回で鳴る時刻。"""
    ctx, fxse = _se_ctx(fx_dir)
    return _se_call(fxse, fxse.bank_info, ctx)


def se_bank_set(kinds, fx_dir=None):
    """種類ごとの設定を変えて se/bank.json に書く。
    kinds: {種類: {"use": [候補, …], "db": {候補: dB か null}, "add": [候補], "remove": [候補]}}（変えるものだけ）"""
    ctx, fxse = _se_ctx(fx_dir)
    return _se_call(fxse, fxse.bank_set, ctx, kinds)


def se_audition(kind, src, out_wav, fx_dir=None):
    """1 つの候補を 0 dB（標準の大きさ）で鳴らした wav を書く。候補ごとの音量は再生側でかける。"""
    ctx, fxse = _se_ctx(fx_dir)
    return _se_call(fxse, fxse.audition, ctx, kind, src, out_wav)


# ======================================================================= 画面から足す部品
def _anchor(tl, a, default=0.0):
    if a is None:
        return default
    if isinstance(a, (int, float)):
        return float(a)
    t = a.get("t")
    if t is None and a.get("cue") is not None:
        n = int(a["cue"])
        if a.get("kw"):
            t = tl.s(n, a["kw"], end=bool(a.get("kw_end")))
        else:
            t = tl.c(n) if not a.get("cue_end") else tl.ce(n)
    return float(t or 0) + float(a.get("off", 0) or 0)


def _color(c, default):
    """画面から足した演出の色の指定（名前・#rrggbb・[r, g, b]）を (r, g, b) にする。名前の表は kit の COLORS"""
    if isinstance(c, str):
        named = get_kit().COLORS
        if c.lower() in named:
            return named[c.lower()]
        if c.startswith("#") and len(c) == 7:
            return tuple(int(c[i:i + 2], 16) for i in (1, 3, 5))
    if isinstance(c, (list, tuple)) and len(c) >= 3:
        return tuple(int(v) for v in c[:3])
    return default


def _studio_params(kind, p):
    """画面から足す 4 種類（telop・note・stamp・chips）の引数を、種類ごとに既定を入れて kit の studio_clip に渡す形にする。
    種類と引数はここ（公開側）で決める。kit は絵だけを描く。色（color・accent）は指定が無ければ None で、kit が見た目の色にする"""
    fx = _fxmod()
    if kind == "telop":
        return {"text": p.get("text", ""), "size": p.get("size", 56), "weight": p.get("weight", 900),
                "color": _color(p.get("color"), None), "sub": p.get("sub"),
                "tag": p.get("tag", "KEY POINT"), "cx": p.get("cx", fx.STAGE_CX), "cy": p.get("cy", 450),
                "accent": _color(p.get("accent"), None)}
    if kind == "note":
        return {"text": p.get("text", "※")}
    if kind == "stamp":
        return {"text": p.get("text", "OK"), "cx": p.get("cx", 1500), "cy": p.get("cy", 300),
                "color": _color(p.get("color"), None), "size": p.get("size", 64), "angle": p.get("angle", -7)}
    if kind == "chips":
        return {"words": p.get("words", []), "cx": p.get("cx", fx.STAGE_CX), "cy": p.get("cy", 440),
                "size": p.get("size", 56), "tag": p.get("tag")}
    raise KeyError(kind)


def gui_clips(tl, clips, warnings=None):
    """画面から追加した演出（fx/studio_clips.json）を作る。作成できないものは除いて、warnings（あれば）に理由を足す。"""
    fx = _fxmod()
    from PIL import Image
    out = []

    def where(c):
        # どの演出か見分けられるよう、種類と開始の時刻（秒で指定していれば）を添える
        st = c.get("start")
        at = f"・{float(st):.1f} 秒" if isinstance(st, (int, float)) else ""
        return f"{c.get('id')}（{c.get('type')}{at}）"

    for c in clips:
        if c.get("disabled"):
            continue
        try:
            t0 = _anchor(tl, c.get("start"))
            t1 = _anchor(tl, c.get("end"), t0 + 3.0)
            p = c.get("params", {})
            kind = c.get("type")
            els = []
            if kind in ("telop", "note", "stamp", "chips"):
                els = get_kit().studio_clip(kind, t0, t1, _studio_params(kind, p))
            elif kind == "image":
                im = Image.open(p["path"]).convert("RGBA")
                s = float(p.get("scale", 1))
                if s != 1:
                    im = im.resize((max(1, round(im.width * s)), max(1, round(im.height * s))), Image.LANCZOS)
                els = [fx.Sprite(im, p.get("x", 700), p.get("y", 200), t0, t1, enter=p.get("enter", "fade"),
                                 z=p.get("z", 20))]
            else:
                print(f"[gui] 画面から追加した演出 {c.get('id')} は種類 {kind} を知らないので除外しました", file=sys.stderr)
                if warnings is not None:
                    add_warning(warnings, f"gui:{c.get('id')}",
                                f"画面から追加した演出 {where(c)} を作成できませんでした（表示していません）: 対応していない種類です")
                continue
            if not isinstance(els, (list, tuple)):
                els = [els]
            els = [e for e in _flatten(els) if e is not None]
            tl.els.extend(els)
            g = Group(0, 0, "gui", json.dumps(c, ensure_ascii=False)[:600], kind)
            g.key = "gui:" + str(c.get("id"))
            g.els = els
            out.append(g)
        except Exception as e:
            print(f"[gui] 画面から追加した演出 {c.get('id')} を作れないので除外しました:", file=sys.stderr)
            traceback.print_exc()
            if warnings is not None:
                add_warning(warnings, f"gui:{c.get('id')}",
                            f"画面から追加した演出 {where(c)} を作成できませんでした（表示していません）: "
                            f"{type(e).__name__}: {e}")
    return out


def _flatten(x):
    for e in x:
        if isinstance(e, (list, tuple)):
            yield from _flatten(e)
        else:
            yield e


# ======================================================================= 字幕
# モニター・mp4・確認画像・字幕の透過 .mov のどれも、ここで描いた絵を使う（見た目を揃えるため）。
# 見た目（共通 → 話者 → セリフの重ね方と既定）は Rust（render/subs.rs）が決め、全項目が入った形で届く。
# ここでは重ねず、足りない項目があればエラーにする（黙って既定で描かない）。
SUB_KEYS = ("font", "weight", "size", "color", "stroke", "stroke_w", "shadow", "shadow_color", "shadow_x", "shadow_y",
            "shadow_blur", "y", "line_h", "margin_l", "margin_r", "align", "wrap")
_sub_fonts = {}
# フォントごとの警告（spec → [{key, message}]）。フォントは覚えておくので、2 回目からは描くたびにここから返す
_font_warn = {}
# 今の依頼で使ったフォント（subtitle / subtitle_measure の返事に、そのフォントの警告を付ける）
_req_fonts = set()


def font_warnings():
    """今の依頼で使ったフォントの警告。"""
    out = []
    for spec in sorted(_req_fonts, key=str):
        for w in _font_warn.get(spec, []):
            add_warning(out, w["key"], w["message"])
    return out


def _rgba_of(c, default=(255, 255, 255, 255)):
    """#rrggbb / #rrggbbaa / [r,g,b(,a)]"""
    if isinstance(c, str) and c.startswith("#") and len(c) in (7, 9):
        v = [int(c[i:i + 2], 16) for i in range(1, len(c), 2)]
        return tuple(v + [255] * (4 - len(v)))
    if isinstance(c, (list, tuple)) and len(c) >= 3:
        return tuple(int(x) for x in c[:4]) + ((255,) if len(c) == 3 else ())
    return default


def sub_font(spec, size, weight, fallback):
    """spec: フォントのファイル（.ttc は「パス#番号」）。可変フォントは太さの軸だけ weight にする。"""
    key = (spec, int(round(size)), int(round(weight)))
    _req_fonts.add(spec)
    f = _sub_fonts.get(key)
    if f is not None:
        return f
    from PIL import ImageFont
    path, idx = spec, 0
    m = re.match(r"^(.*)#(\d+)$", spec or "")
    if m:
        path, idx = m.group(1), int(m.group(2))
    try:
        f = ImageFont.truetype(path, int(round(size)), index=idx)
    except OSError:
        # 見つからないフォントは既定に戻す（別の PC で開いたときなど）
        print(f"[subtitle] フォントが見つからないので既定のフォントで描きます: {spec}", file=sys.stderr)
        add_warning(_font_warn.setdefault(spec, []), f"font:{spec}",
                    f"字幕のフォントを読み込めませんでした（既定のフォントで表示しています）: {spec}")
        f = ImageFont.truetype(fallback, int(round(size)))
    try:
        axes = f.get_variation_axes()
    except Exception:
        axes = None
    if axes:
        vals = []
        for a in axes:
            nm = a.get("name", b"")
            nm = (nm.decode("utf-8", "replace") if isinstance(nm, bytes) else str(nm)).lower()
            if nm in ("weight", "wght"):
                vals.append(max(a["minimum"], min(a["maximum"], weight)))
            else:
                vals.append(a.get("default", a["minimum"]))
        try:
            f.set_variation_by_axes(vals)
        except Exception as e:
            print(f"[subtitle] フォントの太さを変えられません（{spec}）: {e}", file=sys.stderr)
            add_warning(_font_warn.setdefault(spec, []), f"font_weight:{spec}",
                        f"字幕のフォントの太さを変更できませんでした（フォントの既定の太さで表示しています）: {spec}: {e}")
    if len(_sub_fonts) > 64:
        _sub_fonts.clear()
    _sub_fonts[key] = f
    return f


# 字幕の自動の折り返しの禁則（行頭に置かない文字・行末に置かない文字）
NO_START = set("、。，．,.・：；:;？！?!」』）)】〉》］]｝}〕゛゜ー～…‥ぁぃぅぇぉっゃゅょゎァィゥェォッャュョヮヵヶ々ゝゞヽヾ")
NO_END = set("「『（(【〈《［[｛{〔")


def wrap_text(text, tw, width):
    """幅 width に収まるよう 1 文字ずつ折り返す（禁則つき）。tw は文字列の幅を返す関数。
    - 行頭に来る句読点・閉じ括弧・小さい仮名などは、前の行の終わりにぶら下げる（少しはみ出してよい）
    - 行末に来る開き括弧は、次の行へ送る"""
    lines, line = [], ""
    for ch in text:
        if line and tw(line + ch) > width:
            if ch in NO_START:
                line += ch  # ぶら下げ
                continue
            if len(line) > 1 and line[-1] in NO_END:
                lines.append(line[:-1])
                line = line[-1] + ch
                continue
            lines.append(line)
            line = ch
            continue
        line += ch
    lines.append(line)
    return lines


def _hang_width(ln, tw):
    """はみ出しの判定に使う幅（行末にぶら下げた禁則の文字は数えない）"""
    core = ln.rstrip("".join(NO_START))
    return tw(core) if core else tw(ln)


def sub_style(st):
    """依頼の見た目。全項目が揃っていなければエラー（既定で補わない）。"""
    miss = [k for k in SUB_KEYS if not isinstance(st, dict) or st.get(k) is None]
    if miss:
        raise SeError("字幕の見た目に足りない項目があります: " + ", ".join(miss))
    return st


def sub_layout(text, st, manual, fallback_font):
    """行に分けて、各行の位置（1920x1080 の座標）を決める。manual なら改行は text の \\n のとおり（折り返さない）。
    fallback_font: 読めないフォントの代わり（Rust が送る既定のフォント）。"""
    from PIL import Image, ImageDraw
    st = sub_style(st)
    if not fallback_font:
        raise SeError("字幕の依頼に fallback_font がありません")
    f = sub_font(st["font"], st["size"], st["weight"], fallback_font)
    d = ImageDraw.Draw(Image.new("RGBA", (1, 1)))
    tw = lambda s: d.textlength(s, font=f)
    left, right = float(st["margin_l"]), 1920 - float(st["margin_r"])
    width = max(10.0, right - left)
    if manual or not st["wrap"]:
        lines = text.split("\n")
    else:
        lines = wrap_text(text.replace("\n", ""), tw, width)
    lh = float(st["line_h"])
    y0 = float(st["y"]) - lh * len(lines)
    out = []
    for i, ln in enumerate(lines):
        w = tw(ln)
        # 幅を超えた行は、ぶら下げた句読点を除いた本体で揃え、句読点だけを枠の外に出す（自動の折り返しのときだけ）。
        # 本体がはみ出していなければ、はみ出しに数えない
        wo = _hang_width(ln, tw) if not manual and st["wrap"] and w > width else w
        if st["align"] == "left":
            x = left
        elif st["align"] == "right":
            x = right - wo
        else:
            x = (left + right) / 2 - wo / 2
        out.append({"text": ln, "x": x, "y": y0 + lh * i, "w": w,
                    "over": bool(ln) and (x < left - 0.5 or x + wo > right + 0.5)})
    return st, f, out, (left, right)


def sub_info(st, out, lr):
    return {"lines": [[round(o["x"], 1), round(o["y"], 1), round(o["x"] + o["w"], 1), round(o["y"] + st["line_h"], 1)]
                      for o in out],
            "texts": [o["text"] for o in out], "overflow": [o["over"] for o in out],
            "l": lr[0], "r": lr[1]}


def sub_render(text, st, manual, fallback_font, scale=1.0):
    """字幕 1 枚。透明でない所だけ（rect）の RGBA と、行の位置を返す。"""
    from PIL import Image, ImageDraw, ImageFilter
    st, f, out, lr = sub_layout(text, st, manual, fallback_font)
    info = sub_info(st, out, lr)
    sw = max(0, int(round(float(st["stroke_w"]))))
    fill, stroke = _rgba_of(st["color"]), _rgba_of(st["stroke"], (30, 80, 200, 255))
    sh_a = max(0.0, min(1.0, float(st["shadow"] or 0)))
    sx, sy, blur = float(st["shadow_x"]), float(st["shadow_y"]), max(0.0, float(st["shadow_blur"]))
    probe = ImageDraw.Draw(Image.new("RGBA", (1, 1)))
    boxes = [probe.textbbox((o["x"], o["y"]), o["text"], font=f, stroke_width=sw) for o in out if o["text"]]
    if not boxes:
        return dict(info, rect=None), b""
    x0, y0 = min(b[0] for b in boxes), min(b[1] for b in boxes)
    x1, y1 = max(b[2] for b in boxes), max(b[3] for b in boxes)
    if sh_a > 0:
        pad = blur * 2 + 2
        x0, y0 = min(x0, x0 + sx - pad), min(y0, y0 + sy - pad)
        x1, y1 = max(x1, x1 + sx + pad), max(y1, y1 + sy + pad)
    x0, y0, x1, y1 = int(x0) - 2, int(y0) - 2, int(x1) + 3, int(y1) + 3
    im = Image.new("RGBA", (x1 - x0, y1 - y0), (0, 0, 0, 0))
    if sh_a > 0:
        sc = _rgba_of(st["shadow_color"], (0, 0, 0, 255))
        sc = sc[:3] + (int(round(sc[3] * sh_a)),)
        lay = Image.new("RGBA", im.size, (0, 0, 0, 0))
        dl = ImageDraw.Draw(lay)
        for o in out:
            if o["text"]:
                dl.text((o["x"] - x0 + sx, o["y"] - y0 + sy), o["text"], font=f, fill=sc, stroke_width=sw, stroke_fill=sc)
        if blur > 0:
            lay = lay.filter(ImageFilter.GaussianBlur(blur))
        im.alpha_composite(lay)
    d = ImageDraw.Draw(im)
    for o in out:
        if o["text"]:
            d.text((o["x"] - x0, o["y"] - y0), o["text"], font=f, fill=fill, stroke_width=sw, stroke_fill=stroke)
    bx, by = x0, y0
    if abs(scale - 1.0) > 1e-6:
        # 1920 で描いてから縮める（折り返しや位置が解像度で変わらないように）
        nx0, ny0 = int(round(x0 * scale)), int(round(y0 * scale))
        nx1, ny1 = int(round(x1 * scale)), int(round(y1 * scale))
        im = im.resize((max(1, nx1 - nx0), max(1, ny1 - ny0)), Image.LANCZOS)
        bx, by = nx0, ny0
    return dict(info, rect=[bx, by, im.width, im.height]), im.tobytes()


def list_fonts():
    """入っているフォント（Windows の Fonts と、ユーザーごとのフォント）。日本語が出せるかも調べる。"""
    from PIL import ImageFont
    try:
        from fontTools.ttLib import TTFont, TTCollection
    except ImportError:
        TTFont = TTCollection = None
    dirs = ["C:/Windows/Fonts"]
    la = os.environ.get("LOCALAPPDATA")
    if la:
        dirs.append(os.path.join(la, "Microsoft", "Windows", "Fonts").replace("\\", "/"))
    out = []
    for dd in dirs:
        if not os.path.isdir(dd):
            continue
        for fn in sorted(os.listdir(dd)):
            low = fn.lower()
            if not low.endswith((".ttf", ".otf", ".ttc")):
                continue
            path = f"{dd}/{fn}"
            jp_faces = None
            if TTFont is not None:
                try:
                    faces = TTCollection(path, lazy=True).fonts if low.endswith(".ttc") else [TTFont(path, lazy=True)]
                    jp_faces = []
                    for ft in faces:
                        cm = ft.getBestCmap() or {}
                        jp_faces.append(0x3042 in cm and 0x6F22 in cm)  # 「あ」「漢」
                except Exception:
                    jp_faces = None
            for idx in range(16 if low.endswith(".ttc") else 1):
                try:
                    f = ImageFont.truetype(path, 20, index=idx)
                except Exception:
                    break
                fam, sty = f.getname()
                wax = None
                try:
                    for a in f.get_variation_axes() or []:
                        nm = a.get("name", b"")
                        nm = (nm.decode("utf-8", "replace") if isinstance(nm, bytes) else str(nm)).lower()
                        if nm in ("weight", "wght"):
                            wax = [a["minimum"], a["maximum"], a.get("default", a["minimum"])]
                except Exception:
                    pass
                jp = jp_faces[idx] if jp_faces is not None and idx < len(jp_faces) else None
                out.append({"spec": path + (f"#{idx}" if low.endswith(".ttc") else ""), "family": fam, "style": sty,
                            "weight_axis": wax, "jp": jp, "file": fn})
    return out


# ======================================================================= メインループ
def serve():
    for raw in sys.stdin.buffer:
        line = raw.decode("utf-8").strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except Exception as e:
            send({"ok": False, "error": f"依頼の JSON を読めません: {e}"})
            continue
        rid = req.get("id")
        op = req.get("op")
        try:
            if op == "ping":
                send({"id": rid, "ok": True, "pid": os.getpid(), "engine": ENGINE_VERSION})
            elif op == "load":
                # 読み込みに失敗したとき、前のタイムラインのまま描き続けないよう先に捨てる
                L.tl = None
                try:
                    d = load(req["timeline"], req.get("overrides"), req.get("clips"),
                             req.get("placeholder", True))
                    send(dict(d, id=rid))
                except Exception:
                    L.error = traceback.format_exc()
                    send({"id": rid, "ok": False, "error": L.error})
            elif op == "describe":
                send(dict(describe(), id=rid))
            elif op == "overrides":
                try:
                    set_overrides(req.get("overrides") or {})
                except ValueError as e:
                    # 前の調整のまま。理由は本体が警告に出す
                    send({"id": rid, "ok": False, "error": str(e)})
                    continue
                send({"id": rid, "ok": True})
            elif op == "frame":
                if L.tl is None:
                    send({"id": rid, "ok": False, "error": "演出がまだ読み込まれていません"})
                    continue
                t0 = time.time()
                img = frame(float(req["t"]), req.get("hide"), req.get("solo"), float(req.get("scale", 1.0)))
                hdr, data = frame_bytes(img)
                hdr.update(id=rid, ok=True, ms=round((time.time() - t0) * 1000, 1))
                send(hdr, data)
            elif op == "se":
                _shelf_set(req)
                send(dict(se_render(req["out_dir"], req.get("name", "studio"), req.get("profile", "full")), id=rid,
                          ok=True))
            elif op == "se_bank":
                _shelf_set(req)
                send(dict(se_bank_info(req.get("fx_dir")), id=rid, ok=True))
            elif op == "se_bank_set":
                _shelf_set(req)
                send(dict(se_bank_set(req.get("kinds") or {}, req.get("fx_dir")), id=rid, ok=True))
            elif op == "se_audition":
                _shelf_set(req)
                send(dict(se_audition(req["kind"], req["src"], req["out"], req.get("fx_dir")), id=rid, ok=True))
            elif op == "still_png":
                img = frame(float(req["t"]), req.get("hide"), req.get("solo"), float(req.get("scale", 1.0)))
                b = io.BytesIO()
                img.save(b, "PNG")
                send({"id": rid, "ok": True}, b.getvalue())
            elif op == "subtitle":
                # 演出の読み込みは要らない（字幕だけのワーカーとしても使う）
                _req_fonts.clear()
                hdr, data = sub_render(req.get("text", ""), req.get("style"), bool(req.get("manual")),
                                       req.get("fallback_font"), float(req.get("scale", 1.0)))
                send(dict(hdr, id=rid, ok=True, warnings=font_warnings()), data)
            elif op == "scene_label":
                # シーン番号の札の絵だけ（番号は Rust が決める。演出の読み込みは要らない）
                hdr, data = scene_label_render(req["scene"])  # "id" は依頼の番号
                send(dict(hdr, id=rid, ok=True), data)
            elif op == "subtitle_measure":
                _req_fonts.clear()
                res = []
                for it in req.get("items", []):
                    st, _, out, lr = sub_layout(it.get("text", ""), it.get("style"), bool(it.get("manual")),
                                                   it.get("fallback_font"))
                    res.append(sub_info(st, out, lr))
                send({"id": rid, "ok": True, "items": res, "warnings": font_warnings()})
            elif op == "fonts":
                send({"id": rid, "ok": True, "fonts": list_fonts()})
            elif op == "quit":
                send({"id": rid, "ok": True})
                return
            else:
                send({"id": rid, "ok": False, "error": f"不明な op です: {op}"})
        except SeError as e:
            send({"id": rid, "ok": False, "error": str(e)})
        except Exception:
            send({"id": rid, "ok": False, "error": traceback.format_exc()})


def main():
    a = sys.argv[1:]
    if a and a[0] == "--describe":
        d = load(a[1])
        print(json.dumps({k: v for k, v in d.items() if k != "groups"}, ensure_ascii=False)[:1500])
        for g in d["groups"][:400]:
            print(f'{g["key"]:<34s} {g["t0"]:8.2f}-{g["t1"]:8.2f} n={g["n"]:3d} L{g["line"]:<5d} {g["fp"]} se={len(g["se"])}')
        print("groups", len(d["groups"]), "build", d["build_sec"])
        return
    if a and a[0] == "--still":
        load(a[1], os.path.join(os.path.dirname(a[1]), "studio_overrides.json"))
        frame(float(a[2])).save(a[3])
        return
    serve()


if __name__ == "__main__":
    main()
