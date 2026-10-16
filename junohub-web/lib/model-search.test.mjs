import assert from "node:assert/strict"
import { test } from "node:test"
import { matchesModel } from "./model-search.ts"

test("model search accepts partial, case-insensitive and separated keywords", () => {
  for (const query of ["", "GPT", "gpt6", "gpt 6", "astra gpt", " GPT-6 "]) {
    assert.ok(matchesModel("vendor/gpt-6-astra", query), query)
  }
  assert.ok(matchesModel("claude-opus-4-8", "opus4.8"))
  assert.equal(matchesModel("gpt-6-astra", "claude"), false)
  assert.equal(matchesModel("gpt-6-astra", "gpt pro"), false)
})
