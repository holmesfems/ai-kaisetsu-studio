// 失敗の知らせの共通の部品（純粋な関数）。

/** 同じ鍵の知らせを 1 回だけにする（再生・描画のたびに同じ失敗をトーストしないため）。forget で次はまた知らせる */
export function onceKeys() {
  const seen = new Set<string>();
  return {
    /** 初めての鍵なら true を返して覚える */
    first(k: string): boolean {
      if (seen.has(k)) return false;
      seen.add(k);
      return true;
    },
    forget(k: string): void {
      seen.delete(k);
    },
  };
}

/** akf の URL（fileUrl の `/file?p=<encoded>&v=2` か `/audio/abc.wav`）か、そのままのパスから、知らせに出すファイル名を取り出す */
export function urlFileName(url: string): string {
  let p = url;
  // そのままのパス（E:/rec/#1.mp4 など）は ? や # も名前のうち
  const isUrl = /^[a-z][a-z0-9+.-]*:\/\//i.test(url);
  const m = isUrl ? /[?&]p=([^&#]*)/.exec(url) : null;
  if (m) {
    try {
      p = decodeURIComponent(m[1]);
    } catch {
      p = m[1];
    }
  } else if (isUrl) {
    p = url.replace(/[?#].*$/, "");
  }
  const last = p.split(/[\\/]/).filter(Boolean).pop();
  return last || url;
}
