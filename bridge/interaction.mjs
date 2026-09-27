// Composer catalogs, native V2 submission, and pending form/inbox snapshots.
import { pathToFileURL } from "node:url"
import { resolve } from "node:path"
const pendingVersion = new Map()

/** Fetch advertised commands and agents for this location. */
export async function catalog(client, emit, id, directory) {
  const location = { directory }
  const [commands, agents] = await Promise.all([client.command.list({ location }), client.agent.list({ location })])
  emit(id, "commands", commands.data)
  const primary = agents.data.filter((agent) => !agent.hidden && agent.mode !== "subagent").map((agent) => ({ id: agent.id, name: agent.name }))
  emit(id, "agents", primary)
  return primary
}

/** Search references through the server, never by guessing resource contents. */
export async function suggest(client, emit, id, directory, query) {
  const location = { directory }
  const results = await Promise.allSettled([
    client.file.find({ location, query, limit: 20, type: "file" }),
    client.agent.list({ location }), client.reference.list({ location }), client.mcp.resource.catalog({ location }),
  ])
  const data = results.map((result) => result.status === "fulfilled" ? result.value.data : [])
  const items = [
    ...(data[0] || []).map((file) => ({ label: file.path, kind: "file", value: pathToFileURL(resolve(directory, file.path)).href })),
    ...(data[1] || []).filter((agent) => !agent.hidden).map((agent) => ({ label: agent.name, kind: "agent", value: agent.name })),
    ...(data[2] || []).filter((ref) => !ref.hidden).map((ref) => ({ label: ref.name, kind: "file", value: pathToFileURL(resolve(directory, ref.path)).href })),
    ...(data[3]?.resources || []).map((resource) => ({ label: resource.name, kind: "file", value: resource.uri })),
  ].filter((item) => item.value && item.label?.toLowerCase().includes(query.toLowerCase())).slice(0, 30)
  emit(id, "suggestions", { query, items })
}

/** Dispatch slash commands via their API and ordinary text via durable prompt admission. */
export async function submit(client, sessionID, input, directory) {
  const common = { sessionID, files: input.files || [], agents: input.agents || [], ...(input.delivery && { delivery: input.delivery }) }
  const match = input.text.match(/^\/([^\s]+)(?:\s+([\s\S]*))?$/)
  if (match) {
    const commands = await client.command.list({ location: { directory } })
    if (!commands.data.some((command) => command.name === match[1])) throw new Error(`Unknown command /${match[1]}`)
    await client.session.command({ ...common, name: match[1], text: match[2] || "" })
  } else await client.session.prompt({ ...common, id: input.messageID, text: input.text })
}

/** Restore pending questions and durable follow-ups after opening or reconnecting. */
export async function pendingRequests(client, emit, id, sessionID) {
  const version = (pendingVersion.get(id) || 0) + 1
  pendingVersion.set(id, version)
  const [forms, inbox] = await Promise.all([client.session.form.list({ sessionID }), client.session.inbox.list({ sessionID })])
  if (pendingVersion.get(id) !== version) return
  emit(id, "question", forms[0] || null)
  emit(id, "inbox", inbox.map((item) => ({ id: item.id, text: item.payload.text || item.type })))
  return inbox
}
