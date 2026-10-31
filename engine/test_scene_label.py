"""シーン番号の札（旧#97）。番号は Rust だけが決め、Python は札の絵を描くだけ。python engine/test_scene_label.py で確かめる。"""
import os
import sys
import tempfile

FXKIT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "fxkit")
sys.path.insert(0, FXKIT)  # pip で入れた別の版ではなく、この作業フォルダの fxkit を使う
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import akfx_worker as w  # noqa: E402

# scene_label の依頼: subtitle と同じ形（w・h・rect のヘッダと、rect の生の RGBA）
hdr, data = w.scene_label_render("57-2")
assert hdr["rect"] is not None and hdr["w"] > 0 and hdr["h"] > 0, hdr
bx, by, bw, bh = hdr["rect"]
assert len(data) == bw * bh * 4 and bw > 0 and bh > 0
assert 0 <= bx and bx + bw <= hdr["w"] and by + bh <= hdr["h"]
# 番号が違えば絵も違う
assert w.scene_label_render("57-3")[1] != data
# 数字で来ても文字として扱う
assert w.scene_label_render(57)[0] == w.scene_label_render("57")[0]

# load の後のクリップに __scene_labels が無い（番号の札のクリップを足さない）
d = tempfile.mkdtemp(prefix="aks-scene-")
open(os.path.join(d, "script_cues.json"), "w", encoding="utf-8").write(
    '{"cues": [{"n": 1, "start": 1.0, "end": 3.0, "text": "こんにちは", "speaker": "トーク1"}]}')
# 回の fx は `import fxkit`（kit が入っていれば）か、公開の芯だけなら `import fxcore as fxkit`
import importlib.util  # noqa: E402

imp = "import fxkit" if importlib.util.find_spec("fxkit") else "import fxcore as fxkit"
open(os.path.join(d, "t_fx.py"), "w", encoding="utf-8").write(
    "import os\n" + imp + "\n\n\ndef build():\n"
    "    tl = fxkit.Timeline(os.path.join(os.path.dirname(os.path.abspath(__file__)), 'script_cues.json'))\n"
    "    tl.add(fxkit.El(1.0, 2.0))\n"
    "    return tl\n")
info = w.load(os.path.join(d, "t_fx.py"), None, None, True)
keys = [g["key"] for g in info["groups"]]
assert keys and "__scene_labels" not in keys, keys
assert "SceneLabel" not in [g["label"] for g in info["groups"]]
# 本物の依頼の形で: Rust の call は依頼の "id" を依頼の番号で上書きするので、番号は "scene" で渡す
# （"id" で渡していたときは、全部の札が依頼の番号「SCENE 2」になっていた）
import json  # noqa: E402
import struct  # noqa: E402
import subprocess  # noqa: E402

# -I だと PYTHONPATH が効かないので、fxkit の場所は PYTHONPATH で渡す（元の PYTHONPATH も残す。公開側だけの確かめの sitecustomize がある）
p = subprocess.Popen([sys.executable, os.path.join(os.path.dirname(os.path.abspath(__file__)), "akfx_worker.py")],
                     env=dict(os.environ, PYTHONPATH=os.pathsep.join(x for x in (FXKIT, os.environ.get("PYTHONPATH")) if x)), stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
p.stdin.write((json.dumps({"op": "scene_label", "scene": "11-2", "id": 7}) + "\n").encode("utf-8"))
p.stdin.flush()
n = struct.unpack("<I", p.stdout.read(4))[0]
got = json.loads(p.stdout.read(n).decode("utf-8"))
body = p.stdout.read(got["nbytes"])
p.stdin.close()
p.wait(timeout=30)
want, want_body = w.scene_label_render("11-2")
assert got["ok"] and got["id"] == 7, got
assert (got["w"], got["h"]) == (want["w"], want["h"]) and body == want_body, (got, want)
print("ok")
