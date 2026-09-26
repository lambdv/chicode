import { test } from "node:test"
import { deepStrictEqual } from "node:assert"
import { conversation, variants } from "./history.mjs"
import { seed, update } from "./live.mjs"

// History retains model and tool counts without exposing reasoning text.
test("projects chat history", () => {
  deepStrictEqual(conversation([
    { id: "user-1", type: "user", text: "Hi" },
    { id: "reply-1", type: "assistant", model: { providerID: "openai", id: "gpt-6" }, content: [{ type: "reasoning", text: "thought" }, { type: "text", text: "Hello" }, { type: "tool", id: "tool-1", name: "read" }] },
    { id: "reply-2", type: "assistant", content: [], error: { message: "Model unavailable" } },
    { type: "idle" },
  ]), [
    { id: "user-1", role: "user", body: "Hi", model: "", tools: 0, thinking: false },
    { id: "reply-1", role: "assistant", body: "Hello", model: "openai/gpt-6", tools: 1, thinking: false },
    { id: "reply-2", role: "error", body: "Model unavailable", model: "", tools: 0, thinking: false },
  ])
})

// Live events update only the matching assistant and count each tool once.
test("tracks live reasoning, text, and tool calls", () => {
  const live = seed([], [])
  const event = (type, data) => update(live, { type, data: { assistantMessageID: "reply-1", ...data } })
  deepStrictEqual(event("session.step.started", { model: { providerID: "openai", id: "gpt-6" } }),
    { id: "reply-1", role: "assistant", body: "", model: "openai/gpt-6", tools: 0, thinking: false })
  deepStrictEqual(event("session.reasoning.started", { ordinal: 0 }).thinking, true)
  deepStrictEqual(event("session.tool.input.started", { id: "tool-1" }).tools, 1)
  deepStrictEqual(event("session.tool.called", { id: "tool-1" }).tools, 1)
  deepStrictEqual(event("session.text.delta", { delta: "Hello" }).body, "Hello")
  deepStrictEqual(event("session.reasoning.ended", { ordinal: 0 }).thinking, false)
  deepStrictEqual(event("session.step.ended", {}).model, "openai/gpt-6")
})

// Tool-only steps remain visible in restored history and keep their call IDs.
test("restores tool-only steps and unfinished reasoning", () => {
  const messages = [{
    id: "reply-2", type: "assistant", model: { providerID: "test", id: "model" },
    content: [
      { type: "reasoning", text: "private", time: { created: 1 } },
      { type: "tool", id: "call-1", name: "read" },
    ],
  }]
  const rows = conversation(messages)
  deepStrictEqual(rows, [{ id: "reply-2", role: "assistant", body: "", model: "test/model", tools: 1, thinking: true }])
  const live = seed(messages, rows)
  deepStrictEqual(update(live, { type: "session.tool.called", data: { assistantMessageID: "reply-2", id: "call-1" } }).tools, 1)
  deepStrictEqual(update(live, { type: "session.reasoning.ended", data: { assistantMessageID: "reply-2", ordinal: 0 } }).thinking, false)
})

// Selecting default removes an explicit reasoning variant.
test("lists available variants", () => {
  deepStrictEqual(variants({ variants: [{ id: "high" }, { id: "low" }] }), ["default", "high", "low"])
  deepStrictEqual(variants(null), [])
})
