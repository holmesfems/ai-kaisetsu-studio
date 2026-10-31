// 攻略動画（studio/guide.json）の操作。状態は本体が持ち、変わるたびに guide イベントで届く（state/index.ts）。
import { call } from "../io/api";
import type { Guide } from "../gen/Guide";
import type { GuideBench } from "../gen/GuideBench";
import type { GuideFixed } from "../gen/GuideFixed";
import { editWithRetry } from "../logic/guide";
import { toast, toastError, useS } from "./store";

const set = useS.setState;
const get = useS.getState;

/** 本体の攻略動画の状態を取り直す。聞いている間に別の回を開いたら捨てる */
export async function refreshGuide(root: string | null) {
  if (!root) return;
  try {
    const g = await call("guide_get");
    if (get().project?.root !== root) return;
    set({ guide: g });
  } catch (e) {
    toastError("攻略動画の状態を取得できませんでした", e);
  }
}

/** 素材を選んで攻略動画にする（guide.json を作る）。src は回のフォルダの中の動画 */
export async function createGuide(src: string) {
  try {
    set({ guide: await call("guide_create", { src }) });
    toast("攻略動画にしました。「解析」を押してください");
  } catch (e) {
    toastError("攻略動画にできませんでした", e);
  }
}

/** 攻略動画の演出の入口（fx/guide_fx.py）を作って、演出を読み込む */
export async function createGuideFx() {
  try {
    await call("guide_fx_create");
    toast("演出のファイル（fx/guide_fx.py）を作りました");
  } catch (e) {
    toastError("演出のファイルを作れませんでした", e);
  }
}

// guide.json の変更は 1 つずつ順に行う（前の保存が返る前に次の変更が古い中身から作られて、先の変更を消さないように）
let queue: Promise<unknown> = Promise.resolve();

/** 攻略動画の変更を順番待ちで行う。task は自分の番が来てから今の状態（get()）を読むこと */
export function guideTask<T>(task: () => Promise<T>): Promise<T> {
  const run = queue.then(task, task);
  queue = run.catch(() => undefined);
  return run;
}

/** 保存の本体（順番待ちの中から呼ぶ）。rev は保存のもとにした GuideState.rev。保存できたら null、できなければ理由（通知は呼ぶ側） */
async function trySave(g: Guide, rev: number | null): Promise<unknown | null> {
  try {
    set({ guide: await call("guide_set", { guide: g, base_rev: rev }) });
    return null;
  } catch (e) {
    return e;
  }
}

/** 本体の状態を取り直す（保存の食い違いのあと）。取り直せなかったら通知を出して false */
async function refetch(): Promise<boolean> {
  try {
    set({ guide: await call("guide_get") });
    return true;
  } catch (e) {
    toastError("攻略動画の状態を取得できませんでした", e);
    return false;
  }
}

/** 順番待ちの中（guideTask の task）で、今の guide.json を fn で直して保存する。editGuide の中身そのもの（editGuide は順番待ちに並ぶので、中からは呼べない）。
 *  ほかで guide.json が変わっていて断られたら、取り直して fn をもう 1 回当てる。保存できたら true。fn が null を返したら保存しない */
export async function editGuideInTask(fn: (g: Guide) => Guide | null | Promise<Guide | null>): Promise<boolean> {
  const r = await editWithRetry({ current: () => get().guide, save: trySave, refetch }, fn);
  if (r.kind === "error") toastError("攻略動画を保存できませんでした", r.error);
  return r.kind === "saved";
}

/** 今の guide.json を fn で直して保存する。前の変更が済んでから、その時点の最新の中身を直す。保存できたら true。
 *  fn が null を返したら保存しない。fn の中・guideTask の中から呼ばないこと（順番待ちが詰まる。中では editGuideInTask） */
export function editGuide(fn: (g: Guide) => Guide | null | Promise<Guide | null>): Promise<boolean> {
  return guideTask(() => editGuideInTask(fn));
}

/** guide.json を丸ごと保存する（素材の選び直しなど）。順番待ちを通る。食い違いなら取り直さず断る（丸ごとなので重ねられない） */
export function saveGuide(g: Guide): Promise<boolean> {
  return guideTask(async () => {
    const err = await trySave(g, get().guide?.rev ?? null);
    if (err !== null) toastError("攻略動画を保存できませんでした", err);
    return err === null;
  });
}

/** Claude の変更（GuideState.claude_undo）を控えに戻す。id は画面が見せていた変更の番号（その間に変わっていた・2 回押したら本体が断る）。
 *  その後に画面で直していたら本体が断る。順番待ちを通る */
export function undoClaudeGuide(id: number): Promise<boolean> {
  return guideTask(async () => {
    try {
      set({ guide: await call("guide_undo_claude", { id }) });
      return true;
    } catch (e) {
      toastError("Claude の変更を戻せませんでした", e);
      return false;
    }
  });
}

/** 素材の解析を始める（進み具合は progress、終わりは guide イベント） */
export async function scanGuide() {
  try {
    await call("guide_scan");
  } catch (e) {
    toastError("解析を始められませんでした", e);
  }
}

/** 固定値を設定に保存する（全部の回で使う） */
export async function saveGuideFixedDefault(fixed: GuideFixed) {
  try {
    set({ guide: await call("guide_save_fixed_default", { fixed }) });
    toast("固定値を設定に保存しました");
  } catch (e) {
    toastError("固定値を保存できませんでした", e);
  }
}

/** 待機欄の大きさ・不透明度を設定に保存する（全部の回で使う。モニターは guide イベントですぐ描き直る）。保存できたら true */
export async function saveGuideBench(bench: GuideBench): Promise<boolean> {
  try {
    set({ guide: await call("guide_save_bench", { bench }) });
    return true;
  } catch (e) {
    toastError("待機欄の設定を保存できませんでした", e);
    return false;
  }
}

/**
 * 出力の時刻 t（手動パートの中）に操作を足す。足した操作の id を返す（断られたら null。理由は通知に出す）。
 * id は本体が振るので、足す前後の一覧の差から見つける
 */
export function addGuideOp(t: number): Promise<string | null> {
  return guideTask(() => addGuideOpNow(t));
}

async function addGuideOpNow(t: number): Promise<string | null> {
  const before = new Set((get().guide?.guide?.ops ?? []).map((o) => o.id));
  try {
    const g = await call("guide_op_add", { t });
    set({ guide: g });
    const id = (g.guide?.ops ?? []).find((o) => !before.has(o.id))?.id ?? null;
    if (id) set({ guideOp: id });
    return id;
  } catch (e) {
    toastError("操作を足せませんでした", e);
    return null;
  }
}
