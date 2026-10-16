import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import { test } from "node:test"
import ts from "typescript"
function load(name) {
  const output = ts.transpileModule(readFileSync(new URL(`./${name}.ts`, import.meta.url), "utf8"), {compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2022}}).outputText
  const cjsModule = {exports:{}}
  new Function("require","module","exports",output)(path => {assert.equal(path,"./model-api");return load("model-api")},cjsModule,cjsModule.exports)
  return cjsModule.exports
}
const {modelExample, modelUploadExample, modelPayload, modelEndpoint} = load("model-access")
test("dialogue and embedding examples use the selected protocol and exact alias", () => {
  assert.equal(modelPayload("embedding-demo","embeddings").input,"需要向量化的文本")
  assert.equal(modelPayload("chat-demo","responses").input,"你好")
  assert.equal(modelPayload("claude-demo","messages").max_tokens,256)
  assert.equal(modelEndpoint("auto"),"/v1/chat/completions")
  for(const language of ["curl","python","javascript"]) {
    const code=modelExample("chat-demo","responses","/v1/responses",language,"https://demo.test")
    assert.ok(code.includes("https://demo.test/v1/responses"))
    assert.ok(code.includes("JUNOHUB_API_KEY"))
    assert.ok(code.includes("chat-demo"))
    assert.ok(!code.includes("/v1/v1/"))
  }
})
test("extension examples do not invent supplier request fields", () => {
  for(const type of ["images_generations","images_edits","videos_generations","audio_speech","audio_transcriptions","audio_generations","search","rerank"]) {
    assert.equal(modelPayload("demo",type),null)
    for(const language of ["curl","python","javascript"]) {
      const code=modelExample("demo",type,modelEndpoint(type),language)
      assert.ok(code.includes("request.json"))
      assert.ok(!code.includes('"prompt":'))
      assert.ok(!code.includes('"audio_url":'))
    }
  }
})
test("shell examples quote model names and origin as data", () => {
  const alias="demo'$(touch /tmp/never-run)`echo no`"
  const code=modelExample(alias,"chat_completions","/v1/chat/completions","curl","https://demo.test")
  assert.ok(code.includes("demo'\\''$(touch /tmp/never-run)`echo no`"))
  assert.ok(code.startsWith("curl --request POST 'https://demo.test/v1/chat/completions'"))
})

test("file examples preserve alias and let clients generate multipart boundaries", () => {
  for (const [type,endpoint,field,file] of [["images_edits","/v1/images/edits","image[]","image.png"],["audio_transcriptions","/v1/audio/transcriptions","file","audio.mp3"]]) {
    for (const language of ["curl","python","javascript"]) {
      const code=modelUploadExample("file-alias",type,endpoint,language,"https://demo.test")
      for (const value of ["https://demo.test"+endpoint,"file-alias","JUNOHUB_API_KEY",field,file]) assert.ok(code.includes(value))
      assert.ok(!code.includes("Content-Type"+": multipart"))
      assert.ok(!code.includes("boundary="))
      assert.ok(!code.includes("request.json"))
      if (language === "curl") assert.ok(code.includes("--form-string 'model=file-alias'"))
      if (language === "javascript") { assert.ok(code.includes("new FormData")); assert.ok(!code.includes('"Content-Type":')) }
      if (language === "python") { assert.ok(code.includes('files={')); assert.ok(code.includes('response.raise_for_status()')) }
    }
  }
  const code=modelUploadExample("file'$(touch /tmp/never-run)","images_edits","/v1/images/edits","curl","https://demo.test")
  assert.ok(code.includes("model=file'\\''$(touch /tmp/never-run)"))
})
