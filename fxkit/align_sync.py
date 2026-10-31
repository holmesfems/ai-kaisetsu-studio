# -*- coding: utf-8 -*-
"""
発話同期点（セリフの中でキーワードを言い始める時刻）を、実際の音声から求める。

    PYTHONUTF8=1 PYTHONIOENCODING=utf-8 python align_sync.py <files フォルダ> --cues 13 83
        [--retranscribe] [--cpu] [--all]

  --cues A B       : 演出の対象範囲（cue A〜B）
  --retranscribe   : whisper_cache.json を使わず認識し直す
  --cpu            : CUDA が使えないとき（medium / int8。遅く、精度も落ちる）
  --all            : キーワードの無い cue も認識する（普段は不要）

キーワードの出どころ（両方を合わせて使う）:
  1. spec.md の演出表で「〜」で（同期）と書かれたもの（自動で拾う）
  2. <files>/sync_keywords.json … 間合いを合わせるために自分で足すもの。例: {"75": ["援軍の攻撃範囲", "かなり広範囲"]}
     キーワードはそのcueのセリフに文字どおり含まれている必要がある。

入力: <files>/script_cues.json（AK Studio が書き出す cue の表。話者・文・開始・終わり）、<files>/talk/<話者フォルダ>/*.wav と同名の .txt
      話者フォルダは cue の表の話者名と NFKC・空白無視で一致するものを使う。
出力: <files>/sync_points.json（fxcore.Timeline が読む）、sync_points.md（ユーザー確認用）、whisper_cache.json
前提: 各 wav は対応する cue の開始時刻から始まる（動画全体の時刻 = cue 開始 + wav 内の時刻）。
"""
import os
import re
import sys
import json
import glob
import difflib
import argparse
import unicodedata

import numpy as np
import scipy.io.wavfile as wavfile
from scipy.signal import resample_poly

import fxcore

HERE = None   # main() で <files> フォルダに決まる
CUES_JSON = TALK = CACHE = OUT_JSON = OUT_MD = None
CUE_RANGE = range(0)
KEYWORDS = {}


def setup_paths(files, cue_a, cue_b):
    global HERE, CUES_JSON, TALK, CACHE, OUT_JSON, OUT_MD, CUE_RANGE
    HERE = os.path.abspath(files)
    CUES_JSON = os.path.join(HERE, "script_cues.json")
    TALK = os.path.join(HERE, "talk")
    CACHE = os.path.join(HERE, "whisper_cache.json")
    OUT_JSON = os.path.join(HERE, "sync_points.json")
    OUT_MD = os.path.join(HERE, "sync_points.md")
    CUE_RANGE = range(cue_a, cue_b + 1)


def load_keywords(files, cues):
    """spec.md の「〜」で（同期）と sync_keywords.json を合わせる。セリフに無いものは警告して外す。"""
    kw = {}
    spec = os.path.join(files, "spec.md")
    if os.path.exists(spec):
        for line in open(spec, encoding="utf-8-sig"):
            m = re.match(r"^\|\s*(\d+)\s*\|", line)
            if not m:
                continue
            for k in re.findall(r"「([^」]+)」で[（(]同期[）)]", line):
                kw.setdefault(int(m.group(1)), [])
                if k not in kw[int(m.group(1))]:
                    kw[int(m.group(1))].append(k)
    extra = os.path.join(files, "sync_keywords.json")
    if os.path.exists(extra):
        for n, ks in json.load(open(extra, encoding="utf-8")).items():
            for k in ks:
                kw.setdefault(int(n), [])
                if k not in kw[int(n)]:
                    kw[int(n)].append(k)
    out = {}
    for n in sorted(kw):
        if n not in cues:
            print(f"warn: cue {n} は cue の表に無い")
            continue
        ok = [k for k in kw[n] if k in cues[n]["text"]]
        for k in kw[n]:
            if k not in ok:
                print(f"warn: cue {n} のセリフに「{k}」が無いので外した")
        if ok:
            out[n] = ok
    return out


def nkey(s):
    return re.sub(r"\s+", "", unicodedata.normalize("NFKC", s or ""))


FLAG_DIFF = 0.6  # seconds


# ---------------------------------------------------------------- cue の表 / mapping
def norm_ws(s):
    return re.sub(r"\s+", " ", unicodedata.normalize("NFKC", s)).strip()


def map_wavs(cues):
    """Return {cue_no: wav_path}, list of problems."""
    problems = []
    mapping = {}
    used = {}
    folders = {nkey(d): d for d in os.listdir(TALK) if os.path.isdir(os.path.join(TALK, d))}
    tags = sorted({c["speaker"] for c in cues.values() if c["speaker"]})
    pairs = [(tag, folders.get(nkey(tag))) for tag in tags]
    for tag, folder in pairs:
        if folder is None:
            problems.append(f"話者 [{tag}] に対応する talk/ のフォルダが無い（{list(folders.values())}）")
    if not tags or all(f is None for _, f in pairs):
        # 話者で分けられないときは全フォルダを全cueと照合する
        pairs = [(None, d) for d in folders.values()]
    for tag, folder in pairs:
        if folder is None:
            continue
        cand = {n: norm_ws(c["text"]) for n, c in cues.items() if tag is None or c["speaker"] == tag}
        for txt in sorted(glob.glob(os.path.join(TALK, folder, "*.txt"))):
            wav = txt[:-4] + ".wav"
            if not os.path.exists(wav):
                problems.append(f"no wav for {txt}")
                continue
            with open(txt, encoding="utf-8-sig") as f:
                t = norm_ws(f.read())
            exact = [n for n, ct in cand.items() if ct == t]
            if len(exact) == 1:
                n = exact[0]
            elif len(exact) > 1:
                # 同じセリフが2回出る（「次にスキルについてです。」など）: 連番の若い順に、まだ使っていない cue へ
                free = [m for m in exact if m not in used]
                n = free[0] if free else exact[0]
                if not free:
                    problems.append(f"ambiguous exact match {os.path.basename(txt)} -> cues {exact}")
            else:
                scored = sorted(((difflib.SequenceMatcher(None, t, ct).ratio(), n) for n, ct in cand.items()), reverse=True)
                (r1, n), r2 = scored[0], (scored[1][0] if len(scored) > 1 else 0)
                if r1 < 0.8 or r1 - r2 < 0.1:
                    problems.append(f"weak/ambiguous fuzzy match {os.path.basename(txt)} -> cue {n} (ratio {r1:.2f}, 2nd {r2:.2f})")
                else:
                    problems.append(f"fuzzy match {os.path.basename(txt)} -> cue {n} (ratio {r1:.2f})")
            if n in used:
                problems.append(f"cue {n} matched by two wavs: {os.path.basename(used[n])}, {os.path.basename(wav)}")
            used[n] = wav
            mapping[n] = {"wav": wav, "txt_text": t}
    for n in CUE_RANGE:
        if n in cues and n not in mapping:
            problems.append(f"cue {n} has no wav")
    return mapping, problems


# ---------------------------------------------------------------- audio
def load_wav(path):
    sr, d = wavfile.read(path)
    if d.ndim > 1:
        d = d.mean(axis=1)
    if d.dtype == np.int16:
        a = d.astype(np.float32) / 32768.0
    elif d.dtype == np.int32:
        a = d.astype(np.float32) / 2147483648.0
    else:
        a = d.astype(np.float32)
    return sr, a


def onset_offset(sr, a, rel_db=-40.0, floor_db=-55.0):
    """Simple RMS-energy speech onset/offset (10 ms frame, 5 ms hop)."""
    win, hop = int(sr * 0.010), int(sr * 0.005)
    if len(a) < win:
        return 0.0, len(a) / sr
    n = 1 + (len(a) - win) // hop
    idx = np.arange(win)[None, :] + hop * np.arange(n)[:, None]
    rms = np.sqrt(np.mean(a[idx] ** 2, axis=1) + 1e-12)
    db = 20 * np.log10(rms)
    thr = max(db.max() + rel_db, floor_db)
    on = np.where(db > thr)[0]
    if len(on) == 0:
        return 0.0, len(a) / sr
    return on[0] * hop / sr, (on[-1] * hop + win) / sr


def frame_db(sr, a):
    win, hop = int(sr * 0.010), int(sr * 0.005)
    n = 1 + (len(a) - win) // hop
    idx = np.arange(win)[None, :] + hop * np.arange(n)[:, None]
    db = 20 * np.log10(np.sqrt(np.mean(a[idx] ** 2, axis=1) + 1e-12))
    return db, win, hop


def silence_gaps(sr, a, min_gap, rel_db=-40.0, floor_db=-55.0):
    """Interior silences (start, end) in seconds (frame centres), at least min_gap long."""
    db, win, hop = frame_db(sr, a)
    sil = db < max(db.max() + rel_db, floor_db)
    out, k, n = [], 0, len(db)
    while k < n:
        if sil[k]:
            j = k
            while j < n and sil[j]:
                j += 1
            if k > 0 and j < n and (j - k) * hop / sr >= min_gap:
                out.append(((k * hop + win / 2) / sr, (j * hop + win / 2) / sr))
            k = j
        else:
            k += 1
    return out


def to16k(sr, a):
    if sr == 16000:
        return a.astype(np.float32)
    from math import gcd
    g = gcd(sr, 16000)
    return resample_poly(a, 16000 // g, sr // g).astype(np.float32)


# ---------------------------------------------------------------- whisper
def setup_cuda_dlls():
    try:
        import nvidia
    except ImportError:
        return
    for p in nvidia.__path__:
        for sub in ("cublas", "cudnn", "cuda_nvrtc"):
            b = os.path.join(p, sub, "bin")
            if os.path.isdir(b):
                os.add_dll_directory(b)
                os.environ["PATH"] = b + os.pathsep + os.environ["PATH"]


def load_model(force_cpu=False):
    from faster_whisper import WhisperModel
    if not force_cpu:
        try:
            setup_cuda_dlls()
            m = WhisperModel("large-v3", device="cuda", compute_type="float16")
            # smoke test so that missing DLLs fail here, not mid-run
            list(m.transcribe(np.zeros(16000, np.float32), language="ja")[0])
            return m, "cuda/float16/large-v3"
        except Exception as e:  # noqa
            print("CUDA failed, falling back to CPU:", e)
    return WhisperModel("medium", device="cpu", compute_type="int8"), "cpu/int8/medium"


def transcribe(model, wav, prompt):
    sr, a = load_wav(wav)
    segs, _ = model.transcribe(to16k(sr, a), language="ja", word_timestamps=True, beam_size=5,
                               initial_prompt=prompt, vad_filter=False,
                               condition_on_previous_text=False)
    words = []
    for s in segs:
        for w in (s.words or []):
            words.append({"w": w.word, "s": round(float(w.start), 3), "e": round(float(w.end), 3),
                          "p": round(float(w.probability), 3)})
    return words


# ---------------------------------------------------------------- alignment
def is_kept(ch):
    """Characters that are 'spoken' (kept after normalization)."""
    if ch.isspace():
        return False
    cat = unicodedata.category(ch)
    if cat[0] in ("P", "Z", "C"):
        return ch == "-"  # keep minus sign (handled in canon())
    if ch in "～〜・":
        return False
    return True


def canon_chars(text):
    """NFKC + drop punctuation/spaces. Returns list of (char, orig_index).
    A '-' immediately followed by a digit is a minus sign -> expanded to 'マイナス';
    any other '-' is dropped. '%' -> 'パーセント'. All expansions map back to the
    original char index so that keyword boundaries remain exact."""
    out = []
    for i, ch in enumerate(text):
        for c in unicodedata.normalize("NFKC", ch):
            if c == "-":
                nxt = unicodedata.normalize("NFKC", text[i + 1]) if i + 1 < len(text) else ""
                if nxt[:1].isdigit():
                    out.extend(("マイナス"[k], i) for k in range(4))
                continue
            if c == "%":
                out.extend((x, i) for x in "パーセント")
                continue
            if c == "ー" or c.isalnum() or is_kept(c):
                out.append((c.lower(), i))
    return out


def recog_chars(words):
    """Per-character timestamps for recognized text: each word's kept chars are
    spread evenly over the word's [start, end]."""
    # canonicalize the joined text (so that e.g. a "-" token followed by a "3" token
    # still becomes "マイナス3"), then give each kept char the timing of its word
    full, owner = "", []
    for wi, w in enumerate(words):
        full += w["w"]
        owner += [wi] * len(w["w"])
    per_word = {}
    for c, oi in canon_chars(full):
        per_word.setdefault(owner[oi], []).append(c)
    chars = []
    for wi in sorted(per_word):
        w, cs = words[wi], per_word[wi]
        d = (w["e"] - w["s"]) / len(cs)
        for k, c in enumerate(cs):
            chars.append({"c": c, "s": w["s"] + k * d, "e": w["s"] + (k + 1) * d})
    return chars


def align(known, words):
    """Map every kept char of the known text to (start, end, how).
    how: 'match' (exact char match), 'interp' (inside a replace/delete block)."""
    kc = canon_chars(known)
    rc = recog_chars(words)
    a = "".join(c for c, _ in kc)
    b = "".join(c["c"] for c in rc)
    sm = difflib.SequenceMatcher(None, a, b, autojunk=False)
    res = [None] * len(kc)
    for op, i1, i2, j1, j2 in sm.get_opcodes():
        if op == "equal":
            for k in range(i2 - i1):
                r = rc[j1 + k]
                res[i1 + k] = (r["s"], r["e"], "match")
        elif op in ("replace", "delete"):
            if op == "replace":
                ts, te = rc[j1]["s"], rc[j2 - 1]["e"]
            else:  # known chars not recognized: squeeze between neighbours
                ts = rc[j1 - 1]["e"] if j1 > 0 else (rc[0]["s"] if rc else 0.0)
                te = rc[j1]["s"] if j1 < len(rc) else ts
            n = i2 - i1
            for k in range(n):
                res[i1 + k] = (ts + (te - ts) * k / n, ts + (te - ts) * (k + 1) / n, "interp")
        # 'insert' (extra recognized chars) -> nothing to assign
    return kc, res, a, b, sm


def keyword_time(known, kw, kc, res, rc_words):
    pos = known.find(kw)
    if pos < 0:
        raise ValueError(f"keyword {kw!r} not in text")
    idxs = [k for k, (_, oi) in enumerate(kc) if pos <= oi < pos + len(kw)]
    if not idxs or res[idxs[0]] is None:
        return None
    first, last = res[idxs[0]], res[idxs[-1]]
    method = "whisper" if first[2] == "match" and last[2] == "match" else "interp"
    return pos, first[0], last[1], method, idxs


# Whisper (large-v3) word timestamps on these TTS lines come out EARLY: measured on an387 cues 13-83
# against the waveform, mid-phrase ~0.10 s early (geminate ッ closures), word ends before a
# pause ~0.14 s early, and the word after a pause ~0.23 s early (Whisper swallows the pause into
# the next word).  So we re-time each char with
#   t = whisper + WHISPER_LAG
# and then snap to what the waveform says exactly:
#   * every 、。 in the script is matched to its RMS pause -> the char before ends at the pause
#     start, the char after starts at the pause end;
#   * any char start that lands inside a pause (>= PAUSE_MIN_GAP) is moved to the pause end;
#   * the first char starts at the RMS onset, the last char ends at the RMS offset.
# Validation (an387 cues 13-83, leave-the-anchor-out): geminate MAE 0.04 s, pause-start MAE 0.04 s
# vs 0.10 / 0.14 s for raw Whisper.
PUNCT = "、。，！？!?"
PAUSE_MIN_GAP = 0.10   # s; TTS comma pauses are >=0.13 s, geminate (ッ) closures are shorter
WHISPER_LAG = 0.12     # s
PAUSE_TOL = 0.35       # s


def match_pauses(known, kc, res, gaps):
    """[(kc index before punct, kc index after punct, (gap_start, gap_end)), ...]"""
    out, used = [], set()
    for p, ch in enumerate(known):
        if ch not in PUNCT:
            continue
        before = [k for k, (_, oi) in enumerate(kc) if oi < p]
        after = [k for k, (_, oi) in enumerate(kc) if oi > p]
        if not before or not after:
            continue
        kb, ka = before[-1], after[0]
        tb = (res[kb][1] + res[ka][0]) / 2 + WHISPER_LAG
        cand = [(abs((g[0] + g[1]) / 2 - tb), gi) for gi, g in enumerate(gaps) if gi not in used]
        if not cand:
            continue
        dist, gi = min(cand)
        if dist <= PAUSE_TOL:
            used.add(gi)
            out.append((kb, ka, gaps[gi]))
    return out


def retime(kc, res, known, gaps, onset, offset):
    """Corrected (start, end) for every script char."""
    st = [r[0] + WHISPER_LAG for r in res]
    en = [r[1] + WHISPER_LAG for r in res]
    for g0, g1 in gaps:
        for k in range(len(st)):
            if g0 - 0.02 <= st[k] < g1:
                st[k] = g1
            if g0 < en[k] <= g1 + 0.02:
                en[k] = g0
    pauses = match_pauses(known, kc, res, gaps)
    for kb, ka, (g0, g1) in pauses:
        en[kb] = g0
        st[ka] = g1
    st[0] = onset
    en[-1] = offset
    for k in range(len(st)):
        en[k] = max(en[k], st[k])
    return list(zip(st, en)), pauses


def coverage(known, words):
    """Fraction of the script's spoken chars that are exactly matched by the recognition."""
    if not words:
        return 0.0
    kc, res, *_ = align(known, words)
    return sum(1 for r in res if r and r[2] == "match") / max(1, len(kc))


def snippet(words, t0, t1, pad=0.6):
    return "".join(w["w"] for w in words if w["e"] >= t0 - pad and w["s"] <= t1 + pad).strip()


# ---------------------------------------------------------------- output helpers
def mmss(t):
    m = int(t // 60)
    return f"{m:02d}:{t - 60 * m:06.3f}"


def r3(x):
    return None if x is None else round(float(x), 3)


def main():
    global KEYWORDS
    ap = argparse.ArgumentParser()
    ap.add_argument("files")
    ap.add_argument("--cues", nargs=2, type=int, required=True)
    ap.add_argument("--retranscribe", action="store_true")
    ap.add_argument("--cpu", action="store_true")
    ap.add_argument("--all", action="store_true")
    o = ap.parse_args()
    args = [f"--{k}" for k in ("retranscribe", "cpu", "all") if getattr(o, k)]
    setup_paths(o.files, *o.cues)
    cues = {n: dict(c, speaker=c["speaker"] or None) for n, c in fxcore.load_cues(CUES_JSON).items()}
    KEYWORDS = load_keywords(HERE, cues)
    print("keywords:", sum(len(v) for v in KEYWORDS.values()), "個 /", len(KEYWORDS), "cue")
    mapping, problems = map_wavs(cues)
    for p in problems:
        if not p.startswith("fuzzy match"):
            print("warn:", p)

    # ---- step 1: cue table
    cue_info = {}
    for n in CUE_RANGE:
        if n not in cues:
            continue
        c = cues[n]
        info = {"start": r3(c["start"]), "end": r3(c["end"]), "speaker": c["speaker"]}
        if n in mapping:
            wav = mapping[n]["wav"]
            sr, a = load_wav(wav)
            on, off = onset_offset(sr, a)
            info.update({"wav": os.path.relpath(wav, HERE).replace("\\", "/"),
                         "wav_dur": r3(len(a) / sr), "onset": r3(on), "offset": r3(off),
                         "srt_dur": r3(c["end"] - c["start"]),
                         "dur_diff": r3(len(a) / sr - (c["end"] - c["start"])),
                         "txt_equals_srt": mapping[n]["txt_text"] == norm_ws(c["text"])})
        cue_info[n] = info

    # ---- step 2: whisper
    cache = {}
    if os.path.exists(CACHE) and "--retranscribe" not in args:
        with open(CACHE, encoding="utf-8") as f:
            cache = json.load(f)
    need = [n for n in CUE_RANGE if n in mapping and (n in KEYWORDS or "--all" in args)]
    todo = [n for n in need if str(n) not in cache or cache[str(n)].get("text") != mapping[n]["txt_text"]
            or "variants" not in cache[str(n)]]
    model_desc = cache.get("_model", "")
    if todo:
        model, model_desc = load_model("--cpu" in args)
        print("model:", model_desc)
        for n in todo:
            known = mapping[n]["txt_text"]
            # The full-line initial_prompt sometimes makes Whisper skip already-"seen" text or
            # hallucinate an outro ("ご視聴ありがとうございました"), so we also try a keyword-only
            # prompt and no prompt, and keep the variant whose text matches the script best.
            prompts = [("full", known), ("keywords", "、".join(KEYWORDS.get(n, [])) or None), ("none", None)]
            variants = {}
            for label, prompt in prompts:
                words = transcribe(model, mapping[n]["wav"], prompt)
                variants[label] = {"words": words, "coverage": round(coverage(known, words), 3)}
                print(f"cue {n} [{label} {variants[label]['coverage']:.2f}]: {''.join(w['w'] for w in words)}")
            cache[str(n)] = {"text": known, "variants": variants}
        cache["_model"] = model_desc
        with open(CACHE, "w", encoding="utf-8") as f:
            json.dump(cache, f, ensure_ascii=False, indent=1)

    # ---- step 2/3: keywords
    sync = []
    for n, kws in KEYWORDS.items():
        if n not in cue_info:
            continue
        c, ci = cues[n], cue_info[n]
        known = mapping[n]["txt_text"] if n in mapping else norm_ws(c["text"])
        variants = cache.get(str(n), {}).get("variants", {})
        best = max(variants, key=lambda k: (variants[k]["coverage"], -list(variants).index(k))) if variants else None
        words = variants[best]["words"] if best else []
        if best:
            cue_info[n]["whisper_variant"] = best
            cue_info[n]["whisper_coverage"] = variants[best]["coverage"]
            cue_info[n]["recognized_full"] = "".join(w["w"] for w in words)
        kc, res, a_str, b_str, sm = align(known, words) if words else (canon_chars(known), None, "", "", None)
        on, off = ci.get("onset", 0.0), ci.get("offset", c["end"] - c["start"])
        fixed, pauses = None, []
        if res and n in mapping:
            sr, aud = load_wav(mapping[n]["wav"])
            fixed, pauses = retime(kc, res, known, silence_gaps(sr, aud, PAUSE_MIN_GAP), on, off)
            cue_info[n]["pauses_used"] = [[r3(g0), r3(g1)] for _, _, (g0, g1) in pauses]
        for kw in kws:
            pos = known.find(kw)
            naive_in = on + pos / len(known) * (off - on)
            naive_end_in = on + (pos + len(kw)) / len(known) * (off - on)
            r = keyword_time(known, kw, kc, res, words) if res else None
            if r is None:
                t_in, t_end_in, method, rec = naive_in, naive_end_in, "ratio", ""
                raw_in = raw_end = None
            else:
                _, raw_in, raw_end, method, idxs = r
                rec = snippet(words, raw_in, raw_end)
                t_in, t_end_in = fixed[idxs[0]][0], fixed[idxs[-1]][1]
            sync.append({"cue": n, "keyword": kw, "t": r3(c["start"] + t_in), "t_end": r3(c["start"] + t_end_in),
                         "in_wav": r3(t_in), "in_wav_end": r3(t_end_in), "method": method,
                         "whisper_raw_in_wav": r3(raw_in), "whisper_raw_in_wav_end": r3(raw_end),
                         "naive_t": r3(c["start"] + naive_in), "naive_in_wav": r3(naive_in),
                         "diff_vs_naive": r3(t_in - naive_in), "recognized": rec})

    out = {"model": model_desc, "mapping_problems": problems,
           "cues": {str(n): cue_info[n] for n in CUE_RANGE if n in cue_info}, "sync": sync}
    with open(OUT_JSON, "w", encoding="utf-8") as f:
        json.dump(out, f, ensure_ascii=False, indent=1)

    # ---- markdown
    L = [f"# 発話同期ポイント（cue {CUE_RANGE.start}〜{CUE_RANGE.stop - 1}）", "",
         f"- 音声認識: faster-whisper `{model_desc}`（language=ja, word_timestamps, beam 5, vad_filter=False）。"
         "initial_prompt は「台本全文／キーワードのみ／なし」の3通りで認識し、台本との一致率が最も高いものを採用"
         "（全文プロンプトは「ご視聴ありがとうございました」などの幻聴や冒頭の読み飛ばしが多かったため）",
         "- 時刻 = SRTのcue開始 + セリフ内の時刻（wav は cue 開始から始まる前提。下の表の「差」が大きい cue は要確認）",
         f"- セリフ内の時刻は Whisper の値に +{WHISPER_LAG:.2f}秒 の遅れ補正をかけ、読点・句点の無音（RMS）に"
         "スナップしたもの（Whisperはこの音声で0.1〜0.2秒早く出る。補正後の誤差は実測で平均約0.04秒）。補正前の値は「Whisper生」列",
         "- 方法: whisper = キーワードの最初と最後の文字が認識結果と一致 / interp = 認識結果の文字が違うため前後から補間",
         "- 素朴推定 = 発話開始 + 文字位置/全文字数 × 発話長（RMSで求めた発話区間）",
         f"- ⚠ = 方法がwhisper以外、または素朴推定と{FLAG_DIFF}秒より大きくずれている行",
         "", "| cue | キーワード | 時刻 (mm:ss.mmm) | 終了 | セリフ内 (秒) | Whisper生 (秒) | 方法 | 素朴推定との差 (秒) | 認識テキスト | 注意 |",
         "|---:|---|---|---|---:|---:|---|---:|---|---|"]
    for s in sync:
        flag = "⚠" if (s["method"] != "whisper" or abs(s["diff_vs_naive"]) > FLAG_DIFF) else ""
        raw = "–" if s["whisper_raw_in_wav"] is None else f"{s['whisper_raw_in_wav']:.3f}"
        L.append(f"| {s['cue']} | {s['keyword']} | {mmss(s['t'])} | {mmss(s['t_end'])} | {s['in_wav']:.3f} | {raw} | "
                 f"{s['method']} | {s['diff_vs_naive']:+.3f} | {s['recognized']} | {flag} |")
    L += ["", f"## wavとSRTの対応（cue {CUE_RANGE.start}〜{CUE_RANGE.stop - 1}）", "",
          "| cue | 話者 | SRT開始 | SRT長 | wav長 | 差 | 発話開始 | 発話終了 |", "|---:|---|---|---:|---:|---:|---:|---:|"]
    for n in CUE_RANGE:
        if n not in cue_info:
            continue
        ci = cue_info[n]
        if "wav" in ci:
            L.append(f"| {n} | {ci['speaker']} | {mmss(ci['start'])} | {ci['srt_dur']:.3f} | {ci['wav_dur']:.3f} | "
                     f"{ci['dur_diff']:+.3f} | {ci['onset']:.3f} | {ci['offset']:.3f} |")
        else:
            L.append(f"| {n} | {ci['speaker']} | {mmss(ci['start'])} | – | wavなし | | | |")
    if problems:
        L += ["", "## 対応付けの注意", ""] + [f"- {p}" for p in problems]
    with open(OUT_MD, "w", encoding="utf-8") as f:
        f.write("\n".join(L) + "\n")
    print("wrote", OUT_JSON, OUT_MD)


if __name__ == "__main__":
    main()
