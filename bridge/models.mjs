// Cache advertised models and apply explicit model/variant selections.
import { variants } from "./history.mjs"

export const modelsByDir = new Map()

/** Reuse the location's advertised model list across windows. */
export async function cachedModels(client, directory) {
  if (modelsByDir.has(directory)) return modelsByDir.get(directory)
  const location = { directory }
  const task = (async () => {
    const [listed, fallback] = await Promise.all([client.model.list({ location }), client.model.default({ location })])
    const models = listed.data.filter((model) => model.enabled)
    const fallbackModel = fallback.data
    if (fallbackModel && !models.some((model) => model.providerID === fallbackModel.providerID && model.id === fallbackModel.id)) models.push(fallbackModel)
    return { models, fallbackModel }
  })()
  modelsByDir.set(directory, task)
  try { return await task } catch (error) { modelsByDir.delete(directory); throw error }
}

/** Publish only capabilities and variants belonging to the selected model. */
export async function modelState(client, emit, id, session, directory) {
  const { models, fallbackModel } = await cachedModels(client, directory)
  const ref = session.model || fallbackModel
  const current = models.find((model) => model.providerID === ref?.providerID && model.id === ref?.id)
  emit(id, "models", models.map((model) => `${model.providerID}/${model.id}`))
  emit(id, "model", ref ? `${ref.providerID}/${ref.id}` : "")
  emit(id, "variants", variants(current))
  emit(id, "variant", session.model?.variant || "default")
  emit(id, "capabilities", current?.capabilities?.input || [])
  emit(id, "agent", session.agent || "")
}

/** Serialize model changes before prompt admission; default removes variant overrides. */
export async function selectModel(client, emit, input, directory, sessionID, draft) {
  const { models, fallbackModel } = await cachedModels(client, directory)
  const session = draft || await client.session.get({ sessionID })
  const ref = session.model || fallbackModel
  const model = input.kind === "model"
    ? models.find((item) => `${item.providerID}/${item.id}` === input.model)
    : models.find((item) => item.providerID === ref?.providerID && item.id === ref?.id)
  if (!model) throw new Error("Model is not available")
  if (input.kind === "variant" && !variants(model).includes(input.variant)) throw new Error("Variant is not available")
  const selected = { providerID: model.providerID, id: model.id,
    ...(input.kind === "variant" && input.variant !== "default" && { variant: input.variant }) }
  if (draft) draft.model = selected
  else await client.session.switchModel({ sessionID, model: selected })
  await modelState(client, emit, input.id, { ...session, model: selected }, directory)
}
