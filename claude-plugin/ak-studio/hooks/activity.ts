// Claude が今やっていることを、画面（AK Studio）に出す日本語にする。register.tsx から使い、activity.test.ts で確かめる。
// 純関数だけ（Mod の API には触らない）。

export type ToolCall = {
  tool: string
  skill?: string
  agentId?: string
  [k: string]: unknown
}

const str = (v: unknown): string => (typeof v === 'string' ? v.trim() : '')

/** 長い文は切る（画面の 1 行に収める） */
function clip(s: string, n = 40): string {
  const t = s.replace(/\s+/g, ' ').trim()
  return t.length > n ? t.slice(0, n - 1) + '…' : t
}

function base(p: string): string {
  const t = p.replace(/\\/g, '/').replace(/\/+$/, '')
  return t.slice(t.lastIndexOf('/') + 1) || t
}

// 公開側に入れるのは、このプラグインに入っているスキルだけ。ほかのスキルは「「名前」スキルを使っています」と出る
const SKILLS: Record<string, string> = {
  'voisona-accent': 'アクセントを調べています',
}

/** ak-studio の MCP ツール（接頭辞 mcp__ak-studio__ を除いた名前）。null は「知らせない」 */
function studioTool(name: string, e: ToolCall): string | null {
  switch (name) {
    case 'script_read': return '台本を読んでいます'
    case 'script_propose': return '推敲案を書いています'
    case 'script_apply': return '台本を直しています'
    case 'render_still': return '確認画像を描いています'
    case 'render_sheet': return '確認画像を並べて描いています'
    case 'sync_report': return '同期点を調べています'
    case 'fx_reload': return '演出を読み込み直しています'
    case 'clip_adjust': return '演出を微調整しています'
    case 'clips_list':
    case 'scenes_list': return '演出の一覧を見ています'
    case 'memo_write': return '構成メモを書いています'
    case 'export': return '書き出しています'
    case 'reply': {
      const s = str(e.status)
      if (s) return s
      return null // 返事の本文は、そのまま届く
    }
    default:
      if (name.startsWith('media_')) return '素材を置いています'
      if (name.startsWith('se_')) return '効果音を見ています'
      return 'AK Studio を操作しています'
  }
}

/**
 * ツール呼び出しを、画面に出す文にする。知らせなくてよいもの（reply の本文）は null。
 * サブエージェントの作業（agentId あり）は頭に「（サブエージェント）」を付ける。
 */
export function describeToolCall(e: ToolCall): string | null {
  const t = String(e.tool || '')
  let s: string | null
  if (t.startsWith('mcp__ak-studio__')) {
    s = studioTool(t.slice('mcp__ak-studio__'.length), e)
  } else {
    switch (t) {
      case 'Skill': {
        const raw = str(e.skill)
        const name = raw.slice(raw.lastIndexOf(':') + 1)
        s = SKILLS[name] ?? (name ? `「${name}」スキルを使っています` : 'スキルを使っています')
        break
      }
      case 'Read': {
        const f = str(e.file_path)
        s = f ? `「${base(f)}」を読んでいます` : 'ファイルを読んでいます'
        break
      }
      case 'Edit':
      case 'Write': {
        const f = str(e.file_path)
        s = f ? `「${base(f)}」を書き換えています` : 'ファイルを書き換えています'
        break
      }
      case 'Grep':
      case 'Glob': s = 'ファイルを探しています'; break
      case 'Bash':
      case 'PowerShell': {
        const d = str(e.description)
        s = d ? clip(d) : 'コマンドを実行しています'
        break
      }
      case 'WebSearch': {
        const q = str(e.query)
        s = q ? `「${clip(q, 24)}」を調べています` : 'ウェブを調べています'
        break
      }
      case 'WebFetch': s = 'ウェブページを読んでいます'; break
      case 'Agent':
      case 'Task': {
        const d = str(e.description)
        s = d ? `サブエージェントに任せています（${clip(d, 24)}）` : 'サブエージェントに任せています'
        break
      }
      default: s = `\`${t}\` を使っています`
    }
  }
  if (s === null) return null
  return e.agentId ? `（サブエージェント）${s}` : s
}

/** 知らせの間引き: 同じ文は送らない・短い間隔では送らない（ただし working の切り替えは通す） */
export type Sent = { working: boolean; text: string; at: number }

export function shouldSend(prev: Sent | null, next: { working: boolean; text: string }, now: number, minMs = 330): boolean {
  if (!prev) return true
  if (prev.working !== next.working) return true
  if (prev.text === next.text) return false
  return now - prev.at >= minMs
}
