export function parseOAuthResult(value: string, expectedState: string) {
  const input = value.trim()
  let code = input
  let state = expectedState
  let error = ""
  if (!input) return { code: "", state, error }
  if (/^https?:\/\//i.test(input)) {
    try {
      const url = new URL(input)
      code = url.searchParams.get("code") || ""
      state = url.searchParams.get("state") || ""
      if (url.searchParams.has("error")) error = "授权未获批准，请重新生成授权链接。"
      else if (url.searchParams.getAll("code").length > 1 || url.searchParams.getAll("state").length > 1) error = "回调地址包含重复参数，请重新复制本次完整回调地址。"
      else if (!state) error = "回调地址缺少 state，请粘贴完整的本次回调地址。"
      else if (!code.trim()) error = "回调地址缺少授权码，请完成登录并同意授权后，再复制完整回调地址。"
    } catch { error = "回调地址格式无效，请粘贴完整地址。" }
  } else if (input.includes("://") || /^https?:/i.test(input)) {
    error = "请粘贴完整的 HTTP 回调地址或平台显示的授权码。"
  } else if (input.includes("#")) {
    const index = input.indexOf("#")
    code = input.slice(0, index).trim()
    state = input.slice(index + 1).trim()
    if (!state) error = "授权结果缺少 state，请粘贴完整结果。"
    else if (!code) error = "授权结果缺少授权码，请粘贴完整结果。"
  }
  if (!error && expectedState && state !== expectedState) error = "这是另一次授权的回调，请使用本次生成的授权链接。"
  return { code: error ? "" : code, state, error }
}
