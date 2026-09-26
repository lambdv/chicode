// Convert OpenCode's projected messages to compact conversation rows.

/** Extract visible messages with model and activity metadata. */
export function conversation(messages) {
  return messages.flatMap((message) => {
    if (message.type === "user") return message.text ? [{ id: message.id, role: "user", body: message.text, model: "", tools: 0, thinking: false }] : []
    if (message.type !== "assistant") return []
    const parts = message.content || []
    const body = parts.filter((part) => part.type === "text").map((part) => part.text).join("\n")
    const tools = parts.filter((part) => part.type === "tool").length
    const thinking = parts.some((part) => part.type === "reasoning" && part.time && !part.time.completed)
    const model = message.model ? `${message.model.providerID}/${message.model.id}` : ""
    if (!body && !tools && !thinking && !message.error) return []
    return [{ id: message.id, role: message.error ? "error" : "assistant", body: message.error?.message || body, model, tools, thinking }]
  })
}

/** List selectable variants, including the model's unmodified default. */
export function variants(model) {
  return model ? ["default", ...model.variants.map((variant) => variant.id).filter((id) => id !== "default")] : []
}
