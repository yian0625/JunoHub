// Match partial keywords regardless of case or model-name separators.
export function matchesModel(model: string, query: string) {
  const normalize = (value: string) => value.normalize("NFKC").toLocaleLowerCase().replace(/[^\p{L}\p{N}]/gu, "")
  const normalizedModel = normalize(model)
  return query.trim().split(/\s+/).every(word => normalizedModel.includes(normalize(word)))
}
