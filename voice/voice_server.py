"""AK Studio と声のエンジン（VoiSona Talk など）をつなぐローカルサーバー。

ブラウザから声のエンジンの API へは直接つなげない（CORS）ので、このサーバーが間に入る。
アクセント確認の画面もこのサーバーから配信する（http://localhost:8765/review。/ はそこへ転送する）。
声のエンジンは Python のモジュール 1 つ（約束 v1。docs/spec/issues-8-12.md）。--engine で選ぶ。

    set VOISONA_USER=...   （VoiSona にサインインしているメールアドレス）
    set VOISONA_PASS=...   （VoiSona の環境設定 → API で決めたパスワード）
    python voice_server.py --project <回のフォルダ> [--port 8765] [--engine voisona|voicevox|<エンジンの .py>]

ふつうは AK Studio が起動する（--project にその回のフォルダが入る）。
作った音声は <プロジェクト>/voice_cache/ に、書き出した音声と SRT は <プロジェクト>/voice_out/ に置く。
プリセットはこのファイルと同じフォルダのものを、辞書は環境変数 AK_VOICE_DICT（無ければ %LOCALAPPDATA% の
ak-studio フォルダの voice_dict.json）を、全部の回で共通に使う。回のフォルダが並んでいる所は AK_AN_ROOT（無ければ --project の親）。
"""
import argparse
import hashlib
import importlib
import importlib.util
import json
import os
import re
import sys
import traceback
import wave
from array import array
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

TOOLS = Path(__file__).resolve().parent
sys.path.insert(0, str(TOOLS))
import voicecommon as vc  # noqa: E402

REVIEW = TOOLS / "accent_review.html"
PROJECT = CACHE = OUT = None  # main() で --project から決める

# ---------- 声のエンジン（約束 v1） ----------
ENGINE_API = 1
REQUIRED = ("ENGINE_API", "NAME", "LABEL", "FEATURES", "PRESETS", "RANGES", "check", "voice_info", "synthesize")
ENGINE = None      # 読んだエンジンのモジュール
ENGINE_DIR = TOOLS  # エンジンのファイルのあるフォルダ（プリセットはここの ENGINE.PRESETS）
ENGINE_PATH = None  # .py のパスで読んだときだけ、そのパス（resolve 済み。ping に出す）
ENGINE_ARG = None   # .py のパスで読んだときだけ、--engine に与えられた値のまま（ping に出す。本体が設定と比べる）
BUILTIN_ENGINES = ("voisona", "voicevox")  # 組み込みのエンジンの名前。パスで読むエンジンの NAME には使えない


def engine_fail(msg):
    print(f"声のエンジンを読めません: {msg}", file=sys.stderr)
    sys.exit(2)


def load_engine(value):
    """--engine の値（名前か .py のパス）からエンジンを読み、入口を確かめる。だめなら理由を出して sys.exit(2)。"""
    global ENGINE, ENGINE_DIR, ENGINE_PATH, ENGINE_ARG
    value = (value or "voisona").strip()
    try:
        if value.lower().endswith(".py"):
            path = Path(value).resolve()
            if not path.is_file():
                engine_fail(f"ファイルがありません: {path}")
            spec = importlib.util.spec_from_file_location(f"ak_voice_engine_{path.stem}", path)
            mod = importlib.util.module_from_spec(spec)
            sys.path.insert(0, str(path.parent))  # エンジンが同じフォルダの voicecommon などを読めるように
            spec.loader.exec_module(mod)
            folder, ep = path.parent, str(path)
        else:
            if not re.fullmatch(r"[a-z0-9_]+", value):
                engine_fail(f"エンジンの名前が正しくありません: {value}")
            mod = importlib.import_module(f"{value}_tts")
            folder, ep = TOOLS, None
    except ImportError as e:
        engine_fail(f"{value} を import できません: {e}")
    except Exception as e:  # エンジンのファイルの中の失敗も、理由を出して止める
        engine_fail(f"{value} の読み込みに失敗しました: {type(e).__name__}: {e}")
    missing = [n for n in REQUIRED if not hasattr(mod, n)]
    if missing:
        engine_fail(f"{value} に約束の入口がありません: {', '.join(missing)}")
    if mod.ENGINE_API != ENGINE_API:
        engine_fail(f"{value} の約束の版（ENGINE_API={mod.ENGINE_API}）に対応していません（対応は {ENGINE_API}）")
    if not isinstance(mod.NAME, str) or not re.fullmatch(r"[a-z0-9_]+", mod.NAME):
        engine_fail(f"{value} の NAME は英小文字・数字・_ だけにしてください: {mod.NAME!r}")
    if ep and mod.NAME in BUILTIN_ENGINES:
        # 同じ NAME だと、音声キャッシュの鍵・話者の声のファイルが組み込みのエンジンと混ざる
        engine_fail(f"{value} の NAME（{mod.NAME}）は組み込みのエンジンの名前です。別の名前にしてください（{'・'.join(BUILTIN_ENGINES)} 以外）")
    if not (folder / mod.PRESETS).is_file():
        engine_fail(f"話者のプリセット（{mod.PRESETS}）がありません: {folder}")
    ENGINE, ENGINE_DIR, ENGINE_PATH, ENGINE_ARG = mod, folder, ep, (value if ep else None)
    return mod


load_engine("voisona")  # 既定（--engine で main() が読み直す）


def features():
    return set(ENGINE.FEATURES)


def load_presets():
    return vc.load_presets(ENGINE_DIR / ENGINE.PRESETS)


def engine_info():
    info = {"name": ENGINE.NAME, "label": ENGINE.LABEL, "features": sorted(features()), "api": ENGINE_API}
    if ENGINE_PATH:
        info["path"] = ENGINE_PATH
        info["arg"] = ENGINE_ARG or ENGINE_PATH
    return info


def root():
    """回のフォルダが並んでいる所（AK_AN_ROOT。無ければ今の回の親）。"""
    env = os.environ.get("AK_AN_ROOT")
    return Path(env) if env else PROJECT.parent


def set_project(path):
    global PROJECT, CACHE, OUT
    PROJECT = Path(path).resolve()
    CACHE = PROJECT / "voice_cache"
    OUT = PROJECT / "voice_out"


def projects():
    """script/ フォルダがある回の一覧（新しい順）。"""
    found = [p for p in root().iterdir() if p.is_dir() and (p / "script").is_dir()]
    return [p.name for p in sorted(found, key=lambda p: p.name, reverse=True)]


def switch_project(body):
    name = body.get("name", "")
    path = root() / name if name else Path(body.get("path", ""))
    if not path.is_dir() or path.resolve() == TOOLS:
        raise ValueError(f"回のフォルダが見つかりません: {path}")
    set_project(path)
    return ping()


def ping():
    try:
        presets = episode_presets()
    except ValueError:  # 話者の声のファイルが書きかけでも、つながっていることは返す
        presets = load_presets()
    return {"ok": True, "project": PROJECT.name, "path": str(PROJECT), "dict": str(vc.dict_path()), "projects": projects(),
            "engine": engine_info(), "speakers": list(presets["speakers"]),
            "intonation": {sp: v["global_parameters"].get("intonation", 1.0)  # 抑揚の標準（話者の声の値）
                           for sp, v in presets["speakers"].items()}}


# ---------- 回ごとの話者の声（<回>/script/_voice_speakers*.json。AK Studio のトークトラックの ⚙ で決める） ----------
# 声の単位がエンジンで違う（VoiSona の音量 dB と VOICEVOX の倍率など）ので、エンジンごとに別のファイルに持つ
SPEAKERS = "_voice_speakers.json"  # voisona のファイル（今までのまま）


def speakers_file_name(name, path=None):
    """話者の声のファイル名。voisona は _voice_speakers.json、ほかは _voice_speakers.<名前>.json。
    名前は、.py のパスで読んだエンジンならそのファイル名の拡張子を除いた部分、組み込みならエンジンの名前。
    英数字・_・- 以外は落とし、空なら custom。AK Studio（script/store.rs の speakers_file）と同じ規則"""
    if path is None and name == "voisona":
        return SPEAKERS
    stem = name
    if path is not None:
        stem = path.replace("\\", "/").rsplit("/", 1)[-1]
        if stem.lower().endswith(".py"):
            stem = stem[:-3]
    stem = "".join(c for c in stem if c.isascii() and (c.isalnum() or c in "_-")) or "custom"
    return f"_voice_speakers.{stem}.json"


def speakers_file():
    """読み込んでいるエンジンの話者の声のファイル名。"""
    if ENGINE_PATH:
        return speakers_file_name(ENGINE.NAME, ENGINE_ARG or ENGINE_PATH)
    return speakers_file_name(ENGINE.NAME)


def speaker_overrides():
    """{話者: {"speed": …, "style_weights": […]}}。無ければ {}。読めなければ（書きかけなど）エラー。"""
    fname = speakers_file()
    path = PROJECT / "script" / fname
    if not path.exists():
        return {}
    text = path.read_text(encoding="utf-8-sig").strip()
    if not text:
        return {}
    try:
        d = json.loads(text)
    except ValueError:
        raise ValueError(f"{fname} が読めません（書きかけかもしれません）") from None
    return d if isinstance(d, dict) else {}


def usable_overrides(over):
    """エンジンが使えない調整（styles が無いときの style_weights）を落とした、話者の声の調整。"""
    if "styles" in features():
        return over
    return {sp: {k: v for k, v in o.items() if k != "style_weights"} if isinstance(o, dict) else o
            for sp, o in over.items()}


def apply_speakers(base, over):
    return vc.apply_speakers(base, usable_overrides(over), ENGINE.RANGES)


def episode_presets():
    """共通のプリセットに、この回の話者の声の調整を重ねたもの。音声キャッシュの鍵もこれで決まる。"""
    return apply_speakers(load_presets(), speaker_overrides())


def presets_for(item, base=None):
    """1 行分のプリセットと、その話者の声（調整を重ねた global_parameters）。

    item に speaker_params（AK Studio が送る、その話者の調整）があればそれを使い、無ければ今のエンジンの話者の声のファイル。
    """
    base = base or load_presets()
    sp = item.get("speaker")
    if "speaker_params" in item:
        over = {sp: item.get("speaker_params") or {}}
    else:
        over = speaker_overrides()
    presets = apply_speakers(base, over)
    return presets, presets["speakers"].get(sp, {}).get("global_parameters", {})


def speakers_info():
    """話者ごとの、共通のプリセットの声・スタイルの名前・この回の調整・範囲。"""
    presets, over, out = load_presets(), speaker_overrides(), {}
    for sp, p in presets["speakers"].items():
        gp = dict(p.get("global_parameters", {}))
        try:
            info = ENGINE.voice_info(p) or {}
            names, err = info.get("style_names") or [], None
        except Exception as e:  # 声のエンジンが止まっていても、プリセットにある名前で出す
            info, names, err = {}, [], str(e)
        labels = p.get("_style_names") or []
        if not names:
            names = labels
        if len(labels) != len(names):
            labels = names
        if not isinstance(gp.get("style_weights"), list) and info.get("default_style_weights"):
            gp["style_weights"] = info["default_style_weights"]
        out[sp] = {"voice_name": p.get("voice_name", ""), "base": gp, "style_names": names, "style_labels": labels,
                   "override": over.get(sp) or {}, "error": err}
    return {"speakers": out, "range": ENGINE.RANGES, "file": speakers_file()}


# ---------- アクセントのレビュー（accent_review.html と voice_review.py） ----------
def review_dir():
    return PROJECT / "voice_review"


def read_json(path):
    return json.loads(path.read_text(encoding="utf-8")) if path.exists() else None


def get_review():
    return {"project": PROJECT.name, "proposal": read_json(review_dir() / "proposal.json"),
            "result": read_json(review_dir() / "result.json")}


def save_review(body):
    review_dir().mkdir(exist_ok=True)
    path = review_dir() / "result.json"
    path.write_text(json.dumps(body, ensure_ascii=False, indent=1), encoding="utf-8")
    return {"path": str(path)}


def cache_key(speaker, text, analyzed_text, presets, entries, params=None):
    rel = vc.relevant(entries, text)
    parts = [3, speaker, text, analyzed_text or "", presets["speakers"][speaker], presets["pauses"], rel]
    if ENGINE.NAME != "voisona":  # VoiSona の鍵は今までと同じ（今の声のキャッシュをそのまま使う）
        parts.append({"engine": ENGINE.NAME})
    if rel and not analyzed_text and "accent" in features():
        parts.append(getattr(ENGINE, "ACCENT_RULE", 1))  # 辞書の当て方を変えたら、辞書が関わる行だけ作り直す
    params = vc.clean_params(params)
    if params:  # 抑揚・音素の長さ（無い行のキーは今までと同じ）
        parts.append({"params": params})
    src = json.dumps(parts, ensure_ascii=False, sort_keys=True)
    return hashlib.sha1(src.encode("utf-8")).hexdigest()[:16]


def with_extra(entries, extra):
    """辞書に、試したい項目（dict_extra）を表記で上書きして足す。"""
    if not extra:
        return entries
    over = {e["surface"] for e in extra}
    return [e for e in entries if e.get("surface") not in over] + list(extra)


def effective(body):
    """エンジンが使えない入力（FEATURES に無いもの）を落とした (analyzed_text, params)。synth と lookup の両方で使う。"""
    feats = features()
    analyzed = body.get("analyzed_text") if "accent" in feats else None
    params = dict(body.get("params") or {})
    if "durations" not in feats:
        params.pop("durations", None)
    if "intonation" not in feats:
        params.pop("intonation", None)
    return analyzed, params or None


def synth(body):
    presets, applied = presets_for(body)
    speaker, text = body["speaker"], body["text"].strip()
    if speaker not in presets["speakers"]:
        raise ValueError(f"プリセットにない話者です: {speaker}")
    if not text:
        raise ValueError("セリフが空です")
    entries = with_extra(vc.load_dict(), body.get("dict_extra"))
    analyzed, params = effective(body)
    key = cache_key(speaker, text, analyzed, presets, entries, params)
    meta = CACHE / f"{key}.json"
    if meta.exists():
        return dict(json.loads(meta.read_text(encoding="utf-8")), speaker_params=applied)
    CACHE.mkdir(exist_ok=True)
    r = ENGINE.synthesize(speaker, text, CACHE / f"{key}.wav", presets, analyzed, entries, params)
    r.update(id=key, speaker=speaker, text=text)
    meta.write_text(json.dumps(r, ensure_ascii=False), encoding="utf-8")
    return dict(r, speaker_params=applied)  # 使った話者の声（AK Studio が、古い中継でないかを確かめる）


def lookup(body):
    """作り直さずに、キャッシュにある解析結果だけを返す（無ければ null）。"""
    base, entries, out = load_presets(), vc.load_dict(), []
    for it in body["items"]:
        text = (it.get("text") or "").strip()
        if it.get("speaker") not in base["speakers"] or not text:
            out.append(None)
            continue
        presets, applied = presets_for(it, base)
        analyzed, params = effective(it)
        meta = CACHE / f"{cache_key(it['speaker'], text, analyzed, presets, entries, params)}.json"
        out.append(dict(json.loads(meta.read_text(encoding="utf-8")), speaker_params=applied) if meta.exists() else None)
    return {"results": out}


def read_wav(path):
    with wave.open(str(path)) as w:
        assert w.getsampwidth() == 2 and w.getnchannels() == 1, "16bit モノラルのみ対応"
        return w.getframerate(), array("h", w.readframes(w.getnframes()))


def write_wav(path, rate, samples):
    with wave.open(str(path), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(rate)
        w.writeframes(samples.tobytes())


def srt_time(sec):
    ms = round(sec * 1000)
    h, ms = divmod(ms, 3600_000)
    m, ms = divmod(ms, 60_000)
    s, ms = divmod(ms, 1000)
    return f"{h:02d}:{m:02d}:{s:02d},{ms:03d}"


def write_srt(path, cues):
    path.write_text("".join(f"{i}\n{srt_time(a)} --> {srt_time(b)}\n{t}\n\n" for i, (a, b, t) in enumerate(cues, 1)),
                    encoding="utf-8")


def assemble(body):
    """作った音声を並べ、話者ごとの音声・全体の音声（フルWAV）・SRT を書き出す。

    各 item に start（秒）があればその時刻に置く（トラックビューの並び）。無ければ順に gap 秒ずつあけて並べる。
    """
    presets = load_presets()
    gap = float(body.get("gap", presets.get("line_gap", 0.3)))
    name = re.sub(r'[\\/:*?"<>|]', "_", body.get("name") or "talk")
    clips, rate = [], None
    for it in body["items"]:
        r, s = read_wav(CACHE / f"{it['id']}.wav")
        if rate and r != rate:
            raise ValueError("サンプリングレートがそろっていません")
        rate = r
        clips.append((it, s))
    if not clips:
        raise ValueError("書き出すセリフがありません")

    placed, pos = [], 0
    for it, s in clips:
        if "start" in it:
            pos = int(round(float(it["start"]) * rate))
        placed.append((pos, it, s))
        pos += len(s) + int(gap * rate)
    total = max(p + len(s) for p, _, s in placed)

    speakers = list(dict.fromkeys(it["speaker"] for _, it, _ in placed))
    tracks = {sp: array("h", bytes(total * 2)) for sp in speakers}
    spans = sorted((p, p + len(s)) for p, _, s in placed)
    overlap = any(spans[i][1] > spans[i + 1][0] for i in range(len(spans) - 1))
    mix = array("i", bytes(total * 4)) if overlap else array("h", bytes(total * 2))
    cues, per_sp = [], {sp: [] for sp in speakers}
    for p, it, s in placed:
        tr = tracks[it["speaker"]]
        if not overlap:  # 重ならなければ置くだけ（速い）
            tr[p:p + len(s)] = s
            mix[p:p + len(s)] = s
        else:  # 重なるところは足し合わせる
            for i, v in enumerate(s):
                tr[p + i] = max(-32768, min(32767, tr[p + i] + v))
                mix[p + i] += v
        a, b = p / rate, (p + len(s)) / rate
        cues.append((a, b, it["subtitle"]))
        per_sp[it["speaker"]].append((a, b, it["subtitle"]))
    cues.sort()

    OUT.mkdir(exist_ok=True)
    files = []
    if overlap:
        mix = array("h", (max(-32768, min(32767, v)) for v in mix))
    write_wav(OUT / f"{name}_full.wav", rate, mix)
    files.append(f"{name}_full.wav")
    for sp in speakers:
        write_wav(OUT / f"{name}_{sp}.wav", rate, tracks[sp])
        write_srt(OUT / f"{name}_{sp}.srt", per_sp[sp])
        files += [f"{name}_{sp}.wav", f"{name}_{sp}.srt"]
    write_srt(OUT / f"{name}.srt", cues)
    files.append(f"{name}.srt")
    return {"dir": str(OUT), "files": files, "seconds": round(total / rate, 2), "lines": len(clips)}


class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        pass

    def send(self, code, body, ctype="application/json; charset=utf-8"):
        data = body if isinstance(body, bytes) else json.dumps(body, ensure_ascii=False).encode("utf-8")
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        path = self.path.split("?")[0]
        if path in ("/", "/script_editor.html"):  # 昔の台本エディタの画面は無くなった。アクセント確認へ
            self.send_response(302)
            self.send_header("Location", "/review")
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        if path in ("/review", "/accent_review.html"):
            return self.send(200, REVIEW.read_bytes(), "text/html; charset=utf-8")
        if path == "/api/review":
            return self.send(200, get_review())
        if path == "/api/dict":
            return self.send(200, {"entries": vc.load_dict()})
        if path == "/api/ping":
            return self.send(200, ping())
        if path == "/api/speakers":
            try:
                return self.send(200, speakers_info())
            except Exception as e:
                traceback.print_exc()
                return self.send(500, {"error": str(e)})
        m = re.fullmatch(r"/audio/([0-9a-f]{16})\.wav", path)
        if m and (CACHE / f"{m[1]}.wav").exists():
            return self.send(200, (CACHE / f"{m[1]}.wav").read_bytes(), "audio/wav")
        self.send(404, {"error": "not found"})

    def do_POST(self):
        try:
            body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))) or b"{}")
            if self.path == "/api/synth":
                return self.send(200, synth(body))
            if self.path == "/api/lookup":
                return self.send(200, lookup(body))
            if self.path == "/api/assemble":
                return self.send(200, assemble(body))
            if self.path == "/api/project":
                return self.send(200, switch_project(body))
            if self.path == "/api/review/result":
                return self.send(200, save_review(body))
            if self.path == "/api/dict":
                vc.save_dict(body["entries"])
                return self.send(200, {"entries": vc.load_dict()})
            self.send(404, {"error": "not found"})
        except Exception as e:  # エディタにそのまま見せる
            traceback.print_exc()
            self.send(500, {"error": str(e)})


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=8765)
    ap.add_argument("--project", default=".", help="その回のフォルダ（voice_cache/ と voice_out/ を置く場所）")
    ap.add_argument("--engine", default="voisona", help="声のエンジン。voisona・voicevox、またはエンジンの .py のパス")
    args = ap.parse_args()
    port = args.port
    project = Path(args.project).resolve()
    set_project(project)
    load_engine(args.engine)
    try:
        ENGINE.check()  # つながらない・認証情報が無いときはここで止める
    except RuntimeError as e:
        print(str(e), file=sys.stderr)
        sys.exit(2)
    sys.stdout.reconfigure(errors="replace")
    print(f"声のエンジン: {ENGINE.LABEL}")
    print(f"プロジェクト: {PROJECT}")
    print(f"辞書: {vc.dict_path()}")
    print(f"アクセントの確認: http://localhost:{port}/review  （止めるときは Ctrl+C）")
    HTTPServer(("127.0.0.1", port), Handler).serve_forever()


if __name__ == "__main__":
    main()
