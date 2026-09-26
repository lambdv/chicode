// Track live assistant rows by server message ID without exposing reasoning content.

/** Seed activity IDs from authoritative projected history. */
export function seed(messages, rows) {
  const live = new Map()
  for (const message of messages) {
    const row = rows.find((item) => item.id === message.id && item.role !== "user")
    if (!row) continue
    const parts = message.content || []
    live.set(message.id, {
      row,
      tools: new Set(parts.filter((part) => part.type === "tool").map((part) => part.id)),
      reasoning: new Set(parts.flatMap((part, index) => part.type === "reasoning" && part.time && !part.time.completed ? [index] : [])),
    })
  }
  return live
}

/** Apply one streamed event and return the changed row, if any. */
export function update(live, event) {
  const data = event.data
  const id = data?.assistantMessageID
  if (!id) return null
  let current = live.get(id)
  if (!current) {
    current = { row: { id, role: "assistant", body: "", model: "", tools: 0, thinking: false }, tools: new Set(), reasoning: new Set() }
    live.set(id, current)
  }
  const row = current.row
  switch (event.type) {
    case "session.step.started":
      row.model = data.model ? `${data.model.providerID}/${data.model.id}` : ""
      break
    case "session.text.delta":
      row.body += data.delta
      break
    case "session.reasoning.started":
      current.reasoning.add(data.ordinal)
      row.thinking = true
      break
    case "session.reasoning.ended":
      current.reasoning.delete(data.ordinal)
      row.thinking = current.reasoning.size > 0
      break
    case "session.tool.input.started":
    case "session.tool.called":
      current.tools.add(data.id)
      row.tools = current.tools.size
      break
    case "session.step.failed":
      row.role = "error"
      row.body = data.error?.message || "Agent failed"
    // fall through to clear the thinking state.
    case "session.step.ended":
      current.reasoning.clear()
      row.thinking = false
      break
    default:
      return null
  }
  return { ...row }
}
