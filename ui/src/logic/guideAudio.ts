// 攻略動画の素材の音（通しの素材の音を、映像と同じ並びにした studio/guide_audio.wav）のプレビューの決まり。
// 純粋な関数（再生の予定への追加・行の表示）。wav は本体が裏で作り、できたら GuideState.audio_rev で知らせる。

/** 鳴らすもの 1 つ（io/player.ts の plan が返す形） */
export interface PlanItem {
  voice?: boolean;
  url: string;
  at: number;
  from: number;
  dur: number | null;
  gain: number;
  fi: number;
  fo: number;
  loop: boolean;
}

/** wav の回のフォルダからの場所 */
export const GUIDE_AUDIO_FILE = "studio/guide_audio.wav";

/** 素材の音が無いときに本体が警告に出す文（core/guide_audio.rs）。行の表示の見分けに使う */
export const NO_AUDIO_WARNING = "素材に音がありません";

/** dB → 倍率 */
export const dbGain = (db: number) => Math.pow(10, db / 20);

/**
 * 再生の予定に足す素材の音（0 秒から全部）。まだ作れていない（rev が無い）・消音のときは null。
 * urlOf は wav の URL（rev を付けて、作り直したら読み直す）
 */
export function guideAudioItem(
  rev: number | null | undefined,
  doc: { guide_audio_db?: number; guide_audio_muted?: boolean },
  urlOf: (rev: number) => string,
): PlanItem | null {
  if (rev == null || doc.guide_audio_muted) return null;
  return { url: urlOf(rev), at: 0, from: 0, dur: null, gain: dbGain(doc.guide_audio_db ?? 0), fi: 0, fo: 0, loop: false };
}

/** タイムラインの「素材の音」の行の表示。empty なら帯を出さない */
export function guideAudioRow(
  rev: number | null | undefined,
  warnings: string[],
  muted: boolean,
  why?: string | null,
): { empty: boolean; title: string } {
  if (warnings.includes(NO_AUDIO_WARNING)) return { empty: true, title: "素材に音がありません" };
  // 作れなかったときは、いつまでも「作っています」と出さず、理由を出す
  if (rev == null && why) return { empty: true, title: `素材の音を作れませんでした: ${why.trim()}` };
  if (rev == null) return { empty: true, title: "素材の音を作っています（できると再生で鳴ります）" };
  return { empty: false, title: muted ? "素材の音（消音中）" : "素材の音（通しの素材の音を、映像と同じ並びにしたもの）" };
}
