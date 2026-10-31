// 台本（声の並び）と演出（files/script.srt の cue）の時刻のずれの表示。issue 旧#4
import { useMemo, useState } from "react";
import { exportTalk, useS } from "../state";
import { cueDrift, type CueDrift } from "../logic/voice";

/** ずれ（cue が無い・演出が無いときは null） */
export function useCueDrift(): CueDrift | null {
  const layout = useS((s) => s.layout);
  const cues = useS((s) => s.desc?.cues);
  return useMemo(() => (cues?.length && layout.length ? cueDrift(layout, cues) : null), [layout, cues]);
}

export function driftText(d: CueDrift): string {
  const parts: string[] = [];
  if (d.offs.length) parts.push(`台本と演出の時刻が最大 ${d.max.toFixed(2)} 秒ずれています（${d.offs.length} 個の cue）`);
  if (d.nLines !== d.nCues) parts.push(`セリフ ${d.nLines} 行 / cue ${d.nCues} 個`);
  return parts.join("・");
}

const HELP =
  "演出は files/script.srt の cue の時刻で、声は台本の開始時刻で再生されます。\n" +
  "台本の開始を変えたら「台本 → 演出の入力を書き出し」で script.srt と files/talk/ を作り直してください（演出は自動で再読み込みされます）。";

/** トップバーの警告と、書き出しのボタン */
export function DriftWarning() {
  const d = useCueDrift();
  const [busy, setBusy] = useState(false);
  if (!d || d.ok) return null;
  return (
    <span className="pill warn" title={HELP}>
      ⚠ {driftText(d)}
      <button
        className="ghost"
        style={{ padding: "0 6px" }}
        disabled={busy}
        onClick={async () => {
          setBusy(true);
          await exportTalk();
          setBusy(false);
        }}
      >
        {busy ? "書き出し中…" : "台本 → 演出の入力を書き出し"}
      </button>
    </span>
  );
}

export { HELP as DRIFT_HELP };
