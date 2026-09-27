// Local slash actions backed by explicit V2 session operations.

/** Execute only operations supported by the installed official client. */
export async function action(client, emit, id, sessionID, name) {
  if (name === "compact") return client.session.compact({ sessionID })
  if (name === "redo") return client.session.revert.clear({ sessionID })
  if (name === "undo") {
    await client.session.interrupt({ sessionID })
    let cursor
    const messages = []
    do {
      const page = await client.message.list({ sessionID, order: "asc", limit: 100, ...(cursor && { cursor }) })
      messages.push(...page.data); cursor = page.cursor.next
    } while (cursor)
    const last = messages.findLast((message) => message.type === "user")
    if (!last) throw new Error("No user turn to undo")
    await client.session.revert.stage({ sessionID, messageID: last.id })
    emit(id, "restore-draft", last.text)
    return
  }
  if (name === "fork") { emit(id, "forked", await client.session.fork({ sessionID })); return }
  if (name === "export") { emit(id, "export", await client.session.export({ sessionID })); return }
  throw new Error(`Unsupported local action: ${name}`)
}
