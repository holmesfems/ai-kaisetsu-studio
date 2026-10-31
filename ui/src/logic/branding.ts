// 画面の上の帯の文言。公開の既定に、非公開の上書き（presets/ui/branding.json）を合わせる

export type Branding = {
  /** 帯の名前（大きく出る） */
  title: string;
  /** 名前の横に小さく出す一言。空にすると出さない */
  sub: string;
};

/** 公開の既定。AK は AI Kaisetsu の頭文字 */
export const DEFAULT_BRANDING: Branding = { title: "AK Studio", sub: "AI Kaisetsu Studio" };

/** 上書きの JSON（形は不明）を既定に重ねる。文字列でない・空の title は無視して既定のまま（sub は空文字で消せる） */
export function mergeBranding(override: unknown, base: Branding = DEFAULT_BRANDING): Branding {
  const o = override && typeof override === "object" ? (override as Record<string, unknown>) : {};
  const title = typeof o.title === "string" && o.title.trim() ? o.title.trim() : base.title;
  const sub = typeof o.sub === "string" ? o.sub.trim() : base.sub;
  return { title, sub };
}
