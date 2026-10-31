import React from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { call, fileUrl } from "../io/api";
import { addMedia } from "../state/media";
import { useS } from "../state";
import { player } from "../io/player";
import { tc } from "../logic/util";
import { AUDIO_EXTS, IMAGE_EXTS, VIDEO_EXTS, isAud, isImg } from "../logic/media";

export default function MediaPanel() {
  const doc = useS((s) => s.project?.doc);
  const root = useS((s) => s.project?.root);
  if (!doc) return null;
  const pick = async (filters?: { name: string; extensions: string[] }[]) => {
    const r = await openDialog({ multiple: true, defaultPath: root, filters });
    if (!r) return;
    await addMedia(Array.isArray(r) ? r : [r]);
  };
  const all = doc.tracks.flatMap((tr) => tr.clips.map((c) => ({ tr, c })));
  return (
    <div className="col" style={{ flex: 1, minHeight: 0, padding: 10, overflow: "auto" }}>
      <div className="row" style={{ flexWrap: "wrap" }}>
        <button className="primary" onClick={() => pick()}>素材を追加…</button>
        <button onClick={() => pick([{ name: "画像", extensions: [...IMAGE_EXTS] }])}>立ち絵（画像）</button>
        <button onClick={() => pick([{ name: "動画", extensions: [...VIDEO_EXTS] }])}>録画</button>
        <button onClick={() => pick([{ name: "音声", extensions: [...AUDIO_EXTS] }])}>BGM</button>
      </div>
      <div className="dim2" style={{ fontSize: 11, lineHeight: 1.7 }}>
        再生位置に追加します。エクスプローラーからタイムラインへドラッグ＆ドロップすることもできます。
        録画（1920×1080）は録画の窓（位置 1240, 460・66.667%）に、立ち絵は左側に配置されます。
      </div>
      <div className="section">ON TIMELINE</div>
      {all.length === 0 && <div className="dim">素材がありません。「素材を追加」から追加してください</div>}
      {all.map(({ tr, c }) => {
        const img = isImg(c.src);
        return (
          <div
            key={c.id}
            className="row"
            style={{ padding: "4px 6px", borderBottom: "1px solid var(--line)", cursor: "pointer" }}
            onClick={() => {
              useS.setState({ selection: { kind: "media", track: tr.id, clip: c.id } });
              player.seek(Math.max(c.start, Math.min(useS.getState().t, c.start + c.dur - 0.1)));
            }}
          >
            {img ? <img src={fileUrl(c.src)} style={{ height: 34, maxWidth: 50, objectFit: "contain" }} /> : <span className="chip">{isAud(c.src) ? "♪" : "▶"}</span>}
            <div className="col grow" style={{ gap: 0, overflow: "hidden" }}>
              <span style={{ whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>{c.name}</span>
              <span className="dim2 ui" style={{ fontSize: 11 }}>{tr.name}・{tc(c.start)}〜{tc(c.start + c.dur)}{c.loop ? "・ループ" : ""}</span>
            </div>
            <button className="ghost" onClick={(e) => { e.stopPropagation(); call("reveal", { path: c.src }); }} title="エクスプローラーで表示">📁</button>
          </div>
        );
      })}
    </div>
  );
}
