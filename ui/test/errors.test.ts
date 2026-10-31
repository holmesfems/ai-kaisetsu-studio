// logic/errors.ts: invoke の reject を ApiError にそろえ、失敗の文にする
import test from "node:test";
import assert from "node:assert/strict";
import { ERROR_HINT, errorMessage, failText, toApiError } from "../src/logic/errors.ts";

test("toApiError: ApiError の形・ただの文字列・Error を受ける", () => {
  const a = { code: "not_found", message: "台本がありません", detail: "No such file" } as const;
  assert.deepEqual(toApiError(a), a);
  assert.deepEqual(toApiError("だめでした"), { code: "failed", message: "だめでした", detail: "だめでした" });
  const e = toApiError(new Error("boom"));
  assert.equal(e.code, "failed");
  assert.equal(e.message, "boom");
  // 知らないコードの object や null も落ちない
  assert.equal(toApiError({ code: "zzz", message: "x" }).code, "failed");
  assert.equal(toApiError(null).message, "null");
  // message を持つ object は [object Object] にしない（コードが未知なら failed）
  assert.deepEqual(toApiError({ message: "だめ" }), { code: "failed", message: "だめ", detail: "だめ" });
  assert.equal(toApiError({ code: "zzz", message: "x", detail: "d" }).detail, "d");
  // cause は有効なコードのときだけ残る
  assert.equal(toApiError({ code: "partly_saved", message: "m", detail: "d", cause: "need_ffmpeg" }).cause, "need_ffmpeg");
  assert.equal(toApiError({ code: "partly_saved", message: "m", detail: "d", cause: "zzz" }).cause, undefined);
  assert.equal(toApiError({ code: "constructor", message: "m" }).code, "failed");
});

test("failText: 「{what}: {message}」（what は呼ぶ側が書く文の頭）", () => {
  assert.equal(failText("台本を保存できませんでした", "ディスクが一杯です"), "台本を保存できませんでした: ディスクが一杯です");
  assert.equal(
    failText("台本を読み込めませんでした", { code: "bad_format", message: "書式が正しくありません（行 3、列 5）", detail: "" }),
    "台本を読み込めませんでした: 書式が正しくありません（行 3、列 5）",
  );
});

test("failText: partly_saved は文の頭を付けずそのまま", () => {
  const m = "効果音の設定は保存しましたが、効果音を作り直せませんでした: 中止しました";
  assert.equal(failText("効果音の設定を保存できませんでした", { code: "partly_saved", message: m, detail: m }), m);
});

test("failText: partly_saved は元の種類（cause）の案内を付ける", () => {
  const m = "効果音の設定は保存しましたが、効果音を作り直せませんでした: ffmpeg が見つかりません";
  assert.equal(
    failText("効果音の設定を保存できませんでした", { code: "partly_saved", message: m, detail: m, cause: "need_ffmpeg" }),
    `${m}。${ERROR_HINT.need_ffmpeg}`,
  );
  // 案内の無い cause は message だけ
  assert.equal(failText("x", { code: "partly_saved", message: m, detail: m, cause: "denied" }), m);
});

test("failText: コードごとの案内が付く", () => {
  const f = failText("書き出せませんでした", { code: "need_ffmpeg", message: "ffmpeg が見つかりません。", detail: "" });
  assert.equal(f, `書き出せませんでした: ffmpeg が見つかりません。${ERROR_HINT.need_ffmpeg}`);
  assert.ok(failText("音声を作成できませんでした", { code: "voice_down", message: "接続できません", detail: "" }).endsWith("VoiSona Talk が起動しているか確認してください"));
  assert.ok(failText("保存できませんでした", { code: "no_project", message: "回を開いていません", detail: "" }).includes("回を開いてから操作してください"));
  assert.ok(failText("描画できませんでした", { code: "engine_down", message: "止まっています", detail: "" }).includes(ERROR_HINT.engine_down!));
  // 案内が無いコードは message だけ
  assert.equal(failText("保存できませんでした", { code: "failed", message: "だめ", detail: "" }), "保存できませんでした: だめ");
});

test("errorMessage: message だけを返す", () => {
  assert.equal(errorMessage({ code: "failed", message: "理由", detail: "d" }), "理由");
  assert.equal(errorMessage("文字列"), "文字列");
});
