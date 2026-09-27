// Incrementally reduce the installed V2 event contract into server-owned messages.

/** Seed without changing message order or duplicating tool calls. */
export function seed(messages) {
  const state = new Map(messages.map((message) => [message.id, message]))
  state.inbox = new Map()
  state.seen = new Set()
  state.turnStarts = new Map()
  for (const message of messages) {
    if (message.type === "user") state.turnStart = undefined
    if (message.type === "assistant" && state.turnStart === undefined) state.turnStart = message.time?.created ?? null
    if (message.type === "assistant") state.turnStarts.set(message.id, state.turnStart)
    if (message.type === "idle") state.turnStart = undefined
  }
  return state
}

/** Apply one event; return changed message IDs for bounded UI updates. */
export function update(live, event) {
  const d = event.data || {}
  const type = event.type
  const timestamp = event.created
  if (event.id) {
    live.seen ||= new Set()
    if (live.seen.has(event.id)) return []
    live.seen.add(event.id)
    if (live.seen.size > 2048) live.seen.delete(live.seen.values().next().value)
  }
  live.inbox ||= new Map()
  if (type === "session.inbox.enqueued") { live.inbox.set(d.inboxID, d.item); return [] }
  if (type === "session.inbox.cancelled") { live.inbox.delete(d.inboxID); return [] }
  if (type === "session.inbox.delivered") {
    const item = live.inbox.get(d.inboxID)
    live.inbox.delete(d.inboxID)
    if (item?.type !== "user") return []
    live.set(d.inboxID, { ...item.payload, id: d.inboxID, type: "user", time: { created: timestamp } })
    live.turnStart = undefined
    return [d.inboxID]
  }
  const id = d.assistantMessageID || (type === "session.message.content.updated" && d.messageID)
  if (!id) return []
  let message = live.get(id)
  if (!message) {
    message = { id, type: "assistant", content: [], time: {} }
    live.set(id, message)
    live.turnStarts ||= new Map()
    live.turnStarts.set(id, live.turnStart)
  }
  message.time ||= {}
  const parts = message.content
  const ordinal = d.ordinal
  let part
  if (type.startsWith("session.text.") || type.startsWith("session.reasoning.")) {
    if (!Number.isInteger(ordinal) || ordinal < 0) return []
    const kind = type.split(".")[1]
    part = parts[ordinal] ||= { type: kind, text: "", ...(kind === "reasoning" && { time: { created: timestamp } }) }
    if (kind === "reasoning" && type.endsWith(".started")) part.time ||= { created: timestamp }
    if (type.endsWith(".delta")) part.text += d.delta || ""
    if (type.endsWith(".ended")) {
      part.text = d.text ?? part.text
      if (part.time) part.time.completed = timestamp
    }
  } else if (type.startsWith("session.tool.")) {
    part = parts.find((item) => item?.type === "tool" && item.id === d.id)
    if (!part) {
      part = { type: "tool", id: d.id, name: d.name || "Tool", state: { status: "streaming", input: "" }, time: { created: timestamp } }
      parts.push(part)
    }
    if (d.name) part.name = d.name
    part.time ||= {}
    switch (type) {
      case "session.tool.input.delta": part.state.input = String(part.state.input || "") + (d.delta || ""); break
      case "session.tool.input.ended": part.state.input = d.text; break
      case "session.tool.called": part.state = { status: "running", input: d.input }; part.time.ran = timestamp; break
      case "session.tool.progress": Object.assign(part.state, { ...(d.content && { content: d.content }), ...(d.metadata && { metadata: d.metadata }) }); break
      case "session.tool.success":
      case "session.tool.failed":
        Object.assign(part.state, { status: type.endsWith("failed") ? "error" : "completed", ...(d.content && { content: d.content }), ...(d.metadata && { metadata: d.metadata }), error: d.error })
        part.time.completed = timestamp
        break
    }
  } else switch (type) {
    case "session.message.content.updated": message.content = d.content; break
    case "session.step.started":
      Object.assign(message, { model: d.model, agent: d.agent })
      message.time.created = d.started ?? timestamp
      if (live.turnStart === undefined) live.turnStart = message.time.created ?? null
      live.turnStarts ||= new Map()
      live.turnStarts.set(id, live.turnStart)
      break
    case "session.step.ended": message.finish = d.finish; message.time.completed = timestamp; delete message.retry; break
    case "session.step.failed": message.error = d.error; message.time.completed = timestamp; delete message.retry; break
    case "session.retry.scheduled": message.retry = { attempt: d.attempt, error: d.error }; break
    default: return []
  }
  if (message.time.completed) for (const item of parts) {
    if (item?.type === "reasoning" && item.time) item.time.completed ??= message.time.completed
  }
  return [id]
}
