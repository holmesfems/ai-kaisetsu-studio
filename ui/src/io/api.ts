import { invoke } from "@tauri-apps/api/core";
import { listen, UnlistenFn } from "@tauri-apps/api/event";
import type { Commands, Events } from "./commands";

/** 引数が無いコマンドは、引数なしで呼ぶ */
type CallArgs<K extends keyof Commands> = Commands[K]["args"] extends undefined ? [] : [args: Commands[K]["args"]];

/**
 * Tauri のコマンドを呼ぶ。名前・引数・戻り値は io/commands.ts の表で決まる。
 * 失敗すると Promise は `ApiError`（gen/ApiError.ts: code・message・detail）で reject する。文にするときは logic/errors.ts の `failText` を通す
 */
export function call<K extends keyof Commands>(cmd: K, ...args: CallArgs<K>): Promise<Commands[K]["ret"]> {
  return invoke<Commands[K]["ret"]>(cmd, args[0] as Record<string, unknown> | undefined);
}

/** 本体からの知らせを受ける。名前と中身の型は io/commands.ts の Events で決まる */
export function on<K extends keyof Events>(name: K, fn: (v: Events[K]) => void): Promise<UnlistenFn> {
  return listen<Events[K]>(name, (e) => fn(e.payload));
}

/** akf:// の URL（Windows の WebView2 では http://akf.localhost/…） */
export function akf(path: string, q: Record<string, string | number | undefined | null> = {}): string {
  const qs = Object.entries(q)
    .filter(([, v]) => v !== undefined && v !== null && v !== "")
    .map(([k, v]) => `${k}=${encodeURIComponent(String(v))}`)
    .join("&");
  return `http://akf.localhost${path}${qs ? "?" + qs : ""}`;
}

export const fileUrl = (p: string) => akf("/file", { p });
