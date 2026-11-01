// AK Studio とつなぐ Mod。
//
// - AK Studio（アプリ）が動いていれば、1.5 秒おきに状況を見て、プロンプトの上の帯に出す
//   （回・再生位置・シーン・選んでいる演出クリップと台本の行・保留中の推敲案）
// - このセッションが AK Studio で開いている回のフォルダで動いていれば、自動で「つなぐ」。
//   つないでいる間は、画面のチャットで送ったメッセージを $.prompt.submit でこのセッションに届ける
//   （待機中なら新しいターンが始まる。作業中なら終わってから）
// - Claude の作業中（メインのターンが動いている間）は、画面のメッセージを受け取らずに AK Studio に置いておく
//   （置いてある間は、画面で取り消せば Claude に届かない。issue 旧#21）。作業中に受け取るのは取り消しの知らせだけで、
//   取り消されたのが今のターンを始めたメッセージそのものなら、$.turn.abort でそのターンを止める
//   （取り消しの知らせは、次に届けるメッセージの頭に添える）
// - つないでいる間、Claude の作業状況（作業中か・何をしているか）を /api/agent/activity で AK Studio に知らせる
//   （ターンの始まり・終わり、ツールの呼び出し、考え中・返事を書き中）。画面のチャットの「考え中」の表示になる（issue 旧#51）
// - /studio … 状況を見る、/studio on・off … つなぐ・切る（別の回のフォルダからでもつなげる）
//
// AK Studio の場所と合言葉は %USERPROFILE%\.ak-studio\session.json（%LOCALAPPDATA% は Claude のデスクトップアプリの中から
// 古い写しが見えることがあるので使わない。ak-proto の session.rs）。
import { atom, read, update } from 'claude-code'
import type { Register } from 'claude-code'

import type { StudioView } from '../types'
import { describeToolCall, hasPerson, shouldSend } from './activity.ts'
import type { Sent } from './activity.ts'

const view = atom({ plugin: 'ak-studio', key: 'view' } as const, null as StudioView | null)
const hidden = atom({ plugin: 'ak-studio', key: 'hidden' } as const, false)

type Sess = { port: number; token: string }

const norm = (p: string) => p.replace(/\\/g, '/').replace(/\/+$/, '').toLowerCase()

// 帯と /studio の表示用。先に 1/100 秒に丸めてから分と秒に分ける（59.996 秒が 0:60.00 にならないように）
function fmtT(t: number): string {
  const c = Math.round(Math.max(0, t) * 100)
  const m = Math.floor(c / 6000)
  const s = (c % 6000) / 100
  return `${m}:${s.toFixed(2).padStart(5, '0')}`
}

/** 画面のメッセージを、Claude に渡す文にする（各メッセージの文は本体が作った prompt） */
function promptText(ms: any[]): string {
  // 本体が古くて prompt が無いときは、画像・画面の状況が落ちたことを添える（取り消しの知らせは本文だけ）
  const parts = ms.map((m) => m.prompt ?? (m.id === undefined ? m.text : `${m.text}\n${OLD_APP}`))
  return (
    MARK + '\n' +
    parts.join('\n---\n') +
    '\n\n（返事は ak-studio の reply ツールで。ユーザーは AK Studio の画面を見ています）'
  )
}

// セッションの中だけの状態（Mod を読み直したら最初から）
const S = {
  cwd: '',
  interactive: false, // CLI の REPL か（デスクトップアプリでは false。hasPerson）
  started: 0, // つないだ時刻（AK Studio は、いちばん新しくつないだセッションにだけ渡す）
  linked: false,
  manual: null as boolean | null, // /studio on・off で決めたとき
  busy: false,
  warned: false,
  running: false, // メインのターンが動いているか
  submitted: null as number[] | null, // いま $.prompt.submit した画面のメッセージの id（次の turn.start で turn に移す）
  turn: null as { id: string; ids: number[] } | null, // 画面のメッセージで始まった、動いているターン
  carry: [] as string[], // ターンを止めた取り消しの知らせ。次に届けるメッセージの頭に添える
  later: [] as any[], // 作業中に受け取った、別のターン向けの取り消しの知らせ。作業が終わったら届ける
  sess: null as Sess | null, // つないでいるときの AK Studio の場所（作業状況の知らせ用）
  sent: null as Sent | null, // 最後に知らせた作業状況
  wait: null as string | null, // 間引いて送らなかった最新の文（次の tick で送る）
  kind: '', // いまモデルが出している内容（thinking / text）。切り替わりだけ知らせる
}

const MARK = '【AK Studio の画面から】'
const OLD_APP = '（AK Studio の本体が古いため、添えた画像・画面の状況を付けられませんでした。本体を作り直してください）'
const isNotice = (m: any) => m?.ctx?.kind === 'cancel_notice'

async function session($: any): Promise<Sess | null> {
  const base = (await $.env.get('USERPROFILE')) || (await $.env.get('HOME'))
  if (!base) return null
  try {
    const s = JSON.parse(await $.fs.read(`${base}/.ak-studio/session.json`))
    return s && s.port && s.token ? { port: s.port, token: s.token } : null
  } catch {
    return null
  }
}

async function api($: any, s: Sess, path: string): Promise<any | null> {
  try {
    const r = await $.http.fetch(`http://127.0.0.1:${s.port}${path}`, {
      headers: { Authorization: `Bearer ${s.token}` },
    })
    return r.ok ? JSON.parse(r.text) : null
  } catch {
    return null
  }
}

async function post($: any, s: Sess, path: string, body: any): Promise<any | null> {
  try {
    const r = await $.http.fetch(`http://127.0.0.1:${s.port}${path}`, {
      method: 'POST',
      headers: { Authorization: `Bearer ${s.token}`, 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    })
    return r.ok ? JSON.parse(r.text) : null
  } catch {
    return null
  }
}

/** Claude の作業状況を AK Studio に知らせる。同じ文は送らず、短い間隔では間引く。失敗しても何も起こさない */
async function report($: any, working: boolean, text: string) {
  try {
    const s = S.sess
    if (!S.linked || !s) return
    const now = await $.clock.now()
    if (!shouldSend(S.sent, { working, text }, now)) {
      if (working && S.sent?.working && S.sent.text !== text) S.wait = text
      return
    }
    S.wait = null
    S.sent = { working, text, at: now }
    await post($, s, '/api/agent/activity', { working, text })
  } catch {
    // 知らせられなくても、Claude の作業には関係ない
  }
}

/** Claude の作業中に届いた取り消しの知らせ */
async function onNotice($: any, s: Sess, n: any) {
  const of = Number(n?.ctx?.cancel_of)
  const t = S.turn
  // 今のターンが、取り消したメッセージ 1 通だけで始まったものなら、そのターンを止める
  // （ほかのメッセージや、ターミナルで頼んだ作業まで巻き込むときは止めない）
  if (t && t.ids.length === 1 && t.ids[0] === of) {
    try {
      await $.turn.abort({ turnId: t.id })
      S.turn = null
      S.running = false
      S.carry.push(n.text)
      await post($, s, '/api/chat/ack', { id: of })
      $.ui.toast('AK Studio で取り消されたので、Claude の作業を止めました')
      return
    } catch {
      // もう終わっていた・別のターンだった: 知らせとして届ける
    }
  }
  S.later.push(n)
}

async function tick($: any) {
  if (S.busy) return
  S.busy = true
  try {
    if (!hasPerson(S.interactive, await $.session.surfaces())) return
    const s = await session($)
    const st = s ? await api($, s, '/api/status') : null
    if (!s || !st || !st.project) {
      await update($, view, () => null)
      return
    }
    const proj = String(st.project)
    const auto = norm(S.cwd) === norm(proj) || norm(S.cwd).startsWith(norm(proj) + '/')
    const want = S.manual ?? auto
    if (want && !S.linked) {
      S.linked = true
      S.started = await $.clock.now()
      $.ui.toast(`AK Studio（${proj.split('/').pop()}）とつなぎました。画面のチャットがこのセッションに届きます`)
    } else if (!want && S.linked) {
      S.linked = false
    }
    S.sess = S.linked ? s : null
    if (S.linked && S.wait !== null && S.running) void report($, true, S.wait)
    if (S.linked) {
      // 作業中は取り消しの知らせだけを受け取る（ふつうのメッセージは AK Studio に置いたまま＝まだ取り消せる）
      const only = S.running ? '&only=notice' : ''
      const r = await api($, s, `/api/inbox?wait=0&consumer=mod&started=${S.started}${only}`)
      if (r?.superseded) {
        S.linked = false
        S.manual = false
        $.ui.toast('別の Claude のセッションが AK Studio につながったので、こちらは切りました（/studio on でつなぎ直す）')
      } else if (S.running) {
        for (const n of r?.messages ?? []) await onNotice($, s, n)
      } else if (r?.messages?.length || S.later.length) {
        const ms = [...S.later, ...(r?.messages ?? [])]
        S.later = []
        const real = ms.filter((m) => !isNotice(m))
        // ターンを止めた取り消しの知らせは、次のふつうのメッセージに添える（知らせだけで Claude を動かさない）
        const head = real.length ? S.carry.map((t) => ({ text: t })) : []
        if (real.length) S.carry = []
        S.submitted = real.map((m) => Number(m.id))
        await $.prompt.submit({ text: promptText([...head, ...ms]) })
      } else if (r?.note && !S.warned) {
        S.warned = true
        $.ui.toast('AK Studio の設定で、メッセージの届け方が「Mod」になっていません（⚙ 設定 → メッセージの届け方）')
      }
    }
    const ph = st.playhead || {}
    const sel = st.ui?.selection
    const linked = S.linked
    await update($, view, () => ({
      project: proj.split('/').pop() || proj,
      t: Number(ph.t || 0),
      scene: String(ph.scene ?? '-'),
      clip: sel && sel.kind === 'fx' ? String(sel.key) : null,
      lines: Array.isArray(st.ui?.lines) ? st.ui.lines : [],
      // 画面のバッジと同じ「採否を待つ項目の数」（古い本体では案の数）
      pending: Number(st.pending_items ?? st.pending_proposals ?? 0),
      engine: String(st.engine?.status?.state ?? ''),
      linked,
    }))
  } finally {
    S.busy = false
  }
}

export const register: Register = (on) => {
  on('session.start', async ($, e, next) => {
    S.cwd = e.cwd
    S.interactive = e.isInteractive
    await $.command.register({
      name: 'studio',
      description: 'AK Studio：状況を見る／on でつなぐ・off で切る',
      argumentHint: '[on|off]',
    })
    // デスクトップアプリは isInteractive が false で、画面は後からつながる。人がいるかは tick ごとに見る
    $.clock.every(1500, () => tick($))
    return next(e)
  })

  // メインのターンが動いているかを見ておく（作業中は画面のメッセージを AK Studio に置いておくため）
  on('turn.start', async ($, e, next) => {
    if (e.agentId === undefined) {
      S.running = true
      S.kind = ''
      void report($, true, '')
      if (S.submitted && e.text.includes(MARK)) S.turn = { id: e.turnId, ids: S.submitted }
      S.submitted = null
    }
    return next(e)
  })

  on('turn.complete', async ($, e, next) => {
    if (e.agentId === undefined) {
      S.running = false
      S.turn = null
      S.wait = null
      void report($, false, '')
    }
    return next(e)
  })

  // ツールを呼ぶたびに、何をしているかを知らせる（呼び出しは止めも変えもしない）
  on('tool.call', async ($, e, next) => {
    try {
      const t = describeToolCall(e as any)
      if (t !== null) void report($, true, t)
    } catch {
      // 知らせの失敗でツールを止めない
    }
    return next(e)
  }).catch(($, e, next) => next(e)) // 自分が失敗しても、呼び出しはそのまま通す

  // モデルが考え始めた・返事を書き始めたときだけ知らせる（chunk ごとには送らない）。chunk はそのまま流す
  on('turn.step', async function* ($, e, next) {
    const sub = (e as any).agentId !== undefined
    for await (const c of next(e)) {
      try {
        if (!sub && (c.kind === 'thinking' || c.kind === 'text') && c.kind !== S.kind) {
          S.kind = c.kind
          void report($, true, c.kind === 'thinking' ? '考えています' : '返事を書いています')
        } else if (c.kind === 'tool') {
          S.kind = ''
        }
      } catch {
        // 知らせの失敗でモデルの応答を止めない
      }
      yield c
    }
  })

  on('command.run', { command: 'studio' }, async ($, e) => {
    const a = e.args.trim()
    if (a === 'on') {
      S.manual = true
      S.linked = false // 次の tick でつなぎ直す（started を新しくする）
      return { text: 'AK Studio につなぎます。画面のチャットで送ったメッセージが、このセッションに届きます。' }
    }
    if (a === 'off') {
      S.manual = false
      return { text: 'AK Studio から切りました（帯の表示は続きます。/studio on でつなぎ直す）。' }
    }
    if (a === 'hide' || a === 'show') {
      await update($, hidden, () => a === 'hide')
      return { text: a === 'hide' ? '帯を隠しました' : '帯を出しました' }
    }
    const v = await read($, view)
    if (!v) return { text: 'AK Studio が起動していないか、回が開かれていません。' }
    return {
      text:
        `AK Studio: ${v.project}／再生位置 ${fmtT(v.t)}／シーン ${v.scene}` +
        (v.clip ? `／クリップ ${v.clip}` : '') +
        (v.lines.length ? `／行 ${v.lines.join(',')}` : '') +
        `／推敲案 ${v.pending}／演出 ${v.engine}／${v.linked ? 'つないでいます' : 'つないでいません（/studio on）'}`,
    }
  })

  on('ui.render', { component: 'AbovePrompt' }, async ($, e, next) => {
    const v = await read($, view)
    if (!v || e.props.hasSurvey || (await read($, hidden))) return next(e)
    const { Box, Text, Button } = $.ui.resolve(e)
    return (
      <Box>
        <Text color="cyan" bold>◆ AK STUDIO </Text>
        <Text>{v.project} </Text>
        <Text dimColor>{fmtT(v.t)} </Text>
        <Text>SCENE {v.scene} </Text>
        {v.clip ? <Text color="yellow">◆ {v.clip} </Text> : null}
        {v.lines.length ? <Text>¶ {v.lines.slice(0, 6).join(',')} </Text> : null}
        {v.pending ? <Text color="yellow">推敲案 {v.pending} </Text> : null}
        <Text color={v.linked ? 'green' : 'gray'}>{v.linked ? '● つないでいます ' : '○ 未接続 '}</Text>
        <Button
          key="link"
          label={v.linked ? '切る' : 'つなぐ'}
          onPress={() => {
            S.manual = !v.linked
            if (S.manual) S.linked = false
          }}
        />
      </Box>
    )
  })
}
