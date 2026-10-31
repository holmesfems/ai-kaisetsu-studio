// 字幕の見た目の書き換えと、行の分け方・はみ出しの確認。描くのは Rust → Python（モニター・書き出しと同じ絵）。
import { call } from "../io/api";
import { editDoc } from "./project";
import { toastError, useS } from "./store";
import { errorMessage } from "../logic/errors.ts";
import type { FontInfo, SubCheck, SubLine, SubStyle } from "../types";

/** 共通（sp なし）か話者ごと（sp）の見た目を直す。値が null なら上書きを外す */
export function patchSubStyle(sp: string | null, patch: Partial<Record<keyof SubStyle, any>>) {
  editDoc((d) => {
    d.subtitle = { ...(d.subtitle || {}) };
    let tgt: any = d.subtitle;
    if (sp) {
      d.subtitle.speakers = { ...(d.subtitle.speakers || {}) };
      tgt = d.subtitle.speakers[sp] = { ...(d.subtitle.speakers[sp] || {}) };
    }
    for (const [k, v] of Object.entries(patch)) {
      if (v === null || v === undefined) delete tgt[k];
      else tgt[k] = v;
    }
    if (sp && !Object.keys(tgt).length) delete d.subtitle.speakers![sp];
  });
}

/** セリフごとの設定を直す（null で外す）。全部外れたら鍵ごと消す */
export function patchSubLine(key: string, patch: Partial<Record<keyof SubLine, any>> | null) {
  editDoc((d) => {
    const all = { ...(d.subtitle_lines || {}) };
    if (patch === null) delete all[key];
    else {
      const o: any = { ...(all[key] || {}) };
      for (const [k, v] of Object.entries(patch)) {
        if (v === null || v === undefined || v === "") delete o[k];
        else o[k] = v;
      }
      if (Object.keys(o).length) all[key] = o;
      else delete all[key];
    }
    d.subtitle_lines = all;
  });
}

// ------------------------------------------------------------------ 行の分け方・はみ出し（Rust に聞く）
let checkTimer: number | undefined;
export function refreshSubCheck(delay = 400) {
  clearTimeout(checkTimer);
  checkTimer = window.setTimeout(async () => {
    if (!useS.getState().project) return;
    try {
      useS.setState({ subCheck: await call("subs_check") });
    } catch {
      /* Python がまだ無いなど。モニターには出ないだけ */
    }
  }, delay);
}

// ------------------------------------------------------------------ 見た目・区間・鍵（Rust に聞く。Python は使わないのですぐ返る）
let viewTimer: number | undefined;
let viewSeq = 0;
let viewErr = "";
export function refreshSubView(delay = 30) {
  clearTimeout(viewTimer);
  viewTimer = window.setTimeout(async () => {
    const s = useS.getState();
    if (!s.project) return;
    const seq = ++viewSeq;
    try {
      const v = await call("subs_view", { doc: s.project.doc, items: s.layout });
      if (seq !== viewSeq) return; // もっと新しい依頼が出ている・回を閉じた
      useS.setState({ subView: v, subViewErr: null });
      viewErr = "";
    } catch (e) {
      if (seq !== viewSeq) return;
      // 古い見た目・鍵で表示したり書いたりしないように捨て、理由を画面に出す（toast は同じ失敗なら 1 回）
      const msg = errorMessage(e);
      useS.setState({ subView: null, subViewErr: msg });
      if (msg !== viewErr) toastError("字幕の見た目を取得できませんでした", e);
      viewErr = msg;
    }
  }, delay);
}

// 台本・設定が変わったら見た目・区間を取り直す。回を閉じた・切り替えたら、飛んでいる返事ごと捨てる
useS.subscribe((s, p) => {
  if (s.project?.root !== p.project?.root) {
    viewSeq++;
    if (s.subView || s.subViewErr) useS.setState({ subView: null, subViewErr: null });
  }
  if (!s.project) return;
  if (s.project.doc !== p.project?.doc || s.layout !== p.layout) refreshSubView();
});

// 並び・見た目が Rust 側に届いたら確認し直す
useS.subscribe((s, p) => {
  if (s.backRev !== p.backRev) refreshSubCheck(150);
});

let fontsLoading = false;
/** 入っているフォント（初回は数秒かかる。2 回目からは覚えている） */
export async function loadFonts() {
  if (useS.getState().fonts || fontsLoading) return;
  fontsLoading = true;
  try {
    useS.setState({ fonts: await call("list_fonts") });
  } catch {
    useS.setState({ fonts: [] });
  } finally {
    fontsLoading = false;
  }
}
