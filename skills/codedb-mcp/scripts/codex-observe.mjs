#!/usr/bin/env node

import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import readline from 'node:readline'
import { performance } from 'node:perf_hooks'

const CHARS_PER_TOKEN = 4
const FIRST_LINE_READ_CAP = 1024 * 1024
const MAX_STREAM_FILE_BYTES = 2 * 1024 * 1024 * 1024
const LARGE_OUTPUT_TOKEN_THRESHOLD = 3000
const LARGE_READ_TOKEN_THRESHOLD = 1000
const DEFAULT_SINCE = '24h'
const DEFAULT_TOP = 12

function printHelp() {
  console.log(`codedb Codex observer

Usage:
  node scripts/codex-observe.mjs --project <repo-root> [options]

Options:
  --project <path>      Target repository path. Defaults to current directory.
  --sessions <path>     Codex sessions directory. Defaults to ~/.codex/sessions.
  --transcript <path>   Inspect an explicit Codex exec --json transcript. May be repeated.
  --since <duration>    Only scan sessions modified in this window. Examples: 2h, 24h, 7d. Default: 24h.
  --limit <n>           Max recent session files to inspect after filtering by mtime.
  --top <n>             Number of high-cost calls to show. Default: 12.
  --json                Emit machine-readable JSON instead of Markdown.
  --show-prompts        Include short user prompt previews in JSON output.
  --fail-on-mcp-shell   Strict audit: exit with code 2 when source lookup shell/file calls occur after codedb_* in the same turn.
  --help                Show this help.
`)
}

function parseArgs(argv) {
  const out = {
    project: process.cwd(),
    sessions: defaultSessionsDir(),
    transcripts: [],
    since: DEFAULT_SINCE,
    limit: 0,
    top: DEFAULT_TOP,
    json: false,
    showPrompts: false,
    failOnMcpShell: false,
    help: false,
  }

  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i]
    if (arg === '--help' || arg === '-h') {
      out.help = true
    } else if (arg === '--json') {
      out.json = true
    } else if (arg === '--show-prompts') {
      out.showPrompts = true
    } else if (arg === '--fail-on-mcp-shell') {
      out.failOnMcpShell = true
    } else if (arg === '--project') {
      out.project = argv[++i] ?? out.project
    } else if (arg === '--sessions') {
      out.sessions = argv[++i] ?? out.sessions
    } else if (arg === '--transcript') {
      const transcript = argv[++i]
      if (transcript) out.transcripts.push(transcript)
    } else if (arg === '--since') {
      out.since = argv[++i] ?? out.since
    } else if (arg === '--limit') {
      out.limit = Number.parseInt(argv[++i] ?? '0', 10) || 0
    } else if (arg === '--top') {
      out.top = Number.parseInt(argv[++i] ?? String(DEFAULT_TOP), 10) || DEFAULT_TOP
    } else if (!arg.startsWith('-') && out.project === process.cwd()) {
      out.project = arg
    }
  }

  out.project = path.resolve(out.project)
  out.transcripts = out.transcripts.map(item => path.resolve(item))
  return out
}

function defaultSessionsDir() {
  return path.join(os.homedir(), '.codex', 'sessions')
}

function parseDurationMs(value) {
  const raw = String(value ?? '').trim().toLowerCase()
  if (!raw) return parseDurationMs(DEFAULT_SINCE)
  const match = raw.match(/^(\d+(?:\.\d+)?)(ms|s|m|h|d)?$/)
  if (!match) return parseDurationMs(DEFAULT_SINCE)
  const amount = Number(match[1])
  const unit = match[2] ?? 'h'
  const scale = unit === 'ms'
    ? 1
    : unit === 's'
      ? 1000
      : unit === 'm'
        ? 60_000
        : unit === 'h'
          ? 3_600_000
          : 86_400_000
  return amount * scale
}

function normalizePath(value) {
  return String(value ?? '')
    .replace(/\\/g, '/')
    .replace(/\/+$/, '')
    .toLowerCase()
}

function pathMatchesProject(cwd, project) {
  const c = normalizePath(cwd)
  const p = normalizePath(project)
  if (!c || !p) return false
  if (c === p || c.startsWith(`${p}/`)) return true
  const base = p.split('/').filter(Boolean).pop()
  return !!base && (c.endsWith(`/${base}`) || c.includes(`/${base}/`))
}

function discoverJsonlFiles(root, sinceMs, limit) {
  if (!fs.existsSync(root)) return []
  const cutoff = Date.now() - sinceMs
  const stack = [root]
  const files = []

  while (stack.length > 0) {
    const dir = stack.pop()
    let entries
    try {
      entries = fs.readdirSync(dir, { withFileTypes: true })
    } catch {
      continue
    }
    for (const entry of entries) {
      const full = path.join(dir, entry.name)
      if (entry.isDirectory()) {
        stack.push(full)
        continue
      }
      if (!entry.isFile() || !entry.name.endsWith('.jsonl')) continue
      let stat
      try {
        stat = fs.statSync(full)
      } catch {
        continue
      }
      if (stat.mtimeMs < cutoff) continue
      files.push({ path: full, mtimeMs: stat.mtimeMs, size: stat.size })
    }
  }

  files.sort((a, b) => b.mtimeMs - a.mtimeMs)
  return limit > 0 ? files.slice(0, limit) : files
}

async function readFirstJsonLine(filePath) {
  const stream = fs.createReadStream(filePath, {
    encoding: 'utf8',
    start: 0,
    end: FIRST_LINE_READ_CAP - 1,
  })
  stream.on('error', () => {})
  const rl = readline.createInterface({ input: stream, crlfDelay: Infinity })
  try {
    for await (const line of rl) {
      if (!line.trim()) return null
      try {
        return JSON.parse(line)
      } catch {
        return null
      }
    }
  } catch {
    return null
  } finally {
    rl.close()
    stream.destroy()
  }
  return null
}

async function readSessionMeta(filePath) {
  const first = await readFirstJsonLine(filePath)
  if (!first || first.type !== 'session_meta') return null
  const payload = first.payload ?? {}
  const originator = String(payload.originator ?? '').toLowerCase()
  if (originator && !originator.startsWith('codex')) return null
  return {
    cwd: payload.cwd ?? '',
    sessionId: payload.session_id ?? path.basename(filePath, '.jsonl'),
    model: payload.model ?? '',
    timestamp: first.timestamp ?? '',
  }
}

function safeJsonParse(value) {
  if (typeof value !== 'string' || value.trim() === '') return null
  try {
    return JSON.parse(value)
  } catch {
    return null
  }
}

function approxTokens(chars) {
  return Math.ceil(Math.max(0, chars) / CHARS_PER_TOKEN)
}

function extractWallSeconds(output) {
  const match = String(output).match(/Wall time:\s*([0-9.]+)\s*seconds/i)
  return match ? Number(match[1]) : null
}

function extractMcpText(output) {
  const raw = String(output ?? '')
  const match = raw.match(/(?:^|\r?\n)Output:\r?\n/)
  if (!match || match.index === undefined) return ''
  const payload = raw.slice(match.index + match[0].length).trim()
  if (!payload.startsWith('[')) return payload
  try {
    const items = JSON.parse(payload)
    if (!Array.isArray(items)) return payload
    return items
      .map(item => {
        if (item && typeof item === 'object' && typeof item.text === 'string') return item.text
        return ''
      })
      .filter(Boolean)
      .join('\n')
  } catch {
    return payload
  }
}

function parseBundleChildren(text) {
  const source = String(text ?? '')
  const matches = []
  const re = /^--- \[(\d+)\]\s+([A-Za-z0-9_]+)\s+---\s*$/gm
  let match
  while ((match = re.exec(source)) !== null) {
    matches.push({
      index: Number(match[1]),
      tool: match[2],
      markerStart: match.index,
      contentStart: re.lastIndex,
    })
  }
  const children = []
  for (let i = 0; i < matches.length; i++) {
    const current = matches[i]
    const next = matches[i + 1]
    const section = source.slice(current.contentStart, next ? next.markerStart : source.length).trim()
    const time = section.match(/(?:^|\n)time_ms:\s*([0-9.]+)/)
    children.push({
      index: current.index,
      tool: current.tool,
      timeMs: time ? Number(time[1]) : null,
      chars: section.length,
      approxTokens: approxTokens(section.length),
    })
  }
  return children
}

function parseTokenUsage(entry, state, summary) {
  const info = entry.payload?.info
  if (!info || typeof info !== 'object') return

  const total = info.total_token_usage
  const last = info.last_token_usage
  const cumulativeTotal = typeof total?.total_tokens === 'number' ? total.total_tokens : null
  if (cumulativeTotal !== null && state.prevCumulativeTotal === cumulativeTotal) return
  if (cumulativeTotal !== null) state.prevCumulativeTotal = cumulativeTotal

  let input = 0
  let cached = 0
  let output = 0
  let reasoning = 0

  if (last) {
    input = last.input_tokens ?? 0
    cached = last.cached_input_tokens ?? 0
    output = last.output_tokens ?? 0
    reasoning = last.reasoning_output_tokens ?? 0
  } else if (total) {
    input = Math.max(0, (total.input_tokens ?? 0) - state.prevInput)
    cached = Math.max(0, (total.cached_input_tokens ?? 0) - state.prevCached)
    output = Math.max(0, (total.output_tokens ?? 0) - state.prevOutput)
    reasoning = Math.max(0, (total.reasoning_output_tokens ?? 0) - state.prevReasoning)
  }

  if (total) {
    state.prevInput = total.input_tokens ?? state.prevInput
    state.prevCached = total.cached_input_tokens ?? state.prevCached
    state.prevOutput = total.output_tokens ?? state.prevOutput
    state.prevReasoning = total.reasoning_output_tokens ?? state.prevReasoning
  }

  if (input + cached + output + reasoning === 0) return
  summary.modelTokens.input += Math.max(0, input - cached)
  summary.modelTokens.cachedInput += cached
  summary.modelTokens.output += output
  summary.modelTokens.reasoning += reasoning
  summary.modelTokenEvents += 1
}

function isCodedbTool(name) {
  return String(name ?? '').startsWith('codedb_')
}

function classifyNonCodedbLookup(name, args, project) {
  if (name === 'shell_command') return classifyShellCodeLookup(args, project)

  const codeTools = new Set([
    'get_context_tree',
    'get_file_skeleton',
    'get_blast_radius',
    'semantic_code_search',
    'semantic_identifier_search',
  ])
  return codeTools.has(name) ? 'non_codedb_code_tool' : null
}

function classifyShellCodeLookup(args, project) {
  const command = String(args?.command ?? args?.cmd ?? '')
  const workdir = String(args?.workdir ?? '')
  if (!command) return null
  const normalizedWorkdir = normalizePath(workdir)
  const normalizedProject = normalizePath(project)
  const inProject = normalizedWorkdir && normalizedProject && (
    normalizedWorkdir === normalizedProject || normalizedWorkdir.startsWith(`${normalizedProject}/`)
  )
  const commandText = command.replace(/\\/g, '/').toLowerCase()
  if (/(^|[\/\s])(\.agents|skills|\.codedb-mcp)([\/\s]|$)/.test(commandText)) return null
  if (/\b(agents\.md|claude\.md|setup-for-agent\.md|skill\.md|package-lock\.json)\b/.test(commandText)) return null

  const broadSearch = /\b(rg|grep|select-string|findstr)\b/.test(commandText)
  const fileDump = /\b(get-content|cat|type)\b/.test(commandText)
  const treeLookup = /\b(get-childitem|ls|dir)\b/.test(commandText)
  if (!broadSearch && !fileDump && !treeLookup) return null
  if (!inProject && !commandText.includes(normalizedProject)) return null
  return 'shell_or_file_lookup'
}

function summarizeArgs(call) {
  const args = call.args ?? {}
  if (call.name === 'codedb_bundle') {
    const ops = Array.isArray(args.ops) ? args.ops : []
    const counts = new Map()
    for (const op of ops) {
      const tool = op?.tool ?? 'unknown'
      counts.set(tool, (counts.get(tool) ?? 0) + 1)
    }
    const tools = [...counts.entries()].map(([tool, count]) => count === 1 ? tool : `${tool}x${count}`).join(', ')
    return `ops=${ops.length}${tools ? ` (${tools})` : ''}`
  }
  if (call.name === 'codedb_read') {
    if (Array.isArray(args.paths)) return `paths=${args.paths.length}`
    const range = args.line_start || args.line_end ? ` lines=${args.line_start ?? ''}-${args.line_end ?? ''}` : ''
    return `path=${shortPath(args.path)}${range}${args.compact ? ' compact=true' : ''}`
  }
  if (call.name === 'codedb_search' || call.name === 'codedb_context' || call.name === 'codedb_flow') {
    const q = args.task ? `task=${clip(String(args.task), 60)}` : args.query ? `query=${clip(String(args.query), 60)}` : Array.isArray(args.queries) ? `queries=${args.queries.length}` : ''
    const max = args.max_results ? ` max_results=${args.max_results}` : args.max_files ? ` max_files=${args.max_files}` : ''
    const glob = args.path_glob ? ` path_glob=${shortPath(args.path_glob)}` : ''
    const budget = args.max_tokens ? ` max_tokens=${args.max_tokens}` : ''
    return `${q}${max}${glob}${budget}`.trim()
  }
  if (call.name === 'codedb_callers') {
    if (Array.isArray(args.targets)) return `targets=${args.targets.length}`
    return `symbol=${args.symbol ?? ''} definition=${shortPath(args.definition_path)}:${args.definition_line ?? ''}`
  }
  if (call.name === 'codedb_deps') {
    return `path=${shortPath(args.path)} direction=${args.direction ?? ''}${args.transitive ? ' transitive=true' : ''}`
  }
  if (call.name === 'shell_command') {
    return clip(String(args.command ?? ''), 100)
  }
  return Object.keys(args).filter(k => k !== 'project').slice(0, 5).join(', ')
}

function shortPath(value) {
  if (!value) return ''
  const raw = String(value).replace(/\\/g, '/')
  const parts = raw.split('/').filter(Boolean)
  return parts.length <= 4 ? raw : parts.slice(-4).join('/')
}

function clip(value, max) {
  const text = String(value ?? '').replace(/\s+/g, ' ').trim()
  return text.length <= max ? text : `${text.slice(0, Math.max(0, max - 3))}...`
}

async function parseSessionFile(source, options) {
  const stat = fs.statSync(source.path)
  const session = {
    file: source.path,
    cwd: source.meta.cwd,
    sessionId: source.meta.sessionId,
    startedAt: source.meta.timestamp,
    mtime: new Date(source.mtimeMs).toISOString(),
    size: stat.size,
    calls: [],
    bundleChildren: [],
    modelTokens: { input: 0, cachedInput: 0, output: 0, reasoning: 0 },
    modelTokenEvents: 0,
    turns: new Map(),
  }
  if (stat.size > MAX_STREAM_FILE_BYTES) {
    session.skipped = `session file exceeds ${MAX_STREAM_FILE_BYTES} bytes`
    return session
  }

  const callsById = new Map()
  const tokenState = {
    prevCumulativeTotal: null,
    prevInput: 0,
    prevCached: 0,
    prevOutput: 0,
    prevReasoning: 0,
  }
  let turn = 0
  let promptPreview = ''

  const stream = fs.createReadStream(source.path, { encoding: 'utf8' })
  const rl = readline.createInterface({ input: stream, crlfDelay: Infinity })
  try {
    for await (const line of rl) {
      if (!line.trim()) continue
      let entry
      try {
        entry = JSON.parse(line)
      } catch {
        continue
      }

      if (entry.type === 'response_item' && entry.payload?.type === 'message' && entry.payload?.role === 'user') {
        turn += 1
        const texts = Array.isArray(entry.payload.content)
          ? entry.payload.content.map(item => item?.text).filter(value => typeof value === 'string')
          : []
        promptPreview = options.showPrompts ? clip(texts.join(' '), 160) : ''
        if (!session.turns.has(turn)) {
          session.turns.set(turn, { turn, promptPreview, calls: [] })
        }
        continue
      }

      if (entry.type === 'event_msg' && entry.payload?.type === 'token_count') {
        parseTokenUsage(entry, tokenState, session)
        continue
      }

      if (entry.type === 'response_item' && entry.payload?.type === 'function_call') {
        const payload = entry.payload
        const args = safeJsonParse(payload.arguments) ?? {}
        const call = {
          callId: payload.call_id ?? '',
          sessionId: session.sessionId,
          sessionFile: session.file,
          timestamp: entry.timestamp ?? '',
          turn,
          name: payload.name ?? '',
          namespace: payload.namespace ?? '',
          args,
          argsChars: typeof payload.arguments === 'string' ? payload.arguments.length : 0,
          outputChars: 0,
          outputTextChars: 0,
          approxOutputTokens: 0,
          wallSeconds: null,
          success: null,
          codeLookupKind: null,
          bundleChildren: [],
          sequence: session.calls.length + 1,
        }
        call.codeLookupKind = classifyNonCodedbLookup(call.name, args, options.project)
        session.calls.push(call)
        callsById.set(call.callId, call)
        if (!session.turns.has(turn)) {
          session.turns.set(turn, { turn, promptPreview, calls: [] })
        }
        session.turns.get(turn).calls.push(call)
        continue
      }

      if (entry.type === 'response_item' && entry.payload?.type === 'function_call_output') {
        const call = callsById.get(entry.payload.call_id)
        if (!call) continue
        const output = String(entry.payload.output ?? '')
        call.outputChars = output.length
        call.approxOutputTokens = approxTokens(output.length)
        call.wallSeconds = extractWallSeconds(output)
        call.success = /Exit code:\s*0\b/.test(output) || (!/Exit code:\s*\d+\b/.test(output) && !/\berror\b/i.test(output.slice(0, 300)))
        const mcpText = isCodedbTool(call.name) ? extractMcpText(output) : ''
        call.outputTextChars = mcpText ? mcpText.length : output.length
        if (call.name === 'codedb_bundle') {
          call.bundleChildren = parseBundleChildren(mcpText)
          session.bundleChildren.push(...call.bundleChildren.map(child => ({
            ...child,
            sessionId: session.sessionId,
            turn: call.turn,
            parentCallId: call.callId,
            parentTimestamp: call.timestamp,
          })))
        }
      }
    }
  } finally {
    rl.close()
    stream.destroy()
  }

  return session
}

async function parseExecTranscriptFile(filePath, options) {
  const stat = fs.statSync(filePath)
  const session = {
    file: filePath,
    cwd: options.project,
    sessionId: path.basename(filePath, path.extname(filePath)),
    startedAt: '',
    mtime: new Date(stat.mtimeMs).toISOString(),
    size: stat.size,
    calls: [],
    bundleChildren: [],
    modelTokens: { input: 0, cachedInput: 0, output: 0, reasoning: 0 },
    modelTokenEvents: 0,
    turns: new Map([[1, { turn: 1, promptPreview: '', calls: [] }]]),
  }
  if (stat.size > MAX_STREAM_FILE_BYTES) {
    session.skipped = `transcript file exceeds ${MAX_STREAM_FILE_BYTES} bytes`
    return session
  }

  const callsById = new Map()
  const stream = fs.createReadStream(filePath, { encoding: 'utf8' })
  const rl = readline.createInterface({ input: stream, crlfDelay: Infinity })
  try {
    for await (const line of rl) {
      if (!line.trim()) continue
      let entry
      try {
        entry = JSON.parse(line)
      } catch {
        continue
      }

      if (entry.type === 'turn.completed' && entry.usage) {
        const usage = entry.usage
        session.modelTokens.input += Math.max(0, (usage.input_tokens ?? 0) - (usage.cached_input_tokens ?? 0))
        session.modelTokens.cachedInput += usage.cached_input_tokens ?? 0
        session.modelTokens.output += usage.output_tokens ?? 0
        session.modelTokens.reasoning += usage.reasoning_output_tokens ?? 0
        session.modelTokenEvents += 1
        continue
      }

      const item = entry.item
      if (!item || (entry.type !== 'item.started' && entry.type !== 'item.completed')) continue
      if (entry.type === 'item.started') {
        const call = execItemToCall(item, session, options)
        if (!call) continue
        session.calls.push(call)
        callsById.set(call.callId, call)
        session.turns.get(1).calls.push(call)
        continue
      }

      const call = callsById.get(item.id) ?? execItemToCall(item, session, options)
      if (!call) continue
      if (!callsById.has(call.callId)) {
        session.calls.push(call)
        callsById.set(call.callId, call)
        session.turns.get(1).calls.push(call)
      }
      const output = execItemOutputText(item)
      call.outputChars = output.length
      call.outputTextChars = output.length
      call.approxOutputTokens = approxTokens(output.length)
      call.success = item.status === 'completed' || item.exit_code === 0 || !item.error
      if (call.name === 'codedb_bundle') {
        call.bundleChildren = parseBundleChildren(output)
        session.bundleChildren.push(...call.bundleChildren.map(child => ({
          ...child,
          sessionId: session.sessionId,
          turn: call.turn,
          parentCallId: call.callId,
          parentTimestamp: call.timestamp,
        })))
      }
    }
  } finally {
    rl.close()
    stream.destroy()
  }
  return session
}

function execItemToCall(item, session, options) {
  if (item.type !== 'mcp_tool_call' && item.type !== 'command_execution') return null
  const name = item.type === 'mcp_tool_call' ? item.tool : 'shell_command'
  const args = item.type === 'mcp_tool_call'
    ? (item.arguments ?? {})
    : { command: item.command ?? '', workdir: options.project }
  const call = {
    callId: item.id ?? `${session.sessionId}:${session.calls.length + 1}`,
    sessionId: session.sessionId,
    sessionFile: session.file,
    timestamp: '',
    turn: 1,
    name,
    namespace: item.server ?? '',
    args,
    argsChars: JSON.stringify(args).length,
    outputChars: 0,
    outputTextChars: 0,
    approxOutputTokens: 0,
    wallSeconds: null,
    success: null,
    codeLookupKind: null,
    bundleChildren: [],
    sequence: session.calls.length + 1,
  }
  call.codeLookupKind = classifyNonCodedbLookup(call.name, args, options.project)
  return call
}

function execItemOutputText(item) {
  if (item.type === 'command_execution') return String(item.aggregated_output ?? '')
  const result = item.result
  if (!result || typeof result !== 'object') return ''
  const content = Array.isArray(result.content) ? result.content : []
  return content
    .map(part => typeof part?.text === 'string' ? part.text : '')
    .filter(Boolean)
    .join('\n')
}

function aggregate(sessions, options, scannedFiles, elapsedMs) {
  const calls = sessions.flatMap(session => session.calls)
  const codedbCalls = calls.filter(call => isCodedbTool(call.name))
  const bundleCalls = codedbCalls.filter(call => call.name === 'codedb_bundle')
  const childCalls = sessions.flatMap(session => session.bundleChildren)
  const shellLookupCalls = calls.filter(call => call.codeLookupKind)
  const mcpShellLockViolations = findMcpShellLockViolations(sessions)
  const modelTokens = sessions.reduce((acc, session) => {
    acc.input += session.modelTokens.input
    acc.cachedInput += session.modelTokens.cachedInput
    acc.output += session.modelTokens.output
    acc.reasoning += session.modelTokens.reasoning
    return acc
  }, { input: 0, cachedInput: 0, output: 0, reasoning: 0 })

  const toolOutputTokens = sum(calls, call => call.approxOutputTokens)
  const codedbOutputTokens = sum(codedbCalls, call => call.approxOutputTokens)
  const shellLookupOutputTokens = sum(shellLookupCalls, call => call.approxOutputTokens)

  const directStats = groupStats(codedbCalls, call => call.name, call => call.approxOutputTokens, call => call.wallSeconds)
  const childStats = groupStats(childCalls, call => `bundle:${call.tool}`, call => call.approxTokens, call => call.timeMs === null ? null : call.timeMs / 1000)
  const topCalls = [...calls]
    .filter(call => call.approxOutputTokens > 0)
    .sort((a, b) => b.approxOutputTokens - a.approxOutputTokens)
    .slice(0, options.top)
    .map(call => ({
      sessionId: call.sessionId,
      turn: call.turn,
      timestamp: call.timestamp,
      name: call.name,
      tokens: call.approxOutputTokens,
      chars: call.outputChars,
      wallSeconds: call.wallSeconds,
      args: summarizeArgs(call),
      codedb: isCodedbTool(call.name),
      codeLookupKind: call.codeLookupKind,
    }))

  return {
    project: options.project,
    sessionsDir: options.sessions,
    since: options.since,
    scannedFiles,
    matchedSessions: sessions.length,
    elapsedMs,
    modelTokens,
    toolOutputTokens,
    codedbOutputTokens,
    shellLookupOutputTokens,
    mcpShellLockViolations,
    codedbCalls: codedbCalls.length,
    bundleCalls: bundleCalls.length,
    bundleChildCalls: childCalls.length,
    shellLookupCalls: shellLookupCalls.length,
    directStats,
    childStats,
    topCalls,
    findings: buildFindings(sessions, calls, codedbCalls, shellLookupCalls, mcpShellLockViolations),
  }
}

function sum(items, fn) {
  return items.reduce((acc, item) => acc + fn(item), 0)
}

function groupStats(items, keyFn, tokenFn, secondsFn) {
  const groups = new Map()
  for (const item of items) {
    const key = keyFn(item)
    let group = groups.get(key)
    if (!group) {
      group = { name: key, calls: 0, outputTokens: 0, maxOutputTokens: 0, wallSeconds: [] }
      groups.set(key, group)
    }
    const tokens = tokenFn(item) || 0
    group.calls += 1
    group.outputTokens += tokens
    group.maxOutputTokens = Math.max(group.maxOutputTokens, tokens)
    const seconds = secondsFn(item)
    if (typeof seconds === 'number' && Number.isFinite(seconds)) group.wallSeconds.push(seconds)
  }
  return [...groups.values()]
    .map(group => ({
      name: group.name,
      calls: group.calls,
      outputTokens: group.outputTokens,
      avgOutputTokens: group.calls ? Math.round(group.outputTokens / group.calls) : 0,
      maxOutputTokens: group.maxOutputTokens,
      avgWallMs: group.wallSeconds.length ? Math.round(1000 * average(group.wallSeconds)) : null,
      maxWallMs: group.wallSeconds.length ? Math.round(1000 * Math.max(...group.wallSeconds)) : null,
    }))
    .sort((a, b) => b.outputTokens - a.outputTokens || b.calls - a.calls)
}

function average(values) {
  return values.reduce((acc, value) => acc + value, 0) / values.length
}

function findMcpShellLockViolations(sessions) {
  const violations = []
  for (const session of sessions) {
    for (const turn of session.turns.values()) {
      let lockCall = null
      for (const call of turn.calls) {
        if (isCodedbTool(call.name)) {
          lockCall ??= call
          continue
        }
        if (lockCall && call.codeLookupKind) {
          violations.push({
            sessionId: session.sessionId,
            sessionFile: session.file,
            turn: turn.turn,
            lockTool: lockCall.name,
            lockArgs: summarizeArgs(lockCall),
            shellTool: call.name,
            shellArgs: summarizeArgs(call),
            tokens: call.approxOutputTokens,
            timestamp: call.timestamp,
          })
        }
      }
    }
  }
  return violations
}

function buildFindings(sessions, calls, codedbCalls, shellLookupCalls, mcpShellLockViolations) {
  const findings = []
  if (mcpShellLockViolations.length > 0) {
    findings.push({
      title: `${mcpShellLockViolations.length} post-codedb shell/file supplemental lookup call${mcpShellLockViolations.length === 1 ? '' : 's'}`,
      impact: 'low',
      tokens: sum(mcpShellLockViolations, item => item.tokens),
      detail: mcpShellLockViolations.slice(0, 5).map(item => `${item.sessionId} t${item.turn}: after ${item.lockTool} (${item.lockArgs}) used ${item.shellTool} (${item.shellArgs}) ~${formatInt(item.tokens)} tokens`),
      recommendation: 'After codedb_* starts, keep repository lookup inside codedb_* and continue from exact paths, identifiers, symbols, callers, deps, or quoted strings already discovered; state remaining gaps instead of using shell/rg for source lookup.',
    })
  }

  const largeCodedb = codedbCalls.filter(call => call.approxOutputTokens >= LARGE_OUTPUT_TOKEN_THRESHOLD)
  if (largeCodedb.length > 0) {
    findings.push({
      title: `${largeCodedb.length} high-output codedb call${largeCodedb.length === 1 ? '' : 's'}`,
      impact: 'high',
      tokens: sum(largeCodedb, call => Math.max(0, call.approxOutputTokens - 2000)),
      detail: largeCodedb
        .sort((a, b) => b.approxOutputTokens - a.approxOutputTokens)
        .slice(0, 5)
        .map(call => `${call.name} t${call.turn} ~${formatInt(call.approxOutputTokens)} tokens (${summarizeArgs(call)})`),
      recommendation: 'Prefer codedb_flow for broad answer planning, codedb_context only for larger exact-reference packs, paths_only/compact search for exact lookup, and line-scoped reads after outline.',
    })
  }

  const broadReads = codedbCalls.filter(call => {
    if (call.name !== 'codedb_read') return false
    const args = call.args ?? {}
    const hasRange = args.line_start || args.line_end || Array.isArray(args.paths)
    return !hasRange && !args.compact && call.approxOutputTokens >= LARGE_READ_TOKEN_THRESHOLD
  })
  if (broadReads.length > 0) {
    findings.push({
      title: `${broadReads.length} broad codedb_read call${broadReads.length === 1 ? '' : 's'}`,
      impact: 'medium',
      tokens: sum(broadReads, call => Math.max(0, call.approxOutputTokens - 700)),
      detail: broadReads.slice(0, 5).map(call => `t${call.turn} ${summarizeArgs(call)} ~${formatInt(call.approxOutputTokens)} tokens`),
      recommendation: 'Use codedb_outline first, then codedb_read with line_start/line_end or paths[] batch ranges.',
    })
  }

  const broadSearches = codedbCalls.filter(call => {
    if (call.name !== 'codedb_search') return false
    const args = call.args ?? {}
    const maxResults = Number(args.max_results ?? 0)
    return call.approxOutputTokens >= LARGE_OUTPUT_TOKEN_THRESHOLD || maxResults >= 80
  })
  if (broadSearches.length > 0) {
    findings.push({
      title: `${broadSearches.length} broad search result set${broadSearches.length === 1 ? '' : 's'}`,
      impact: 'medium',
      tokens: sum(broadSearches, call => Math.max(0, call.approxOutputTokens - 1200)),
      detail: broadSearches.slice(0, 5).map(call => `t${call.turn} ${call.name} ${summarizeArgs(call)} ~${formatInt(call.approxOutputTokens)} tokens`),
      recommendation: 'For exploratory questions use codedb_flow first. For exact lookup use paths_only, narrower path_glob, and smaller max_results before reading code.',
    })
  }

  if (shellLookupCalls.length > 0) {
    findings.push({
      title: `${shellLookupCalls.length} non-codedb shell/file lookup call${shellLookupCalls.length === 1 ? '' : 's'} inside the project`,
      impact: 'medium',
      tokens: shellLookupCalls.reduce((acc, call) => acc + call.approxOutputTokens, 0),
      detail: shellLookupCalls.slice(0, 5).map(call => `t${call.turn} ${summarizeArgs(call)} ~${formatInt(call.approxOutputTokens)} tokens`),
      recommendation: 'Use codedb_* for broad discovery and graph-shaped lookup; keep shell/file lookup supplemental, narrow, read-only, and based on exact evidence terms.',
    })
  }

  const contextMisses = []
  for (const session of sessions) {
    for (const turn of session.turns.values()) {
      const names = turn.calls.map(call => call.name)
      const hasContext = names.includes('codedb_context') || names.includes('codedb_flow')
      const hasSearch = names.includes('codedb_search')
      const followups = names.filter(name => name === 'codedb_outline' || name === 'codedb_read' || name === 'codedb_deps' || name === 'codedb_callers').length
      if (!hasContext && hasSearch && followups >= 2) {
        contextMisses.push({ sessionId: session.sessionId, turn: turn.turn, names })
      }
    }
  }
  if (contextMisses.length > 0) {
    findings.push({
      title: `${contextMisses.length} exploratory turn${contextMisses.length === 1 ? '' : 's'} likely fit codedb_flow`,
      impact: 'low',
      tokens: contextMisses.length * 500,
      detail: contextMisses.slice(0, 5).map(item => `${item.sessionId} t${item.turn}: ${summarizeToolSequence(item.names.filter(isCodedbTool))}`),
      recommendation: 'Start broad feature or flow questions with codedb_flow; use codedb_context only for a larger exact-reference pack, and use outline plus line-scoped reads when snippets are needed.',
    })
  }

  findings.sort((a, b) => b.tokens - a.tokens)
  return findings
}

function summarizeToolSequence(names) {
  const max = 14
  const shown = names.slice(0, max).join(' -> ')
  const extra = names.length > max ? ` -> ... +${names.length - max} more` : ''
  return `${shown}${extra}`
}

function formatInt(value) {
  return Math.round(value).toLocaleString('en-US')
}

function renderMarkdown(report) {
  const lines = []
  lines.push('# Codex codedb-mcp Token Report')
  lines.push('')
  lines.push(`Project: \`${report.project}\``)
  lines.push(`Sessions: ${formatInt(report.matchedSessions)} matched / ${formatInt(report.scannedFiles)} scanned from \`${report.sessionsDir}\` over ${report.since}`)
  lines.push(`Parse time: ${report.elapsedMs.toFixed(1)}ms`)
  lines.push('')
  lines.push('## Summary')
  lines.push('')
  lines.push('| Metric | Value |')
  lines.push('|---|---:|')
  lines.push(`| Model input tokens | ${formatInt(report.modelTokens.input)} |`)
  lines.push(`| Model cached input tokens | ${formatInt(report.modelTokens.cachedInput)} |`)
  lines.push(`| Model output tokens | ${formatInt(report.modelTokens.output)} |`)
  lines.push(`| Model reasoning tokens | ${formatInt(report.modelTokens.reasoning)} |`)
  lines.push(`| Tool output tokens injected into context | ${formatInt(report.toolOutputTokens)} |`)
  lines.push(`| codedb tool output tokens | ${formatInt(report.codedbOutputTokens)} |`)
  lines.push(`| non-codedb shell/file lookup output tokens | ${formatInt(report.shellLookupOutputTokens)} |`)
  lines.push(`| post-codedb shell/file lookup calls | ${formatInt(report.mcpShellLockViolations.length)} |`)
  lines.push(`| codedb calls / bundles / bundle children | ${formatInt(report.codedbCalls)} / ${formatInt(report.bundleCalls)} / ${formatInt(report.bundleChildCalls)} |`)
  lines.push('')
  lines.push('## codedb Calls')
  lines.push('')
  renderStatsTable(lines, report.directStats)
  if (report.childStats.length > 0) {
    lines.push('')
    lines.push('## Bundle Child Breakdown')
    lines.push('')
    renderStatsTable(lines, report.childStats)
  }
  lines.push('')
  lines.push('## Highest Output Calls')
  lines.push('')
  lines.push('| Tool | Turn | Output tokens | Wall | Args |')
  lines.push('|---|---:|---:|---:|---|')
  for (const call of report.topCalls) {
    const wall = call.wallSeconds === null ? '' : `${(call.wallSeconds * 1000).toFixed(1)}ms`
    lines.push(`| \`${call.name}\` | ${call.turn} | ${formatInt(call.tokens)} | ${wall} | ${escapeTable(call.args)} |`)
  }
  lines.push('')
  lines.push('## Findings')
  lines.push('')
  if (report.findings.length === 0) {
    lines.push('No obvious codedb token waste detected in the scanned sessions.')
  } else {
    for (let i = 0; i < report.findings.length; i++) {
      const finding = report.findings[i]
      lines.push(`${i + 1}. **${finding.title}** (${finding.impact}, estimated avoidable output ~${formatInt(finding.tokens)} tokens)`)
      for (const item of finding.detail) lines.push(`   - ${item}`)
      lines.push(`   - Recommendation: ${finding.recommendation}`)
    }
  }
  lines.push('')
  lines.push('Notes: token estimates use chars/4 for tool output. The report reads Codex JSONL files line by line and does not modify transcripts.')
  return lines.join('\n')
}

function renderStatsTable(lines, stats) {
  if (stats.length === 0) {
    lines.push('No calls.')
    return
  }
  lines.push('| Tool | Calls | Output tokens | Avg tokens | Max tokens | Avg wall | Max wall |')
  lines.push('|---|---:|---:|---:|---:|---:|---:|')
  for (const row of stats.slice(0, 20)) {
    lines.push([
      `| \`${row.name}\``,
      formatInt(row.calls),
      formatInt(row.outputTokens),
      formatInt(row.avgOutputTokens),
      formatInt(row.maxOutputTokens),
      row.avgWallMs === null ? '' : `${formatInt(row.avgWallMs)}ms`,
      row.maxWallMs === null ? '' : `${formatInt(row.maxWallMs)}ms |`,
    ].join(' | '))
  }
}

function escapeTable(value) {
  return String(value ?? '').replace(/\|/g, '\\|')
}

async function main() {
  const options = parseArgs(process.argv.slice(2))
  if (options.help) {
    printHelp()
    return
  }
  const started = performance.now()
  const sinceMs = parseDurationMs(options.since)
  let scannedFiles = 0
  const sessions = []

  if (options.transcripts.length > 0) {
    scannedFiles = options.transcripts.length
    for (const transcript of options.transcripts) {
      sessions.push(await parseExecTranscriptFile(transcript, options))
    }
  } else {
    const candidates = discoverJsonlFiles(options.sessions, sinceMs, options.limit)
    scannedFiles = candidates.length
    const sources = []

    for (const candidate of candidates) {
      const meta = await readSessionMeta(candidate.path)
      if (!meta) continue
      if (!pathMatchesProject(meta.cwd, options.project)) continue
      sources.push({ ...candidate, meta })
    }

    for (const source of sources) {
      sessions.push(await parseSessionFile(source, options))
    }
  }

  const report = aggregate(sessions, options, scannedFiles, performance.now() - started)
  if (options.json) {
    console.log(JSON.stringify(report, null, 2))
  } else {
    console.log(renderMarkdown(report))
  }
  if (options.failOnMcpShell && report.mcpShellLockViolations.length > 0) {
    process.exitCode = 2
  }
}

main().catch(err => {
  console.error(`codex-observe failed: ${err?.message ?? err}`)
  process.exit(1)
})
