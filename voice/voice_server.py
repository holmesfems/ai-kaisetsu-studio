"""AK Studio と VoiSona Talk をつなぐローカルサーバー。

ブラウザから VoiSona の API へは直接つなげない（CORS）ので、このサーバーが間に入る。
アクセント確認の画面もこのサーバーから配信する（http://localhost:8765/review。/ はそこへ転送する）。

    set VOISONA_USER=...   （VoiSona にサインインしているメールアドレス）
    set VOISONA_PASS=...   （VoiSona の環境設定 → API で決めたパスワード）
    python voice_server.py --project <回のフォルダ> [--port 8765]

ふつうは AK Studio が起動する（--project にその回のフォルダが入る）。
作った音声は <プロジェクト>/voice_cache/ に、書き出した音声と SRT は <プロジェクト>/voice_out/ に置く。
プリセットはこのファイルと同じフォルダのものを、辞書は環境変数 AK_VOICE_DICT（無ければ %LOCALAPPDATA% の
ak-studio フォルダの voice_dict.json）を、全部の回で共通に使う。回のフォルダが並んでいる所は AK_AN_ROOT（無ければ --project の親）。
"""
import argparse
import hashlib
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
import voisona_tts as vt  # noqa: E402

REVIEW = TOOLS / "accent_review.html"
PROJECT = CACHE = OUT = None  # main() で --project から決める


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
    except ValueError:  # _voice_speakers.json が書きかけでも、つながっていることは返す
        presets = vt.load_presets()
    return {"ok": True, "project": PROJECT.name, "path": str(PROJECT), "dict": str(vt.dict_path()), "projects": projects(),
            "speakers": list(presets["speakers"]),
            "intonation": {sp: v["global_parameters"].get("intonation", 1.0)  # 抑揚の標準（話者の声の値）
                           for sp, v in presets["speakers"].items()}}


# ---------- 回ごとの話者の声（<回>/script/_voice_speakers.json。AK Studio のトークトラックの ⚙ で決める） ----------
SPEAKERS = "_voice_speakers.json"


def speaker_overrides():
    """{話者: {"speed": …, "style_weights": […]}}。無ければ {}。読めなければ（書きかけなど）エラー。"""
    path = PROJECT / "script" / SPEAKERS
    if not path.exists():
        return {}
    text = path.read_text(encoding="utf-8-sig").strip()
    if not text:
        return {}
    try:
        d = json.loads(text)
    except ValueError:
        raise ValueError(f"{SPEAKERS} が読めません（書きかけかもしれません）") from None
    return d if isinstance(d, dict) else {}


def episode_presets():
    """共通のプリセットに、この回の話者の声の調整を重ねたもの。音声キャッシュの鍵もこれで決まる。"""
    return vt.apply_speakers(vt.load_presets(), speaker_overrides())


def presets_for(item, base=None):
    """1 行分のプリセットと、その話者の声（調整を重ねた global_parameters）。

    item に speaker_params（AK Studio が送る、その話者の調整）があればそれを使い、無ければ _voice_speakers.json。
    """
    base = base or vt.load_presets()
    sp = item.get("speaker")
    if "speaker_params" in item:
        over = {sp: item.get("speaker_params") or {}}
    else:
        over = speaker_overrides()
    presets = vt.apply_speakers(base, over)
    return presets, presets["speakers"].get(sp, {}).get("global_parameters", {})


VOICE_INFO = {}  # voice_name → VoiSona の声の情報（スタイルの名前など。起動中は覚えておく）


def voice_info(name):
    if name not in VOICE_INFO:
        items = vt.call("GET", "/voices").get("items", [])
        ver = next((v["voice_version"] for v in items if v.get("voice_name") == name), None)
        if ver is None:
            raise ValueError(f"VoiSona に声のライブラリがありません: {name}")
        VOICE_INFO[name] = vt.call("GET", f"/voices/{name}/{ver}")
    return VOICE_INFO[name]


def speakers_info():
    """話者ごとの、共通のプリセットの声・スタイルの名前・この回の調整・範囲。"""
    presets, over, out = vt.load_presets(), speaker_overrides(), {}
    for sp, p in presets["speakers"].items():
        gp = dict(p.get("global_parameters", {}))
        try:
            info = voice_info(p["voice_name"])
            names, err = info.get("style_names") or [], None
        except Exception as e:  # VoiSona が止まっていても、プリセットにある名前で出す
            info, names, err = {}, [], str(e)
        labels = p.get("_style_names") or []
        if not names:
            names = labels
        if len(labels) != len(names):
            labels = names
        if not isinstance(gp.get("style_weights"), list) and info.get("default_style_weights"):
            gp["style_weights"] = info["default_style_weights"]
        out[sp] = {"voice_name": p["voice_name"], "base": gp, "style_names": names, "style_labels": labels,
                   "override": over.get(sp) or {}, "error": err}
    return {"speakers": out, "range": vt.GLOBAL_RANGE, "file": SPEAKERS}


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
    rel = vt.relevant(entries, text)
    parts = [3, speaker, text, analyzed_text or "", presets["speakers"][speaker], presets["pauses"], rel]
    if rel and not analyzed_text:
        parts.append(vt.ACCENT_RULE)  # 辞書の当て方を変えたら、辞書が関わる行だけ作り直す
    params = vt.clean_params(params)
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


def synth(body):
    presets, applied = presets_for(body)
    speaker, text = body["speaker"], body["text"].strip()
    if speaker not in presets["speakers"]:
        raise ValueError(f"プリセットにない話者です: {speaker}")
    if not text:
        raise ValueError("セリフが空です")
    entries = with_extra(vt.load_dict(), body.get("dict_extra"))
    key = cache_key(speaker, text, body.get("analyzed_text"), presets, entries, body.get("params"))
    meta = CACHE / f"{key}.json"
    if meta.exists():
        return dict(json.loads(meta.read_text(encoding="utf-8")), speaker_params=applied)
    CACHE.mkdir(exist_ok=True)
    r = vt.synthesize(speaker, text, CACHE / f"{key}.wav", presets, body.get("analyzed_text"), entries,
                      body.get("params"))
    r.update(id=key, speaker=speaker, text=text)
    meta.write_text(json.dumps(r, ensure_ascii=False), encoding="utf-8")
    return dict(r, speaker_params=applied)  # 使った話者の声（AK Studio が、古い中継でないかを確かめる）


def lookup(body):
    """作り直さずに、キャッシュにある解析結果だけを返す（無ければ null）。"""
    base, entries, out = vt.load_presets(), vt.load_dict(), []
    for it in body["items"]:
        text = (it.get("text") or "").strip()
        if it.get("speaker") not in base["speakers"] or not text:
            out.append(None)
            continue
        presets, applied = presets_for(it, base)
        meta = CACHE / f"{cache_key(it['speaker'], text, it.get('analyzed_text'), presets, entries, it.get('params'))}.json"
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
    presets = vt.load_presets()
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
            return self.send(200, {"entries": vt.load_dict()})
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
                vt.save_dict(body["entries"])
                return self.send(200, {"entries": vt.load_dict()})
            self.send(404, {"error": "not found"})
        except Exception as e:  # エディタにそのまま見せる
            traceback.print_exc()
            self.send(500, {"error": str(e)})


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=8765)
    ap.add_argument("--project", default=".", help="その回のフォルダ（voice_cache/ と voice_out/ を置く場所）")
    args = ap.parse_args()
    port = args.port
    project = Path(args.project).resolve()
    set_project(project)
    vt._auth()  # 認証情報がなければここで止める
    sys.stdout.reconfigure(errors="replace")
    print(f"プロジェクト: {PROJECT}")
    print(f"辞書: {vt.dict_path()}")
    print(f"アクセントの確認: http://localhost:{port}/review  （止めるときは Ctrl+C）")
    HTTPServer(("127.0.0.1", port), Handler).serve_forever()


if __name__ == "__main__":
    main()
