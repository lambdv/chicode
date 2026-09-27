import { test } from "node:test"
import { deepStrictEqual as equal, strictEqual as same, ok } from "node:assert"
import { conversation, project, variants, duration, details } from "./history.mjs"
import { seed, update } from "./live.mjs"
import { catalog, submit, pendingRequests } from "./interaction.mjs"
import { selectModel, modelsByDir } from "./models.mjs"

/** Build an installed-client V2 event with explicit server timestamps. */
function event(type, data, created = 1200) {
  return { type: `session.${type}`, created, data: { assistantMessageID: "msg_a", ...data } }
}

test("history preserves message and interleaved part order, attachments and status rows", () => {
  const rows = conversation([
    { id: "msg_u", type: "user", text: "Hi", files: [{ name: "image.png", mime: "image/png" }], time: { created: 1000 } },
    { id: "msg_a", type: "assistant", agent: "build", model: { providerID: "test", id: "model" }, time: { created: 1100, completed: 2000 }, content: [
      { type: "text", text: "Before" }, { type: "tool", id: "call_a", name: "read", state: { status: "completed", input: { path: "file.rs" } } },
      { type: "text", text: "After" }, { type: "reasoning", text: "private", time: { created: 1000, completed: 2000 } },
    ] },
    { id: "msg_c", type: "compaction", status: "completed" },
    { id: "msg_i", type: "idle", outcome: "interrupted" },
  ])
  equal(rows.map((row) => row.role), ["user", "file", "assistant", "tool", "assistant", "meta", "status", "status"])
  same(rows[3].id, "msg_a/call_a")
  same(rows[5].body, "build · test/model · 0.9s")
  ok(!JSON.stringify(rows).includes("private"))
  same(rows.at(-1).body, "Interrupted")
})

test("text completion replaces deltas and reasoning is gated at projection", () => {
  const state = seed([])
  update(state, event("step.started", { started: 1000, agent: "build", model: { providerID: "test", id: "model" } }))
  update(state, event("reasoning.started", { ordinal: 0 }))
  update(state, event("reasoning.delta", { ordinal: 0, delta: "secret" }))
  const hidden = project(state.get("msg_a"))
  ok(hidden.some((row) => row.body === "Thinking…"))
  ok(!JSON.stringify(hidden).includes("secret"))
  ok(project(state.get("msg_a"), true).some((row) => row.body === "secret"))
  update(state, event("reasoning.ended", { ordinal: 0, text: "secret final" }))
  update(state, event("text.started", { ordinal: 1 }))
  update(state, event("text.delta", { ordinal: 1, delta: "Hello" }))
  update(state, event("text.ended", { ordinal: 1, text: "Hello world" }))
  same(state.get("msg_a").content[1].text, "Hello world")
  ok(!project(state.get("msg_a")).some((row) => row.body === "Thinking…"))
})

test("tool input, progress, success and failures retain one call ID", () => {
  const state = seed([])
  update(state, event("tool.input.started", { id: "call_a", name: "shell" }, 1000))
  update(state, event("tool.input.delta", { id: "call_a", delta: '{"command":' }))
  same(state.get("msg_a").content[0].state.status, "streaming")
  update(state, event("tool.input.ended", { id: "call_a", text: '{"command":"ls"}' }))
  update(state, event("tool.called", { id: "call_a", input: { command: "ls" } }, 1500))
  update(state, event("tool.progress", { id: "call_a", content: [{ type: "text", text: "a" }], metadata: { diff: "patch" } }))
  update(state, event("tool.success", { id: "call_a", content: [{ type: "text", text: "a\nb" }] }, 2500))
  const rows = project(state.get("msg_a"))
  same(rows.filter((row) => row.role === "tool").length, 1)
  same(rows[0].id, "msg_a/call_a")
  same(rows[0].elapsed, "1.0s")
  same(rows[0].status, "completed")
  ok(details(state.get("msg_a"), "call_a").includes("a\\nb"))
  update(state, event("tool.failed", { id: "call_a", error: { message: "failed" } }, 3000))
  same(project(state.get("msg_a"))[0].status, "error")
})

test("resume from history updates the existing call without changing order", () => {
  const state = seed([{ id: "msg_a", type: "assistant", time: { created: 1 }, content: [
    { type: "text", text: "Before" }, { type: "tool", id: "call_a", name: "read", state: { status: "running", input: {} }, time: { created: 1 } },
  ] }])
  update(state, event("tool.success", { id: "call_a", content: [{ type: "text", text: "done" }] }))
  same(state.get("msg_a").content.length, 2)
  same(state.get("msg_a").content[1].state.status, "completed")
})

test("invalid times and unsupported variant controls remain unavailable", () => {
  for (const [start, end] of [[null, 10], [1, undefined], [20, 10], [0, 1], [NaN, 1], [1, Infinity], ["1", 2]]) same(duration(start, end), "")
  equal(variants({ variants: [] }), ["default"])
  equal(variants({ variants: [{ id: "high" }] }), ["default", "high"])
  equal(variants(null), [])
})

test("commands use command API and followups retain delivery and stable retry ID", async () => {
  const calls = []
  const client = { command: { list: async () => ({ data: [{ name: "review" }] }) }, session: {
    command: async (input) => calls.push(["command", input]), prompt: async (input) => calls.push(["prompt", input]),
  } }
  await submit(client, "ses_1", { text: "/review branch one", delivery: "queue" }, ".")
  same(calls[0][0], "command"); same(calls[0][1].name, "review"); same(calls[0][1].text, "branch one")
  await submit(client, "ses_1", { text: "follow up", delivery: "steer", messageID: "msg_retry" }, ".")
  await submit(client, "ses_1", { text: "follow up", delivery: "steer", messageID: "msg_retry" }, ".")
  equal(calls[1], calls[2]); same(calls[1][1].id, "msg_retry")
  await import("node:assert/strict").then(({ rejects }) => rejects(submit(client, "ses_1", { text: "/unknown" }, "."), /Unknown command/))
  same(calls.length, 3)
})

test("pending durable inputs and questions are restored from server state", async () => {
  const emitted = []
  const form = { id: "form_1", fields: [{ key: "q", type: "string", options: [] }] }
  const client = { session: { form: { list: async () => [form] }, inbox: { list: async () => [{ id: "inbox_1", payload: { text: "later" }, delivery: "queue" }] } } }
  await pendingRequests(client, (...args) => emitted.push(args), 1, "ses_1")
  equal(emitted.find((item) => item[1] === "question")[2], form)
  equal(emitted.find((item) => item[1] === "inbox")[2], [{ id: "inbox_1", text: "later" }])
})

test("agent choices display names but return server IDs", async () => {
  const emitted = []
  const choices = await catalog({
    command: { list: async () => ({ data: [] }) },
    agent: { list: async () => ({ data: [
      { id: "build", name: "Build", mode: "primary", hidden: false },
      { id: "plan", name: "Plan", mode: "primary", hidden: false },
      { id: "explore", name: "Explore", mode: "subagent", hidden: false },
    ] }) },
  }, (...args) => emitted.push(args), 1, ".")
  equal(choices, [{ id: "build", name: "Build" }, { id: "plan", name: "Plan" }])
  equal(emitted.find((item) => item[1] === "agents")[2], choices)
})

test("admission is pending until delivery, which keeps its ID and transcript order", () => {
  const state = seed([])
  equal(update(state, { type: "session.inbox.enqueued", id: "event_1", created: 1000, data: {
    inboxID: "msg_user", item: { type: "user", delivery: "queue", payload: { text: "Later", files: [] } },
  } }), [])
  same(state.size, 0)
  equal(update(state, { type: "session.inbox.delivered", id: "event_2", created: 2000, data: { inboxID: "msg_user" } }), ["msg_user"])
  update(state, event("step.started", { started: 2001 }))
  equal([...state.keys()], ["msg_user", "msg_a"])
  same(state.inbox.size, 0)
  same(project(state.get("msg_user"))[0].id, "msg_user")
})

test("repeated event IDs do not append text twice and content replacement retains message ID", () => {
  const state = seed([])
  const delta = { ...event("text.delta", { ordinal: 0, delta: "hello" }), id: "event_delta" }
  update(state, delta); update(state, delta)
  same(state.get("msg_a").content[0].text, "hello")
  update(state, { type: "session.message.content.updated", id: "event_replace", data: { messageID: "msg_a", content: [{ type: "text", text: "updated" }] } })
  same(state.size, 1)
  same(project(state.get("msg_a"))[0].body, "updated")
})

test("selecting the default variant removes the previous override", async () => {
  modelsByDir.clear()
  let selected
  const model = { id: "model", providerID: "test", enabled: true, variants: [{ id: "high" }], capabilities: { input: ["text"] } }
  const client = {
    model: { list: async () => ({ data: [model] }), default: async () => ({ data: model }) },
    session: { get: async () => ({ model: { id: "model", providerID: "test", variant: "high" } }), switchModel: async (input) => { selected = input.model } },
  }
  await selectModel(client, () => {}, { id: 1, kind: "variant", variant: "default" }, ".", "ses_1")
  equal(selected, { providerID: "test", id: "model" })
})
