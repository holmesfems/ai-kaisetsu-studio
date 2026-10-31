import test from 'node:test'
import assert from 'node:assert/strict'
import { describeToolCall as d, shouldSend } from './activity.ts'

test('ak-studio のツール', () => {
  assert.equal(d({ tool: 'mcp__ak-studio__script_read' }), '台本を読んでいます')
  assert.equal(d({ tool: 'mcp__ak-studio__media_add' }), '素材を置いています')
  assert.equal(d({ tool: 'mcp__ak-studio__se_set' }), '効果音を見ています')
  assert.equal(d({ tool: 'mcp__ak-studio__ui_focus' }), 'AK Studio を操作しています')
  assert.equal(d({ tool: 'mcp__ak-studio__reply', status: '数字を確認中' }), '数字を確認中')
  assert.equal(d({ tool: 'mcp__ak-studio__reply', text: 'できました' }), null)
})

test('スキル', () => {
  assert.equal(d({ tool: 'Skill', skill: 'ak-studio:voisona-accent' }), 'アクセントを調べています')
  assert.equal(d({ tool: 'Skill', skill: 'ak-studio-fx' }), '「ak-studio-fx」スキルを使っています')
  assert.equal(d({ tool: 'Skill', skill: 'foo:bar' }), '「bar」スキルを使っています')
})

test('組み込みツール', () => {
  assert.equal(d({ tool: 'Read', file_path: 'E:\\a\\b\\script.yaml' }), '「script.yaml」を読んでいます')
  assert.equal(d({ tool: 'Edit', file_path: '/x/y.py' }), '「y.py」を書き換えています')
  assert.equal(d({ tool: 'Grep', pattern: 'x' }), 'ファイルを探しています')
  assert.equal(d({ tool: 'Bash', command: 'ls' }), 'コマンドを実行しています')
  assert.equal(d({ tool: 'Bash', command: 'ls', description: 'List files' }), 'List files')
  assert.equal(d({ tool: 'WebSearch', query: 'Rust 入門' }), '「Rust 入門」を調べています')
  assert.equal(d({ tool: 'WebFetch', url: 'u' }), 'ウェブページを読んでいます')
  assert.equal(d({ tool: 'Agent', description: '調査' }), 'サブエージェントに任せています（調査）')
  assert.equal(d({ tool: 'TodoWrite' }), '`TodoWrite` を使っています')
})

test('サブエージェントは頭に付く', () => {
  assert.equal(d({ tool: 'Read', file_path: 'a.md', agentId: 'x' }), '（サブエージェント）「a.md」を読んでいます')
  assert.equal(d({ tool: 'mcp__ak-studio__reply', agentId: 'x' }), null)
})

test('間引き', () => {
  assert.equal(shouldSend(null, { working: true, text: 'a' }, 0), true)
  assert.equal(shouldSend({ working: true, text: 'a', at: 0 }, { working: true, text: 'a' }, 5000), false)
  assert.equal(shouldSend({ working: true, text: 'a', at: 0 }, { working: true, text: 'b' }, 100), false)
  assert.equal(shouldSend({ working: true, text: 'a', at: 0 }, { working: true, text: 'b' }, 400), true)
  assert.equal(shouldSend({ working: true, text: 'a', at: 0 }, { working: false, text: '' }, 10), true)
})
