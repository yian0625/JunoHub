import { modelInterface } from "./model-api"

export const modelTypeLabels: Record<string, string> = { chat: "对话", image: "图片", video: "视频", audio: "语音", embedding: "向量化", search: "搜索", rerank: "重排" }
export function modelType(type?: string) { return modelInterface(type).capability }
export function modelEndpoint(type?: string, endpoint?: string) { return endpoint || modelInterface(type).endpoint || "/v1/chat/completions" }
export function modelPayload(alias: string, type: string): Record<string, unknown> | null {
  switch (type) {
    case "auto": case "chat_completions": return { model: alias, messages: [{ role: "user", content: "你好" }] }
    case "responses": return { model: alias, input: "你好" }
    case "messages": return { model: alias, max_tokens: 256, messages: [{ role: "user", content: "你好" }] }
    case "embeddings": return { model: alias, input: "需要向量化的文本" }
    // Extension paths do not imply a universal vendor request schema.
    default: return null
  }
}
export type ExampleLanguage = "curl" | "python" | "javascript"
const shellQuote = (value: string) => "'" + value.replaceAll("'", "'\\''") + "'"
export function modelExample(alias: string, type: string, endpoint: string, language: ExampleLanguage, origin = ""): string {
  const body = modelPayload(alias, type)
  const address = `${origin || "https://your-junohub.example"}${endpoint}`
  if (language === "curl") {
    const setup = body ? "" : "# 将供应商要求的完整 JSON 请求体保存为 request.json，其中 model 填写：\n# " + JSON.stringify(alias) + "\n"
    return setup + ["curl --request POST " + shellQuote(address), '  --header "Authorization: Bearer $JUNOHUB_API_KEY"', '  --header "Content-Type: application/json"', body ? "  --data " + shellQuote(JSON.stringify(body, null, 2)) : "  --data-binary @request.json"].join(" \\\n")
  }
  if (language === "python") {
    const payload = body ? "payload = " + JSON.stringify(body, null, 2) : '# request.json 使用供应商要求的 JSON 字段，model 填写 ' + JSON.stringify(alias) + '\nwith open("request.json", encoding="utf-8") as file:\n    payload = json.load(file)'
    return 'import json\nimport os\nimport urllib.request\n\n' + payload + '\nrequest = urllib.request.Request(\n    ' + JSON.stringify(address) + ',\n    data=json.dumps(payload).encode("utf-8"),\n    headers={"Authorization": "Bearer " + os.environ["JUNOHUB_API_KEY"],\n             "Content-Type": "application/json"},\n    method="POST",\n)\nwith urllib.request.urlopen(request) as response:\n    print(response.status)\n    result = response.read()\n    if "json" in response.headers.get("Content-Type", ""):\n        print(result.decode("utf-8"))\n    else:\n        with open("output.bin", "wb") as file:\n            file.write(result)\n        print("已保存到 output.bin")'
  }
  const payload = body ? "const payload = " + JSON.stringify(body, null, 2) + ";" : '// request.json 使用供应商要求的 JSON 字段，model 填写 ' + JSON.stringify(alias) + '\nconst payload = JSON.parse(await readFile("request.json", "utf8"));'
  return (body ? 'import { writeFile } from "node:fs/promises";\n\n' : 'import { readFile, writeFile } from "node:fs/promises";\n\n') + payload + '\nconst response = await fetch(' + JSON.stringify(address) + ', {\n  method: "POST",\n  headers: {\n    Authorization: `Bearer ${process.env.JUNOHUB_API_KEY}`,\n    "Content-Type": "application/json",\n  },\n  body: JSON.stringify(payload),\n});\nif (!response.ok) throw new Error(`HTTP ${response.status}: ${await response.text()}`);\nconsole.log(response.status);\nconst result = await response.arrayBuffer();\nif ((response.headers.get("content-type") || "").includes("json")) {\n  console.log(new TextDecoder().decode(result));\n} else {\n  await writeFile("output.bin", new Uint8Array(result));\n  console.log("已保存到 output.bin");\n}'
}

export function modelUploadExample(alias: string, type: string, endpoint: string, language: ExampleLanguage, origin = ""): string {
  const image = type === "images_edits"
  const field = image ? "image[]" : "file"
  const filename = image ? "image.png" : "audio.mp3"
  const mime = image ? "image/png" : "audio/mpeg"
  const address = `${origin || "https://your-junohub.example"}${endpoint}`
  const note = "# 示例使用常见兼容字段；额外参数及文件字段名请按该模型文档调整。\n"
  if (language === "curl") return note + [
    "curl --request POST " + shellQuote(address),
    '  --header "Authorization: Bearer $JUNOHUB_API_KEY"',
    "  --form-string " + shellQuote("model=" + alias),
    "  --form " + shellQuote(`${field}=@${filename};type=${mime}`),
  ].join(" \\\n")
  if (language === "python") return note + 'import os\nimport requests\n\nwith open(' + JSON.stringify(filename) + ', "rb") as file:\n    response = requests.post(\n        ' + JSON.stringify(address) + ',\n        headers={"Authorization": "Bearer " + os.environ["JUNOHUB_API_KEY"]},\n        data={"model": ' + JSON.stringify(alias) + '},\n        files={' + JSON.stringify(field) + ': (' + JSON.stringify(filename) + ', file, ' + JSON.stringify(mime) + ')},\n        timeout=(10, 180),\n    )\nresponse.raise_for_status()\nif "json" in response.headers.get("Content-Type", ""):\n    print(response.json())\nelse:\n    with open("output.bin", "wb") as file:\n        file.write(response.content)\n    print("已保存到 output.bin")'
  return '// 示例使用常见兼容字段；额外参数及文件字段名请按该模型文档调整。\nimport { readFile, writeFile } from "node:fs/promises";\n\nconst form = new FormData();\nform.append("model", ' + JSON.stringify(alias) + ');\nform.append(' + JSON.stringify(field) + ', new Blob([await readFile(' + JSON.stringify(filename) + ')], { type: ' + JSON.stringify(mime) + ' }), ' + JSON.stringify(filename) + ');\nconst response = await fetch(' + JSON.stringify(address) + ', {\n  method: "POST",\n  headers: { Authorization: `Bearer ${process.env.JUNOHUB_API_KEY}` },\n  body: form,\n});\nif (!response.ok) throw new Error(`HTTP ${response.status}: ${await response.text()}`);\nif ((response.headers.get("content-type") || "").includes("json")) {\n  console.log(await response.json());\n} else {\n  await writeFile("output.bin", new Uint8Array(await response.arrayBuffer()));\n  console.log("已保存到 output.bin");\n}'
}
