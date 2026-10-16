import assert from "node:assert/strict"
import { test } from "node:test"
import { parseOAuthResult } from "./oauth-result.ts"

test("OAuth callback formats and state binding", () => {
  assert.deepEqual(parseOAuthResult(" https://localhost/callback?code=abc&state=current ", "current"), { code: "abc", state: "current", error: "" })
  assert.equal(parseOAuthResult("abc#current", "current").code, "abc")
  assert.equal(parseOAuthResult("abc", "current").code, "abc")
  for (const value of ["https://localhost/callback?code=abc", "https://localhost/callback?code=abc&state=old", "abc#old", "https://localhost/callback?error=access_denied&state=current"]) {
    const parsed = parseOAuthResult(value, "current")
    assert.equal(parsed.code, "")
    assert.ok(parsed.error)
  }
})

test("manual localhost callbacks work without a callback listener", () => {
  for (const origin of ["http://localhost:1455", "http://localhost:8085", "http://127.0.0.1:56121"]) {
    assert.deepEqual(parseOAuthResult(`${origin}/callback?state=current&code=a%2Bb%2Fc%3D`, "current"), {
      code: "a+b/c=", state: "current", error: "",
    })
  }
})

test("incomplete and ambiguous callbacks cannot become authorization codes", () => {
  for (const value of [
    "http://localhost:8085/callback?state=current",
    "http://localhost:8085/callback?code=%20&state=current",
    "http://localhost:8085/callback?code=a&code=b&state=current",
    "http://localhost:8085/callback?code=a&state=current&state=old",
    "http://[invalid",
    "http:/localhost/callback?code=a&state=current",
    "custom://callback?code=a&state=current",
    "#current",
  ]) {
    const parsed = parseOAuthResult(value, "current")
    assert.equal(parsed.code, "", value)
    assert.ok(parsed.error, value)
  }
})
