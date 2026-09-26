// Own the OpenCode V2 client and translate its API and events to line-delimited JSON.
import { createInterface } from "node:readline"
import { conversation, variants } from "./history.mjs"
import { seed, update } from "./live.mjs"

const directory = process.argv[2]
const sessions = new Map()
const folders = new Map()
const drafts = new Map()
const queues = new Map()
const activity = new Map()
const pendingInputs = []
let client
let Service
let modelsByDir = new Map()
let ready = false

/** Write one event to the Rust process without mixing in diagnostic output. */
function emit(id, kind, data = null) {
  process.stdout.write(JSON.stringify({ id, kind, data }) + "\n")
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
  const previous = queues.get(input.id) || Promise.resolve()
  const next = previous.then(() => command(input)).catch((error) => {
    emit(input.id, "error", message(error))
    if (input.kind === "prompt") emit(input.id, "busy", false)
  })
  queues.set(input.id, next)
  next.finally(() => { if (queues.get(input.id) === next) queues.delete(input.id) })
}

/** Replay commands that arrived while Node was still connecting. */
function flush() {
  ready = true
  for (const input of pendingInputs.splice(0)) dispatch(input)
}

/** Refresh the full projected history, following pagination in order. */
async function history(id, sessionID) {
  const messages = []
  let cursor
  do {
    const page = await client.message.list({ sessionID, ...(cursor ? { cursor } : { order: "asc" }), limit: 100 })
    messages.push(...page.data)
    cursor = page.cursor.next
  } while (cursor)
  const rows = conversation(messages)
  activity.set(sessionID, seed(messages, rows))
  emit(id, "history", rows)
}

/** Load models once per directory, then reuse for every window and chat. */
async function cachedModels(dir) {
  const hit = modelsByDir.get(dir)
  if (hit) return hit
  const location = { directory: dir }
  const task = (async () => {
    const [listed, fallback] = await Promise.all([client.model.list({ location }), client.model.default({ location })])
    const models = listed.data.filter((model) => model.enabled)
    const fallbackModel = fallback.data
    if (fallbackModel && !models.some((m) => m.providerID === fallbackModel.providerID && m.id === fallbackModel.id)) {
      models.push(fallbackModel)
    }
    return { models, fallbackModel }
  })()
  modelsByDir.set(dir, task)
  try {
    return await task
  } catch (error) {
    modelsByDir.delete(dir)
    throw error
  }
}

/** Refresh available models and identify the current session model. */
async function modelState(id, session, dir) {
  const { models, fallbackModel } = await cachedModels(dir)
  emit(id, "models", models.map((model) => `${model.providerID}/${model.id}`))
  const ref = session.model || (fallbackModel && { providerID: fallbackModel.providerID, id: fallbackModel.id })
  const current = models.find((model) => model.providerID === ref?.providerID && model.id === ref?.id)
  emit(id, "model", ref ? `${ref.providerID}/${ref.id}` : "")
  emit(id, "variants", variants(current))
  emit(id, "variant", session.model?.variant || "default")
}

/** Show outstanding agent requests, including after a desktop restart. */
async function pending(id, sessionID) {
  const [permissions, forms] = await Promise.all([
    client.permission.list({ sessionID }), client.session.form.list({ sessionID }),
  ])
  const permission = permissions[0]
  const form = forms[0]
  emit(id, "permission", permission ? { id: permission.id, text: permission.message || `${permission.action}: ${permission.resources.join(", ")}` } : null)
  emit(id, "form", form ? { id: form.id, text: `${form.title} (${form.fields.map((field) => field.key).join(", ")})` } : null)
}

/** Open or create a native OpenCode session for a local sidebar entry. */
async function open(command) {
  const id = command.id
  const dir = command.directory || directory
  folders.set(id, dir)
  const existing = command.session || sessions.get(id)
  if (!existing) {
    drafts.set(id, { directory: dir, title: command.title })
    emit(id, "history", [])
    emit(id, "busy", false)
    await modelState(id, {}, dir).catch((error) => emit(id, "error", message(error)))
    emit(id, "ready")
    return
  }
  drafts.delete(id)
  const session = await client.session.get({ sessionID: existing })
  sessions.set(id, session.id)
  emit(id, "session", session.id)
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
    return
  }
  let sessionID = sessions.get(id)
  if (kind === "prompt" && !sessionID && drafts.has(id)) {
    const draft = drafts.get(id)
    const session = await client.session.create({ location: { directory: draft.directory }, title: draft.title })
    sessionID = session.id
    sessions.set(id, sessionID)
    drafts.delete(id)
    emit(id, "session", sessionID)
    if (draft.model) await client.session.switchModel({ sessionID, model: draft.model })
  }
  const draft = drafts.get(id)
  const dir = folders.get(id) || directory
  if (draft && kind === "model") {
    const { models } = await cachedModels(dir)
    const model = models.find((item) => `${item.providerID}/${item.id}` === input.model)
    if (!model) throw new Error("Model is not available")
    draft.model = { providerID: model.providerID, id: model.id }
    emit(id, "model", input.model)
    emit(id, "variants", variants(model))
    emit(id, "variant", "default")
    return
  }
  if (draft && kind === "variant") {
    const { models, fallbackModel } = await cachedModels(dir)
    const ref = draft.model || (fallbackModel && { providerID: fallbackModel.providerID, id: fallbackModel.id })
    const model = models.find((item) => item.providerID === ref?.providerID && item.id === ref?.id)
    if (!model || !variants(model).includes(input.variant)) throw new Error("Variant is not available")
    draft.model = { ...ref, ...(input.variant !== "default" && { variant: input.variant }) }
    emit(id, "variant", input.variant)
    return
  }
  if (!sessionID) throw new Error("Session is not ready. Reopen this chat to retry.")
  if (kind === "prompt") {
    await client.session.prompt({ sessionID, text: input.text })
  } else if (kind === "model") {
    const { models } = await cachedModels(dir)
    const model = models.find((item) => `${item.providerID}/${item.id}` === input.model)
    if (!model) throw new Error("Model is not available")
    await client.session.switchModel({ sessionID, model: { providerID: model.providerID, id: model.id } })
    modelsByDir.delete(dir)
    await modelState(id, await client.session.get({ sessionID }), dir)
  } else if (kind === "variant") {
    const { models, fallbackModel } = await cachedModels(dir)
    const session = await client.session.get({ sessionID })
    const ref = session.model || (fallbackModel && { providerID: fallbackModel.providerID, id: fallbackModel.id })
    const model = models.find((item) => item.providerID === ref?.providerID && item.id === ref?.id)
    if (!variants(model).includes(input.variant)) throw new Error("Variant is not available")
    await client.session.switchModel({ sessionID, model: { ...ref, ...(input.variant !== "default" && { variant: input.variant }) } })
    await modelState(id, await client.session.get({ sessionID }), dir)
  } else if (kind === "rename") {
    await client.session.update({ sessionID, title: input.title })
  } else if (kind === "permission") {
    await client.permission.reply({ sessionID, requestID: input.request, decision: input.decision })
    await pending(id, sessionID)
  } else if (kind === "form") {
    await client.session.form.reply({ sessionID, formID: input.form, answer: input.answer })
    await pending(id, sessionID)
  }
}

/** Route live server events by session, then reconcile the final authoritative history. */
async function events(onConnected) {
  while (true) {
    try {
      for await (const event of client.event.subscribe()) {
        onConnected()
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
        const changed = update(rows, event)
        if (changed) emit(id, "live", changed)
        if (event.type === "session.execution.started") emit(id, "busy", true)
        if (["session.execution.succeeded", "session.execution.failed", "session.execution.interrupted", "session.idle"].includes(event.type)) {
          emit(id, "busy", false)
          history(id, sessionID).catch((error) => emit(id, "error", message(error)))
        }
        if (event.type === "session.model.selected") modelState(id, await client.session.get({ sessionID }), folders.get(id) || directory).catch((error) => emit(id, "error", message(error)))
        if (["permission.asked", "permission.replied", "form.created", "form.replied", "form.cancelled"].includes(event.type)) {
          pending(id, sessionID).catch((error) => emit(id, "error", message(error)))
        }
      }
    } catch (error) {
      emit(-1, "error", `OpenCode event stream: ${message(error)}`)
    }
    // Streams are live-only: resubscribe and reload after any gap.
    await new Promise((resolve) => setTimeout(resolve, 1000))
    for (const [id, sessionID] of sessions) {
      history(id, sessionID).catch((error) => emit(id, "error", message(error)))
      pending(id, sessionID).catch((error) => emit(id, "error", message(error)))
      client.session.active().then((active) => emit(id, "busy", Boolean(active[sessionID]))).catch(() => {})
    }
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
