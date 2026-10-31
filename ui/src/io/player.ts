// 再生。音（声・効果音・BGM）は WebAudio で時刻どおりに鳴らし、その時計に映像（モニターのコマ）を合わせる。
import { akf, call, fileUrl, on } from "./api";
import { onceKeys, urlFileName } from "./notify";
import { failText } from "../logic/errors.ts";
import { lastFrameT, playSynthSkips, toast, toastError, total, useS } from "../state";
import { canResume, firstGap, QUERY_AHEAD, shouldWait, waitText } from "../logic/playwait.ts";
import { isAud } from "../logic/media.ts";
import { GUIDE_AUDIO_FILE, guideAudioItem } from "../logic/guideAudio.ts";
import type { PlayReady } from "../gen/PlayReady.ts";

/** 効果音の URL の版（&v=）を除く */
const stripVersion = (u: string) => u.replace(/[&?]v=\d+$/, "");

const get = useS.getState;
const set = useS.setState;

const voiceKey = (x: { url: string; at: number }) => `${x.url}@${x.at.toFixed(3)}`;

class Player {
  ctx: AudioContext | null = null;
  buffers = new Map<string, Promise<AudioBuffer | null>>();
  sources: AudioScheduledSourceNode[] = [];
  startAt = 0;
  offset = 0;
  seVersion = 0;
  loopRange: [number, number] | null = null;
  /** ここまで鳴らしたら止める（効果音の「前後を聞く」） */
  stopAt: number | null = null;
  private raf = 0;
  /** play() のたびに増やす（読み込み待ちだった音を、止めた・やり直したあとに鳴らさない） */
  private gen = 0;
  /** 鳴らす予定の声（url と置く時刻が鍵）。再生中に並びが変わったとき、変わった声だけ置き直す */
  private voiceSched = new Map<string, AudioScheduledSourceNode[]>();
  /** 音声の読み込み失敗を URL ごとに 1 回だけ知らせる */
  private failNotes = onceKeys();
  /** 準備できていない区間の手前で止まって待っているか（AudioContext を suspend している） */
  private waiting = false;
  /** 「待たずに再生」: pause・seek・次の play まで待たない */
  private skipWait = false;
  /** いま ready を問い合わせている最中か（同時に 1 つまで） */
  private checking = false;
  /** play_ready が失敗した（その再生では待たない） */
  private readyFailed = false;
  /** 「声を待たない」知らせを出した（再生ごとに 1 回） */
  private notedNoVoice = false;
  private checkTimer = 0;
  /** play() の準備（読み込み・問い合わせ）の最中の gen。0 なら始めている最中ではない */
  private starting = 0;
  /** この再生の終わり（秒） */
  private endT = 0;
  /** モニターの状態（演出を隠している・字幕を消しているときは、その分を待たない） */
  private mon = { hide: [] as string[], subs: true };
  /** suspend / resume は非同期なので、順番に 1 つずつ。そのとき欲しい状態（waiting）を見て合わせる */
  private ctxChain: Promise<void> = Promise.resolve();

  ensureCtx() {
    if (!this.ctx) this.ctx = new AudioContext({ sampleRate: 48000 });
    return this.ctx;
  }

  setMonitor(m: { hide: string[]; subs: boolean }) {
    this.mon = m;
  }

  /** 音の時計を waiting に合わせる（待つなら suspend、そうでなければ resume）。最後に欲しかった状態に必ず落ち着く */
  private syncCtx(): Promise<void> {
    this.ctxChain = this.ctxChain.then(async () => {
      const ctx = this.ctx;
      if (!ctx) return;
      try {
        if (this.waiting) {
          if (ctx.state === "running") await ctx.suspend();
        } else if (ctx.state !== "running") {
          await ctx.resume();
        }
      } catch (e) {
        // 時計を動かせない（再生が固まって見える）ときは 1 回だけ知らせる
        if (!this.waiting && this.failNotes.first("ctx-resume")) {
          toastError("音の再生を開始できませんでした", e);
        }
      }
    });
    return this.ctxChain;
  }

  /** 待つ声の行か（声のエンジンが使えない・再生しながら作るのをあきらめた・この再生で作れなかった行は待たない） */
  private voiceWaitable = (lineId: string): boolean => {
    if (!get().voice?.ok) return false;
    // 声を鳴らさない（トークのミュート）・一括作成の最中（再生しながらは作らない）は待たない
    if (get().project?.doc.voice_muted || get().synthAllRun) return false;
    const k = playSynthSkips();
    return !k.gaveUp && !k.failed.has(lineId);
  };

  /** 準備できていない区間を Rust に問い合わせる。失敗したらその再生では待たない（1 回だけ知らせる） */
  private async query(t: number, gen: number): Promise<PlayReady | null> {
    try {
      return await call("play_ready", {
        t0: t,
        t1: Math.min(t + QUERY_AHEAD, this.endT),
        scale: get().scale,
        fx: this.mon.hide.length === 0,
        subs: this.mon.subs,
      });
    } catch (e) {
      if (this.gen === gen && !this.readyFailed) {
        this.readyFailed = true;
        toast(`${failText("再生の準備を確認できませんでした", e)}。待たずに再生します`, "error", 9000);
      }
      return null;
    }
  }

  /** 声を待たない行があれば、その再生で 1 回だけ知らせる */
  private noteNoVoice(r: PlayReady) {
    if (this.notedNoVoice || get().voice?.ok || !r.voice.length) return;
    this.notedNoVoice = true;
    toast("声のエンジンが起動していないため、まだ作っていない声は待たずに再生します", "info");
  }

  private enterWait(text: string) {
    this.waiting = true;
    set({ playWait: { text } });
    this.syncCtx();
  }

  private leaveWait() {
    if (!this.waiting && !get().playWait) return;
    this.waiting = false;
    set({ playWait: null });
    this.syncCtx();
  }

  /** 「待たずに再生」: 待ちを解いて、pause・seek・次の play までは待たない */
  skipPlayWait() {
    this.skipWait = true;
    this.leaveWait();
  }

  /** 再生中、問い合わせの結果で止める・再開する。再生を止めた・やり直したあとに届いた結果は捨てる */
  private async check(gen: number) {
    if (this.checking || this.skipWait || this.readyFailed || !get().playing || this.gen !== gen) return;
    this.checking = true;
    try {
      const r = await this.query(this.now(), gen);
      if (this.gen !== gen || !get().playing || this.skipWait) return;
      if (!r) {
        if (this.readyFailed) this.leaveWait();
        return;
      }
      this.noteNoVoice(r);
      const t = this.now();
      const gap = firstGap(r, t, this.voiceWaitable);
      if (!this.waiting) {
        if (shouldWait(t, this.endT, gap)) this.enterWait(waitText(r, t, this.voiceWaitable));
      } else if (canResume(t, this.endT, gap)) {
        this.leaveWait();
      } else {
        const text = waitText(r, t, this.voiceWaitable);
        if (get().playWait?.text !== text) set({ playWait: { text } });
      }
    } finally {
      this.checking = false;
    }
  }

  now(): number {
    if (!get().playing || !this.ctx) return get().t;
    return this.ctx.currentTime - this.startAt;
  }

  buffer(url: string): Promise<AudioBuffer | null> {
    let p = this.buffers.get(url);
    if (!p) {
      const ctx = this.ensureCtx();
      p = fetch(url)
        .then((r) => (r.ok ? r.arrayBuffer() : Promise.reject({ status: r.status })))
        .then((ab) => ctx.decodeAudioData(ab))
        .then((b) => {
          this.failNotes.forget(url);
          return b;
        })
        .catch((e): null => {
          // 失敗を覚えたままにしない（次に鳴らすときに読み直す）
          if (this.buffers.get(url) === p) this.buffers.delete(url);
          const status = typeof e?.status === "number" ? (e.status as number) : null;
          // 効果音の wav は、まだ作っていない回では無いのが普通なので 404 は知らせない
          const se = this.seUrl();
          const isSe = se != null && stripVersion(se) === stripVersion(url);
          if (!(isSe && status === 404) && this.failNotes.first(url)) {
            const why = status != null ? String(status) : e?.message || String(e);
            toast(`音声を読み込めませんでした: ${urlFileName(url)}（${why}）`, "error");
          }
          return null;
        });
      this.buffers.set(url, p);
    }
    return p;
  }

  /** 鳴らしている素材の音の URL（作り直されたら前の版を捨てるため） */
  private guideAudioUrl: string | null = null;

  forget(url: string) {
    this.buffers.delete(url);
  }

  seUrl(): string | null {
    const d = get().desc;
    if (!d) return null;
    const dir = d.path.replace(/[\\/][^\\/]+$/, "");
    const stem = d.path.split(/[\\/]/).pop()!.replace(/\.py$/, "");
    return fileUrl(`${dir}/out/${stem}_se.wav`) + `&v=${this.seVersion}`;
  }

  /** 鳴らすもの: [url, 置く時刻, 素材のどこから, 長さ, 音量, フェードイン, フェードアウト, ループ] */
  private plan() {
    const s = get();
    const doc = s.project?.doc;
    const out: { voice?: boolean; url: string; at: number; from: number; dur: number | null; gain: number; fi: number; fo: number; loop: boolean }[] = [];
    if (!doc) return out;
    const dbg = (d: number) => Math.pow(10, d / 20);
    if (!doc.voice_muted) {
      for (const it of s.layout) {
        if (it.audio) out.push({ voice: true, url: akf(`/audio/${it.audio}.wav`), at: it.start, from: 0, dur: null, gain: dbg(doc.voice_db), fi: 0, fo: 0, loop: false });
      }
    }
    const se = this.seUrl();
    if (se && !doc.se_muted) out.push({ url: se, at: 0, from: 0, dur: null, gain: dbg(doc.se_db), fi: 0, fo: 0, loop: false });
    // 攻略動画の素材の音（本体が組み立てどおりに並べた wav。作り直されたら rev で読み直す）
    const root = s.project?.root;
    const ga = root ? guideAudioItem(s.guide?.audio_rev, doc, (rev) => fileUrl(`${root}/${GUIDE_AUDIO_FILE}`) + `&v=${rev}`) : null;
    if (ga) {
      // 前の版の音（大きい）は捨てる
      if (this.guideAudioUrl && this.guideAudioUrl !== ga.url) this.forget(this.guideAudioUrl);
      this.guideAudioUrl = ga.url;
      out.push(ga);
    }
    for (const tr of doc.tracks) {
      if (tr.muted) continue;
      for (const c of tr.clips) {
        if (!isAud(c.src)) continue;
        out.push({ url: fileUrl(c.src), at: c.start, from: c.src_in, dur: c.dur, gain: dbg(tr.volume_db + c.volume_db), fi: c.fade_in, fo: c.fade_out, loop: c.loop });
      }
    }
    return out;
  }

  async play() {
    if (get().playing || this.starting) return;
    // pause() が増やすので、読み込みや問い合わせを待つ間に止められたら（gen が変わったら）始めない
    const gen = ++this.gen;
    this.starting = gen;
    this.waiting = false;
    this.skipWait = false;
    this.checking = false;
    this.readyFailed = false;
    this.notedNoVoice = false;
    try {
      await this.start(gen);
    } finally {
      if (this.starting === gen) this.starting = 0;
    }
  }

  /** play() の本体。待っている間に止められた・やり直されたら（gen が変わったら）始めない */
  private async start(gen: number) {
    const ctx = this.ensureCtx();
    await this.syncCtx(); // 前の待ちで止めたままの時計を動かす（resume）
    if (this.gen !== gen) return;
    let t0 = get().t;
    const end = this.stopAt !== null ? Math.min(this.stopAt, total()) : this.loopRange ? this.loopRange[1] : total();
    if (t0 >= end - 0.05) t0 = this.loopRange ? this.loopRange[0] : 0;
    const items = this.plan().filter((x) => x.dur === null || x.at + x.dur > t0 || x.loop);
    // 近い所の音を先に読み込む（遠い所は鳴らすまでに間に合えばよい）
    const near = items.filter((x) => x.at < t0 + 20);
    await Promise.all(near.map((x) => this.buffer(x.url)));
    if (this.gen !== gen) return;
    this.endT = end;
    // 鳴らし始める前に 1 回見て、手前が未準備なら待った状態で始める
    const r0 = await this.query(t0, gen);
    if (this.gen !== gen) return;
    let wait0: string | null = null;
    if (r0) {
      this.noteNoVoice(r0);
      if (shouldWait(t0, end, firstGap(r0, t0, this.voiceWaitable))) wait0 = waitText(r0, t0, this.voiceWaitable);
    }
    this.offset = t0;
    const startTime = ctx.currentTime + 0.06;
    this.startAt = startTime - t0;
    this.sources = [];
    this.voiceSched.clear();
    for (const x of items) {
      const key = voiceKey(x);
      const mark: AudioScheduledSourceNode[] = [];
      if (x.voice) this.voiceSched.set(key, mark);
      this.buffer(x.url).then((buf) => {
        if (!buf || !get().playing || this.gen !== gen) return;
        const ss = this.schedule(buf, x, t0);
        if (x.voice && this.voiceSched.get(key) === mark) this.voiceSched.set(key, ss);
      });
    }
    this.starting = 0;
    set({ playing: true, t: t0 });
    // 声を作り始められるよう、playing にしてから時計を止める（止まっている間も声は作り続ける）
    if (wait0 != null) this.enterWait(wait0);
    clearInterval(this.checkTimer);
    this.checkTimer = window.setInterval(() => this.check(gen), 200);
    const tick = () => {
      if (!get().playing) return;
      const t = this.now();
      if (t >= end) {
        if (this.loopRange && this.stopAt === null) {
          this.pause();
          set({ t: this.loopRange[0] });
          this.play();
          return;
        }
        this.pause();
        set({ t: Math.min(end, lastFrameT()) });
        return;
      }
      set({ t });
      this.raf = requestAnimationFrame(tick);
    };
    this.raf = requestAnimationFrame(tick);
  }

  private schedule(buf: AudioBuffer, x: ReturnType<Player["plan"]>[number], t0: number): AudioScheduledSourceNode[] {
    const ctx = this.ctx!;
    const made: AudioScheduledSourceNode[] = [];
    const g = ctx.createGain();
    g.gain.value = x.gain;
    g.connect(ctx.destination);
    const segLen = buf.duration - x.from;
    const segments: [number, number][] = []; // [タイムラインの時刻, 長さ]
    if (x.loop && segLen > 0) {
      for (let a = x.at; a < x.at + (x.dur ?? segLen); a += segLen) segments.push([a, Math.min(segLen, x.at + (x.dur ?? segLen) - a)]);
    } else {
      segments.push([x.at, Math.min(segLen, x.dur ?? segLen)]);
    }
    for (const [a, len] of segments) {
      if (a + len <= t0) continue;
      const src = ctx.createBufferSource();
      src.buffer = buf;
      src.connect(g);
      const when = this.startAt + a;
      const skip = Math.max(0, t0 - a);
      const startCtx = Math.max(ctx.currentTime, when + skip);
      src.start(startCtx, x.from + skip, Math.max(0.001, len - skip));
      this.sources.push(src);
      made.push(src);
    }
    // フェード（ループでない素材だけ）
    if (!x.loop && x.dur) {
      const a = this.startAt + x.at;
      const b = a + x.dur;
      if (x.fi > 0) {
        g.gain.setValueAtTime(0, Math.max(ctx.currentTime, a));
        g.gain.linearRampToValueAtTime(x.gain, Math.max(ctx.currentTime, a + x.fi));
      }
      if (x.fo > 0) {
        g.gain.setValueAtTime(x.gain, Math.max(ctx.currentTime, b - x.fo));
        g.gain.linearRampToValueAtTime(0, Math.max(ctx.currentTime, b));
      }
    }
    return made;
  }

  /**
   * 再生中に声の並びが変わった（再生しながら音声を作った・台本の直し）ときに、鳴らす声を今の並びに合わせる。
   * 位置も時計もそのまま。同じ声（同じ音声・同じ時刻）は鳴らしたまま、変わった声だけを止めて置き直す
   */
  resync() {
    if (!get().playing || !this.ctx) return;
    const gen = this.gen;
    const items = this.plan().filter((x) => x.voice);
    const keys = new Set(items.map(voiceKey));
    for (const [k, ss] of [...this.voiceSched]) {
      if (keys.has(k)) continue;
      for (const s of ss) {
        try {
          s.stop();
        } catch { /* もう止まっている */ }
      }
      this.voiceSched.delete(k);
    }
    for (const x of items) {
      const key = voiceKey(x);
      if (this.voiceSched.has(key)) continue;
      const mark: AudioScheduledSourceNode[] = [];
      this.voiceSched.set(key, mark);
      this.buffer(x.url).then((buf) => {
        if (!buf || !get().playing || this.gen !== gen || this.voiceSched.get(key) !== mark) return;
        // 読み込みの間に進んだ分は、鳴らし始めを飛ばして合わせる
        this.voiceSched.set(key, this.schedule(buf, x, this.now()));
      });
    }
  }

  pause() {
    this.stopAt = null;
    // 待ちも解く（時計を resume して、ready の結果が遅れて届いても使わない）
    this.gen++;
    clearInterval(this.checkTimer);
    this.skipWait = false;
    this.waiting = false;
    if (get().playWait) set({ playWait: null });
    this.starting = 0;
    this.syncCtx();
    if (!get().playing) return;
    const t = this.now();
    cancelAnimationFrame(this.raf);
    for (const s of this.sources) {
      try {
        s.stop();
      } catch { /* もう止まっている */ }
    }
    this.sources = [];
    this.voiceSched.clear();
    set({ playing: false, t });
  }

  toggle() {
    // 始めている最中の Space は止める（pause が gen を進めて始めるのをやめさせる）
    if (get().playing || this.starting) this.pause();
    else this.play();
  }

  seek(t: number) {
    const was = get().playing || this.starting !== 0;
    if (was) this.pause(); // 始めている最中なら取り消して、新しい位置からやり直す
    set({ t: Math.max(0, Math.min(t, lastFrameT())) });
    if (was) this.play();
  }

  /** a〜b 秒だけ鳴らして止める（効果音の「前後を聞く」。声・BGM・効果音を全部重ねて） */
  playSpan(a: number, b: number) {
    if (get().playing || this.starting) this.pause();
    set({ t: Math.max(0, Math.min(a, lastFrameT())) });
    this.stopAt = b;
    this.play();
  }

  /** 1 行だけ鳴らす（台本の ▶。効果音の候補の試聴は gain で音量をかける）。読めなかったら false */
  private single: AudioBufferSourceNode | null = null;
  private singleGain: GainNode | null = null;
  async playOne(url: string, onEnd?: () => void, gain = 1): Promise<boolean> {
    const ctx = this.ensureCtx();
    if (this.waiting) this.pause(); // 待っている再生の時計を、一行の試聴の resume で動かさない
    await ctx.resume();
    this.stopOne();
    const buf = await this.buffer(url);
    if (!buf) {
      // 読めなかったものは覚えておかない（作り直し・置き直したあとにもう一度読めるように）
      this.buffers.delete(url);
      return false;
    }
    const src = ctx.createBufferSource();
    src.buffer = buf;
    // 鳴らしている間も setOneGain で音量を変えられるように、いつも gain を挟む
    const g = ctx.createGain();
    g.gain.value = gain;
    g.connect(ctx.destination);
    src.connect(g);
    src.onended = () => {
      if (this.single === src) {
        this.single = null;
        this.singleGain = null;
      }
      onEnd?.();
    };
    src.start();
    this.single = src;
    this.singleGain = g;
    return true;
  }

  /** 1 つだけ鳴らしている音（playOne）の音量を、鳴らしたまま変える（効果音の試聴のスライドバー） */
  setOneGain(gain: number) {
    if (this.singleGain && this.ctx) this.singleGain.gain.setTargetAtTime(gain, this.ctx.currentTime, 0.01);
  }

  stopOne() {
    try {
      this.single?.stop();
    } catch { /* 無視 */ }
  }
}

export const player = new Player();

// 効果音の設定が変わって作り直された（画面か Claude）ら、再生の効果音を読み直す
on("se_bank", (v) => {
  if (v.rendered) player.seVersion = v.rev;
});

// 再生中に声の並びが変わった（再生しながら作った声が入る・後ろの行がずれる）ら、鳴らす声を合わせ直す
useS.subscribe((st, prev) => {
  if (st.playing && prev.playing && st.layout !== prev.layout) player.resync();
});
