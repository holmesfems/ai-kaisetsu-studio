"""THIRD_PARTY.md（依存のライセンスの一覧）を作り直す。

    python tools/third_party.py            # THIRD_PARTY.md を書く
    python tools/third_party.py --check    # 書かずに、今の依存と合っているかだけ見る（ずれていれば終了コード 1）

- Rust: `cargo metadata`（Windows 向け・本番と build の依存だけ。開発用は除く）の name・version・license
- npm: `package-lock.json` の本番の依存（dev でないもの）の license
- Python・フォント・外部のソフト: このファイルの表（依存を足したら表も直す）
"""
from __future__ import annotations

import json
import subprocess
import sys
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "THIRD_PARTY.md"

# fxkit/pyproject.toml の dependencies と optional-dependencies（名前・ライセンス・用途）
PYTHON = [
    ("pillow", "HPND（MIT-CMU）", "演出・字幕の絵を描く"),
    ("numpy", "BSD-3-Clause", "画素の計算"),
    ("scipy", "BSD-3-Clause", "画素の計算（ぼかしなど）"),
    ("fonttools", "MIT", "フォントの情報を読む"),
    ("imageio-ffmpeg", "BSD-2-Clause", "ffmpeg の実行ファイルを探す（実行ファイルは下の ffmpeg）"),
    ("faster-whisper", "MIT", "同期点の推定（任意: `fxkit[sync]`）"),
    ("opencv-python", "Apache-2.0（OpenCV 本体。同梱の部品に別のライセンスあり）", "録画の追跡（任意: `fxkit[track]`）"),
]

FONTS = [
    ("Anton", "SIL Open Font License 1.1", "fxkit/fonts/Anton-OFL.txt"),
    ("Saira", "SIL Open Font License 1.1", "fxkit/fonts/Saira-OFL.txt"),
    ("DejaVu Sans Mono", "Bitstream Vera / DejaVu のライセンス（自由に使える）", "fxkit/fonts/DejaVu-LICENSE.txt"),
]

EXTERNAL = [
    ("VoiSona Talk", "別に入れる（同梱しない）", "声の合成。利用者が自分で入れ、環境設定で API をオンにする。利用条件は VoiSona の規約に従う"),
    ("ffmpeg", "別に入れる（同梱しない）", "動画の読み書き。imageio-ffmpeg が持つ実行ファイル、または PATH の ffmpeg を使う。ffmpeg のライセンス（LGPL/GPL。ビルドによる）に従う"),
    ("Microsoft Edge WebView2 Runtime", "別に入れる（同梱しない）", "画面の表示。Windows 11 には入っている。Microsoft の規約に従う"),
]


def rust() -> list[tuple[str, str, str]]:
    meta = json.loads(subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--filter-platform", "x86_64-pc-windows-msvc"],
        cwd=ROOT, check=True, capture_output=True, text=True, encoding="utf-8").stdout)
    pk = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    members = set(meta["workspace_members"])
    seen: set[str] = set()
    stack = list(members)
    while stack:
        i = stack.pop()
        if i in seen:
            continue
        seen.add(i)
        for d in nodes[i]["deps"]:
            # 開発用（kind が dev だけ）の依存は除く
            if any(k["kind"] in (None, "build") for k in d["dep_kinds"]):
                stack.append(d["pkg"])
    rows = []
    for i in seen - members:
        p = pk[i]
        lic = p.get("license") or (f"（license-file: {p['license_file']}）" if p.get("license_file") else "不明")
        rows.append((p["name"], p["version"], lic.replace("|", "/")))
    return sorted(rows)


def npm() -> list[tuple[str, str, str]]:
    lock = json.loads((ROOT / "package-lock.json").read_text(encoding="utf-8"))
    rows = []
    for path, v in lock["packages"].items():
        if not path or v.get("dev"):
            continue
        name = path.split("node_modules/")[-1]
        rows.append((name, v.get("version", ""), str(v.get("license", "不明"))))
    return sorted(rows)


def table(rows: list[tuple[str, ...]], head: tuple[str, ...]) -> str:
    out = ["| " + " | ".join(head) + " |", "|" + "---|" * len(head)]
    out += ["| " + " | ".join(r) + " |" for r in rows]
    return "\n".join(out)


def summary(rows: list[tuple[str, str, str]]) -> str:
    c = Counter(r[2] for r in rows)
    return "\n".join(f"- {lic}: {n}" for lic, n in sorted(c.items(), key=lambda x: (-x[1], x[0])))


def needs_care(rows: list[tuple[str, str, str]]) -> list[str]:
    """GPL・LGPL を選ぶしかないもの（OR で別のライセンスを選べるものは除く）と、ライセンス不明のもの"""
    out = []
    for name, _, lic in rows:
        only = " OR " not in lic and "/" not in lic
        if lic == "不明" or lic.startswith("（license-file") or ("GPL" in lic and only):
            out.append(f"{name}（{lic}）")
    return out


def build() -> str:
    r, n = rust(), npm()
    care = needs_care(r + n)
    care_text = ("Rust・npm の依存に、GPL・LGPL しか選べないもの・ライセンス不明のものは無い。" if not care
                 else "次の依存は、ライセンスを確かめること: " + "、".join(care) + "。")
    s = ["# 依存のライセンス", "",
         "AK Studio 本体は [MIT](LICENSE)。使っている部品のライセンスを以下に挙げる。",
         "この一覧は `python tools/third_party.py` で作り直す（Rust は `cargo metadata`、npm は `package-lock.json` から。",
         "開発用の依存（テスト・ビルドの道具）は含めない）。", "",
         f"## Rust（{len(r)} 件）", "", "ライセンスの内訳:", "", summary(r), "",
         "「/」や OR でつないだものは、そのうちのどれかを選べる（二重ライセンス）。", "",
         "MPL-2.0 の部品（cssparser・selectors など）は、そのファイルを直して配るときだけ、そのファイルのソースを公開する決まり。",
         "AK Studio は直さずに使っている。" + care_text, "",
         table([(a, b, c) for a, b, c in r], ("名前", "版", "ライセンス")), "",
         f"## npm（本番の依存 {len(n)} 件）", "", "ライセンスの内訳:", "", summary(n), "",
         table(n, ("名前", "版", "ライセンス")), "",
         "## Python（fxkit の依存。利用者が `pip` で入れる。同梱しない）", "",
         table(PYTHON, ("名前", "ライセンス", "用途")), "",
         "## フォント（`fxkit/fonts/` に同梱）", "",
         table(FONTS, ("名前", "ライセンス", "ライセンス文")), "",
         "字幕の既定のフォントなど、OS に入っているフォントは同梱しない。", "",
         "## 外部のソフト", "", table(EXTERNAL, ("名前", "同梱", "メモ")), ""]
    return "\n".join(s)


def main() -> int:
    text = build()
    if "--check" in sys.argv:
        if OUT.exists() and OUT.read_text(encoding="utf-8") == text:
            print("THIRD_PARTY.md は今の依存と合っています")
            return 0
        print("THIRD_PARTY.md が今の依存とずれています（python tools/third_party.py で作り直す）")
        return 1
    OUT.write_text(text, encoding="utf-8", newline="\n")
    print(f"書きました: {OUT}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
