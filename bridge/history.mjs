// Project server-ordered messages into small, stable virtual-list rows.

/** Only server millisecond timestamps participate in duration calculations. */
export function duration(start, end) {
  return Number.isFinite(start) && Number.isFinite(end) && start > 0 && end >= start
    ? `${((end - start) / 1000).toFixed(1)}s` : ""
}

/** Keep identity separate from display text and preserve the server part ordinal. */
function row(message, suffix, role, body, extra = {}) {
  return { id: suffix ? `${message.id}/${suffix}` : message.id, message: message.id, role, body, ...extra }
}

/** Summarize useful tool arguments without serializing the entire output. */
export function summary(part) {
  const input = part.state?.input
  const value = typeof input === "string" ? input : input?.command ?? input?.path ?? input?.filePath ?? input?.query ?? input?.pattern ?? JSON.stringify(input || {})
  return `${part.name || "Tool"}  ${String(value).replace(/\s+/g, " ").slice(0, 180)}`
}

/** Project one message; hidden reasoning never crosses the UI boundary. */
export function project(message, showReasoning = false, turnStart) {
  if (message.type === "user") return [
    row(message, "", "user", message.text || ""),
    ...(message.files || []).map((file, index) => row(message, `file/${index}`, "file", `▧ ${file.name || file.source?.path || "Attachment"} · ${file.mime || "file"}`, { part: String(index) })),
  ]
  if (message.type === "assistant") {
    const rows = (message.content || []).flatMap((part, ordinal) => {
      if (!part) return []
      const key = part.id || `${part.type}/${ordinal}`
      if (part.type === "text") return [row(message, key, "assistant", part.text || "")]
      if (part.type === "reasoning") return showReasoning
        ? [row(message, key, "reasoning", part.text || "Thinking…")]
        : part.time && !part.time.completed ? [row(message, key, "status", "Thinking…")] : []
      if (part.type === "tool") return [row(message, key, "tool", summary(part), {
        part: part.id, status: part.state?.status || "streaming",
        elapsed: duration(part.time?.ran ?? part.time?.created, part.time?.completed),
      })]
      return []
    })
    if (message.error) rows.push(row(message, "error", "error", message.error.message || "Response failed"))
    if (message.retry) rows.push(row(message, "retry", "status", `Retry ${message.retry.attempt}: ${message.retry.error?.message || "Waiting"}`))
    rows.push(row(message, "meta", "meta", [message.agent, message.model && `${message.model.providerID}/${message.model.id}`,
      message.finish !== "tool-calls" && duration(turnStart === undefined ? message.time?.created : turnStart, message.time?.completed)].filter(Boolean).join(" · ")))
    return rows
  }
  if (message.type === "idle") return message.outcome === "succeeded" ? [] : [row(message, "", "status", message.outcome === "interrupted" ? "Interrupted" : "Execution failed")]
  if (message.type === "compaction") return [row(message, "", "status", `Compaction · ${message.status || "completed"}${message.error ? `: ${message.error.message}` : ""}`)]
  if (message.type === "shell") return [row(message, "", "status", `$ ${message.command}\n${message.output || ""}`)]
  if (message.type === "agent-switched") return [row(message, "", "status", `Agent · ${message.agent}`)]
  if (message.type === "model-switched") return [row(message, "", "status", `Model · ${message.model?.id || ""}`)]
  return message.text ? [row(message, "", "status", message.text)] : []
}

/** Keep message and content arrays in server order, including interruption dividers. */
export function conversation(messages, showReasoning = false) {
  let start
  return messages.flatMap((message) => {
    if (message.type === "user") start = undefined
    if (message.type === "assistant" && start === undefined) start = message.time?.created ?? null
    const rows = project(message, showReasoning, start)
    if (message.type === "idle") start = undefined
    return rows
  })
}

/** Serialize expanded details only on demand. */
export function details(message, partID) {
  const part = message?.content?.find((part) => part?.id === partID)
  return part ? JSON.stringify({ input: part.state?.input, output: part.state?.content, metadata: part.state?.metadata,
    error: part.state?.error, time: part.time }, null, 2) : "Tool details unavailable"
}

/** Variants are generic model capabilities, not assumed reasoning levels. */
export function variants(model) {
  return model ? ["default", ...(model.variants || []).map((variant) => variant.id).filter((id) => id !== "default")] : []
}
