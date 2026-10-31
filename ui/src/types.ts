import type { Line } from "./gen/Line";
import type { Block } from "./gen/Block";
import type { VoiceAdj } from "./gen/VoiceAdj";
import type { ParamsAdj } from "./gen/ParamsAdj";
import type { Script } from "./gen/Script";
import type { SeEv } from "./gen/SeEv";
import type { Group } from "./gen/Group";
import type { RecInfo } from "./gen/RecInfo";
import type { Cue } from "./gen/Cue";
import type { Desc } from "./gen/Desc";
import type { Warning as EngineWarning } from "./gen/Warning";
import type { EngineStatus } from "./gen/EngineStatus";
import type { MediaClip } from "./gen/MediaClip";
import type { Trans as Transition } from "./gen/Trans";
import type { Track } from "./gen/Track";
import type { Nest } from "./gen/Nest";
import type { ProjectDoc } from "./gen/ProjectDoc";
import type { SubStyle } from "./gen/SubStyle";
import type { SubView } from "./gen/SubView";
import type { SubLineView } from "./gen/SubLineView";
import type { ChatMsg } from "./gen/ChatMsg";
import type { Proposal } from "./gen/Proposal";
import type { ProposalItem } from "./gen/ProposalItem";
import type { Analysis } from "./gen/Analysis";
import type { VoiceItem } from "./gen/VoiceItem";
import type { Settings } from "./gen/Settings";

// Rust に元がある型は ui/src/gen/（ts-rs が cargo test で書き出す）から出し直す。画面だけの型・Rust が Value で素通しする型はここに手書き。
export type {
  Line, Block, VoiceAdj, ParamsAdj, Script, SeEv, Group, RecInfo, Cue, Desc, EngineWarning, EngineStatus,
  MediaClip, Transition, Track, Nest, ProjectDoc, SubStyle, SubView, SubLineView, ChatMsg, Proposal, ProposalItem, VoiceItem, Settings,
};

export type NestFit = Nest["fit"];

export interface Override {
  shift?: number;
  end?: number;
  dx?: number;
  dy?: number;
  scale?: number;
  opacity?: number;
  pivot?: [number, number];
  hidden?: boolean;
  z?: number;
  note?: string;
  /** 録画を差し込んだ区間（Rust がネストから書く。画面からは触らない） */
  no_placeholder?: boolean | [number, number][];
}

export interface SubTrack extends Partial<SubStyle> {
  hidden?: boolean;
  speakers?: Record<string, Partial<SubStyle>>;
}

export interface SubLine extends Partial<SubStyle> {
  /** 手で改行した字幕（改行入り）。台本と違う文にすると「字幕だけ書き換え」 */
  text?: string;
}

export interface FontInfo {
  spec: string;
  family: string;
  style: string;
  weight_axis: [number, number, number] | null;
  jp: boolean | null;
  file: string;
}

/** 字幕の行の分け方とはみ出し（subs_check。Python で測った結果だけ。見た目・区間・鍵は SubView） */
export interface SubCheck {
  lines: Record<string, { overflow: boolean; texts: string[] }>;
}

/** VoiSona の合成結果（voice_server の /api/synth・/api/lookup のうち、画面が使う項目。Rust が持って配る） */
export type { Analysis };

export type Selection =
  | { kind: "fx"; key: string }
  | { kind: "media"; track: string; clip: string }
  | { kind: "line"; id: string }
  | { kind: "subs"; sp?: string }
  | { kind: "se"; se?: string }
  | { kind: "guide_audio" }
  | null;

// 効果音の設定の型は Rust（crates/ak-proto/src/sebank.rs）から作る
export type { SeBank } from "./gen/SeBank";
export type { SeCand } from "./gen/SeCand";
export type { SeKind } from "./gen/SeKind";
