// Own the OpenCode V2 client and translate its API and events to line-delimited JSON.
import { createInterface } from "node:readline"
import { conversation, project, details } from "./history.mjs"
import { seed, update } from "./live.mjs"
import { catalog, suggest, submit, pendingRequests } from "./interaction.mjs"
import { action } from "./actions.mjs"
import { modelState as publishModels, selectModel, modelsByDir } from "./models.mjs"
import { resolve } from "node:path"

const directory = process.argv[2]
const sessions = new Map()
const folders = new Map()
const drafts = new Map()
const queues = new Map()
const activity = new Map()
const loading = new Map()
const expanded = new Map()
const dirty = new Map()
const replying = new Set()
const autoApproving = new Set()
const manualPermissions = new Set()
const agentsByDir = new Map()
let paint
const pendingInputs = []
let client
let Service
let ready = false

/** Write one event to the Rust process without mixing in diagnostic output. */
function emit(id, kind, data = null) {
  process.stdout.write(JSON.stringify({ id, kind, data }) + "\n")
}

/** Restore expansion by stable row ID, keeping collapsed output out of the UI. */
function snapshot(id, state) {
  return conversation([...state.values()]).map((row) => {
    const open = expanded.get(id)?.get(row.id)
    return open ? { ...row, expanded: true, details: details(state.get(open.message), open.part) } : row
  })
}

/** Batch high-frequency deltas into one frame while keeping each message identity. */
function schedule(id, sessionID, messageID) {
  dirty.set(`${id}:${messageID}`, { id, sessionID, messageID })
  paint ||= setTimeout(() => {
    paint = null
    for (const { id, sessionID, messageID } of dirty.values()) {
      const message = activity.get(sessionID)?.get(messageID)
      if (!message) continue
      const start = activity.get(sessionID).turnStarts?.get(messageID)
      emit(id, "parts", { message: messageID, rows: project(message, false, start) })
      for (const item of expanded.get(id)?.values() || []) {
        if (item.message === messageID) emit(id, "details", { row: item.row, body: details(message, item.part) })
      }
    }
    dirty.clear()
  }, 16)
}

/** Give API failures a useful message without exposing credentials or headers. */
function message(error) {
  return error?.message || error?.cause?.message || String(error)
}

/** Find the local sidebar row for a server-owned session. */
function localID(sessionID) {
  for (const [id, session] of sessions) if (session === sessionID) return id
  return null
}

/** Read line-delimited commands immediately; queue until the client connects. */
function listen() {
  const reader = createInterface({ input: process.stdin })
  reader.on("line", (line) => {
    let input
    try { input = JSON.parse(line) } catch { return }
    if (!ready) {
      pendingInputs.push(input)
      return
    }
    dispatch(input)
  })
  reader.on("close", () => process.exit(0))
}

/** Run commands in order per chat without blocking other chats. */
function dispatch(input) {
  const question = ["form", "form-cancel"].includes(input.kind) ? `${input.id}:${input.form}` : null
  if (question) {
    if (replying.has(question)) return
    replying.add(question)
    emit(input.id, "question-busy", true)
  }
  if (["suggest", "details", "attachment"].includes(input.kind)) {
    command(input).catch((error) => emit(input.id, "error", message(error)))
    return
  }
  if (input.kind === "interrupt") {
    const sessionID = sessions.get(input.id)
    if (sessionID) client.session.interrupt({ sessionID }).catch((error) => emit(input.id, "error", message(error)))
    return
  }
  const previous = queues.get(input.id) || Promise.resolve()
  const next = previous.then(() => command(input)).catch((error) => {
    emit(input.id, "error", message(error))
    if (input.kind === "prompt") emit(input.id, "submitted", { ok: false, text: input.text })
    if (["form", "form-cancel"].includes(input.kind)) emit(input.id, "question-busy", false)
  })
  queues.set(input.id, next)
  next.finally(() => { if (queues.get(input.id) === next) queues.delete(input.id); if (question) replying.delete(question) })
}

/** Replay commands that arrived while Node was still connecting. */
function flush() {
  ready = true
  for (const input of pendingInputs.splice(0)) dispatch(input)
}

/** Refresh the full projected history, following pagination in order. */
async function history(id, sessionID) {
  if (loading.has(sessionID)) { loading.get(sessionID).again = true; return loading.get(sessionID).task }
  const buffer = []
  const load = { buffer }
  loading.set(sessionID, load)
  load.task = (async () => {
  const messages = []
  let cursor
  do {
    const page = await client.message.list({ sessionID, ...(cursor ? { cursor } : { order: "asc" }), limit: 100 })
    messages.push(...page.data)
    cursor = page.cursor.next
  } while (cursor)
  const state = seed(messages)
  state.inbox = new Map(activity.get(sessionID)?.inbox || [])
  // Apply only events newer than the fetched message settlement where possible.
  for (const event of buffer) {
    const existing = state.get(event.data?.assistantMessageID)
    if (!existing?.time?.completed || event.created > existing.time.completed) update(state, event)
  }
  if (sessions.get(id) !== sessionID) return
  activity.set(sessionID, state)
  emit(id, "history", snapshot(id, state))
  })()
  try { await load.task } finally { loading.delete(sessionID) }
  if (load.again && sessions.get(id) === sessionID) await history(id, sessionID)
}

/** Refresh available models and identify the current session model. */
async function modelState(id, session, dir) {
  return publishModels(client, emit, id, session, dir)
}

/** Show outstanding agent requests, including after a desktop restart. */
async function pending(id, sessionID) {
  const [permissions, inbox] = await Promise.all([
    client.permission.list({ sessionID }), pendingRequests(client, emit, id, sessionID),
  ])
  if (inbox) {
    const state = activity.get(sessionID) || seed([])
    state.inbox = new Map(inbox.map((item) => [item.id, item]))
    activity.set(sessionID, state)
  }
  let manual
  for (const permission of permissions) {
    const key = `${sessionID}:${permission.id}`
    if (manualPermissions.has(key)) { manual ||= permission; continue }
    if (autoApproving.has(key)) continue
    autoApproving.add(key)
    try {
      await client.permission.reply({ sessionID, requestID: permission.id, decision: "once" })
      manualPermissions.delete(key)
    } catch (error) {
      manualPermissions.add(key)
      manual ||= permission
      emit(id, "error", `Cannot auto-approve permission: ${message(error)}`)
    } finally {
      autoApproving.delete(key)
    }
  }
  emit(id, "permission", manual ? { id: manual.id, text: manual.message || `${manual.action}: ${manual.resources.join(", ")}` } : null)
}

/** Open or create a native OpenCode session for a local sidebar entry. */
async function open(command) {
  const id = command.id
  const dir = resolve(command.directory || directory)
  folders.set(id, dir)
  const agents = await catalog(client, emit, id, dir).catch((error) => { emit(id, "error", message(error)); return [] })
  agentsByDir.set(dir, agents)
  const existing = command.session || sessions.get(id)
  if (!existing) {
    if (drafts.get(id)?.directory !== dir) drafts.set(id, { directory: dir, title: command.title })
    if (!drafts.get(id).agent) drafts.get(id).agent = agents[0]?.id
    emit(id, "history", [])
    emit(id, "busy", false)
    await modelState(id, drafts.get(id), dir).catch((error) => emit(id, "error", message(error)))
    emit(id, "ready")
    return
  }
  drafts.delete(id)
  const session = await client.session.get({ sessionID: existing })
  sessions.set(id, session.id)
  emit(id, "session", session.id)
  // Older desktop builds saved agent labels rather than server IDs.
  const selected = agents.find((agent) => agent.id === session.agent || agent.name === session.agent)
  if (selected && selected.id !== session.agent) {
    await client.session.switchAgent({ sessionID: session.id, agent: selected.id })
    session.agent = selected.id
  }
  // History first for instant content; models and requests follow in parallel.
  await history(id, session.id)
  await Promise.all([
    modelState(id, session, dir).catch((error) => emit(id, "error", message(error))),
    pending(id, session.id).catch((error) => emit(id, "error", message(error))),
    client.session.active().then((active) => emit(id, "busy", Boolean(active[session.id]))).catch(() => {}),
  ])
}

/** Handle commands in order per chat without blocking other chats or the event stream. */
async function command(input) {
  const { id, kind } = input
  if (kind === "open") return open(input)
  if (kind === "close") {
    activity.delete(sessions.get(id))
    sessions.delete(id)
    drafts.delete(id)
    folders.delete(id)
    expanded.delete(id)
    return
  }
  let sessionID = sessions.get(id)
  if (kind === "prompt" && !sessionID && drafts.has(id)) {
    const draft = drafts.get(id)
    const session = await client.session.create({ location: { directory: draft.directory }, title: draft.title })
    sessionID = session.id
    sessions.set(id, sessionID)
    emit(id, "session", sessionID)
  }
  if (kind === "prompt" && sessionID && drafts.has(id)) {
    const draft = drafts.get(id)
    if (draft.model) await client.session.switchModel({ sessionID, model: draft.model })
    if (draft.agent) await client.session.switchAgent({ sessionID, agent: draft.agent })
    drafts.delete(id)
  }
  const draft = drafts.get(id)
  const dir = folders.get(id) || directory
  if (kind === "suggest") return suggest(client, emit, id, dir, input.query)
  if (kind === "details") {
    const items = expanded.get(id) || new Map()
    expanded.set(id, items)
    if (input.open === false) { items.delete(input.row); return }
    items.set(input.row, input)
    emit(id, "details", { row: input.row, body: details(activity.get(sessionID)?.get(input.message), input.part) })
    return
  }
  if (kind === "attachment") {
    const message = activity.get(sessionID)?.get(input.message) || await client.session.message.get({ sessionID, messageID: input.message })
    const file = message.files?.[Number(input.part)]
    if (!file) throw new Error("Attachment is unavailable")
    emit(id, "attachment-view", file)
    return
  }
  if (kind === "agent") {
    const agent = agentsByDir.get(dir)?.find((item) => item.id === input.agent)
    if (!agent) throw new Error("Agent is not available for this project")
    if (draft) draft.agent = agent.id
    else await client.session.switchAgent({ sessionID, agent: agent.id })
    emit(id, "agent", agent.id)
    return
  }
  if (kind === "model" || kind === "variant") return selectModel(client, emit, input, dir, sessionID, draft)
  if (!sessionID) throw new Error("Session is not ready. Reopen this chat to retry.")
  if (kind === "action") {
    await action(client, emit, id, sessionID, input.action)
    await history(id, sessionID)
    return
  }
  if (kind === "prompt") {
    await submit(client, sessionID, input, dir)
    emit(id, "submitted", { ok: true, text: input.text })
    await pendingRequests(client, emit, id, sessionID)
  } else if (kind === "rename") {
    await client.session.update({ sessionID, title: input.title })
  } else if (kind === "permission") {
    manualPermissions.delete(`${sessionID}:${input.request}`)
    await client.permission.reply({ sessionID, requestID: input.request, decision: input.decision })
    await pending(id, sessionID)
  } else if (kind === "form") {
    await client.session.form.reply({ sessionID, formID: input.form, answer: input.answer })
    emit(id, "question-busy", false)
    await pending(id, sessionID)
  } else if (kind === "form-cancel") {
    await client.session.form.cancel({ sessionID, formID: input.form })
    emit(id, "question-busy", false)
    await pending(id, sessionID)
  }
}

/** Route live server events by session, then reconcile the final authoritative history. */
async function events(onConnected) {
  let reconnect = false
  while (true) {
    try {
      for await (const event of client.event.subscribe()) {
        onConnected()
        if (event.type === "server.connected" && reconnect) {
          reconnect = false
          for (const [id, sessionID] of sessions) {
            history(id, sessionID).catch((error) => emit(id, "error", message(error)))
            pending(id, sessionID).catch((error) => emit(id, "error", message(error)))
            client.session.active().then((active) => emit(id, "busy", Boolean(active[sessionID]))).catch(() => {})
          }
        }
        if (["model.updated", "provider.updated", "models-dev.refreshed"].includes(event.type)) {
          modelsByDir.clear()
          for (const [id, sessionID] of sessions) {
            client.session.get({ sessionID }).then((session) => modelState(id, session, folders.get(id) || directory)).catch((error) => emit(id, "error", message(error)))
          }
        }
        const sessionID = event.data?.sessionID || event.data?.form?.sessionID
        const id = localID(sessionID)
        if (id === null) continue
        const rows = activity.get(sessionID) || new Map()
        activity.set(sessionID, rows)
        loading.get(sessionID)?.buffer.push(event)
        const changed = update(rows, event)
        for (const messageID of changed) schedule(id, sessionID, messageID)
        if (event.type.startsWith("session.inbox.")) {
          pendingRequests(client, emit, id, sessionID).catch((error) => emit(id, "error", message(error)))
          if (event.type === "session.inbox.delivered") history(id, sessionID).catch((error) => emit(id, "error", message(error)))
        }
        if (["session.compaction.started", "session.compaction.ended", "session.compaction.failed", "session.message.content.updated"].includes(event.type)) history(id, sessionID).catch((error) => emit(id, "error", message(error)))
        if (event.type === "session.execution.started") emit(id, "busy", true)
        if (event.type === "session.execution.failed") emit(id, "error", event.data?.error?.message || "Execution failed")
        if (["session.execution.succeeded", "session.execution.failed", "session.execution.interrupted", "session.idle"].includes(event.type)) {
          emit(id, "busy", false)
          history(id, sessionID).catch((error) => emit(id, "error", message(error)))
        }
        if (["session.model.selected", "session.agent.selected"].includes(event.type)) client.session.get({ sessionID }).then((session) => modelState(id, session, folders.get(id) || directory)).catch((error) => emit(id, "error", message(error)))
        if (["permission.asked", "permission.replied", "form.created", "form.replied", "form.cancelled"].includes(event.type)) {
          pending(id, sessionID).catch((error) => emit(id, "error", message(error)))
        }
      }
    } catch (error) {
      emit(-1, "error", `OpenCode event stream: ${message(error)}`)
    }
    // Streams are live-only: resubscribe and reload after any gap.
    reconnect = true
    await new Promise((resolve) => setTimeout(resolve, 1000))
  }
}

/** Connect to a V2 service after accepting requests from the desktop app. */
async function main() {
  if (!directory) throw new Error("Missing project directory")
  listen()
  emit(-1, "bridge-started")
  const { OpenCode } = await import("@opencode/client")
  const service = await import("@opencode/client/service")
  Service = service.Service
  const version = (value) => value.startsWith("2.")
  const endpoint = await Service.discover({ version }) || await Service.ensure({ version })
  emit(-1, "service-ready")
  const { OpenCode: Make } = { OpenCode }
  client = OpenCode.make({ baseUrl: endpoint.url, headers: Service.headers(endpoint) })
  let connected
  const listening = new Promise((resolve) => { connected = resolve })
  events(connected).catch((error) => emit(-1, "error", message(error)))
  await Promise.race([
    listening,
    new Promise((_, reject) => setTimeout(() => reject(new Error("Event stream did not connect")), 15000)),
  ])
  flush()
  emit(-1, "ready")
}

main().catch((error) => {
  emit(-1, "error", `Cannot connect to OpenCode V2: ${message(error)}. Install/configure OpenCode V2 and retry.`)
  process.exitCode = 1
})
