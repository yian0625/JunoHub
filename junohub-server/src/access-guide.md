# JunoHub 接入指引

选择可用模型，创建访问密钥，再把 JunoHub 地址接入你的应用。本文中的地址、模型名与密钥均为示例；模型详情提供当前站点的完整调用地址和代码。

## 快速开始

1. 在「可用模型」选择模型；管理员可在「模型路由」查看同一目录。点击「价格与接入」查看它对应的接口和价格。
2. 在「我的密钥」创建访问密钥，确认密钥的模型范围包含需要调用的别名。
3. 使用模型详情的客户端调用地址发起请求，`model` 填写**调用模型名（对外别名）**。
4. 在「请求日志」查看状态、实际用量和费用。普通用户只看到自己的调用记录。

供应商的 Key 和上游路径由管理员配置。应用使用 JunoHub 签发的访问密钥；不要把密钥放在网页前端、公开仓库或文档中。

## 地址与身份验证

| 配置项 | 内容 |
| --- | --- |
| 网关地址 | 当前 JunoHub 网站的根地址，例如 `https://your-junohub.example` |
| OpenAI 兼容客户端的 Base URL | 网关地址加 `/v1`；SDK 会追加接口路径 |
| 完整请求地址 | 模型详情显示的 POST 地址 |
| API Key | 「我的密钥」中的访问密钥 |
| Model | 模型的对外别名 |
| 鉴权请求头 | `Authorization: Bearer <JunoHub访问密钥>` |
| 请求类型 | JSON；图片编辑和语音转写也支持文件表单 |

先在本机设置环境变量：

```bash
export JUNOHUB_ORIGIN="https://your-junohub.example"
export JUNOHUB_API_KEY="你的访问密钥"
```

只把 `/v1` 填入需要 Base URL 的客户端配置；使用完整请求地址时不要再重复追加 `/v1`。需要自动配置本机工具时，使用「客户端配置」。

## 查询与选择模型

控制台的模型目录包含模型类型、响应方式、计费单价和调用名称。搜索模型名称或接口，或使用模型类型下拉筛选。

应用可通过访问密钥读取有权调用的模型：

```bash
curl "$JUNOHUB_ORIGIN/v1/models" \
  -H "Authorization: Bearer $JUNOHUB_API_KEY"
```

响应中的 `data[].id` 是调用名称，`data[].junohub` 提供客户端接口、请求格式、返回方式、能力和价格。`request_formats` 表示网关接收的格式，上游仍须支持对应请求。通过 `GET /v1/models/{模型别名}` 可查询单个模型；别名中的特殊字符需要 URL 编码。密钥的模型范围、路由启用状态以及供应商状态共同决定是否能调用。目录中的“已开放”表示配置允许调用，不代表供应商当前健康。

## 对话接口

按模型详情的接口选择请求格式；一个别名对应其配置的接口。以下请求中的 `MODEL_ALIAS` 替换为该接口的实际模型名。

### Chat Completions

```bash
curl "$JUNOHUB_ORIGIN/v1/chat/completions" \
  -H "Authorization: Bearer $JUNOHUB_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"model":"MODEL_ALIAS","messages":[{"role":"user","content":"你好"}]}'
```

### Responses

```bash
curl "$JUNOHUB_ORIGIN/v1/responses" \
  -H "Authorization: Bearer $JUNOHUB_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"model":"MODEL_ALIAS","input":"你好"}'
```

### Messages

```bash
curl "$JUNOHUB_ORIGIN/v1/messages" \
  -H "Authorization: Bearer $JUNOHUB_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"model":"MODEL_ALIAS","max_tokens":256,"messages":[{"role":"user","content":"你好"}]}'
```

同协议路由直接转发；已配置的 Messages → Chat Completions 适配等待上游非流式结果，再返回 JSON 或兼容事件。该适配不支持 thinking、缓存控制和内置工具等 Anthropic 扩展参数，未实现的协议组合不可配置。

## 流式响应

对支持流式的对话模型，在请求体中添加 `"stream": true`，curl 加 `-N` 避免输出缓冲。接口协议和上游模型都须支持所需流式格式；不要向媒体任务接口添加 stream 来代替查询。

```bash
curl -N "$JUNOHUB_ORIGIN/v1/chat/completions" \
  -H "Authorization: Bearer $JUNOHUB_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"model":"MODEL_ALIAS","stream":true,"messages":[{"role":"user","content":"你好"}]}'
```

使用对应协议的事件格式解析流，不把每一段当作完整 JSON。上游未返回完整实际用量，或连接中途取消时，日志可能标记未计价。

## 图片、视频与语音

JunoHub 提供稳定的客户端入口；供应商的请求字段、返回结构与资源有效期可能不同。模型管理员应提供该路由的具体参数示例。

| 类型 | 客户端接口 | 请求说明 |
| --- | --- | --- |
| 图片生成 | `POST /v1/images/generations` | JSON，按该模型要求填写 prompt 等字段 |
| 图片编辑 | `POST /v1/images/edits` | 文件表单或 JSON，必须与上游格式匹配 |
| 视频生成 | `POST /v1/videos/generations` | 供应商要求的 JSON 字段，可配置任务模式 |
| 音频生成 | `POST /v1/audio/generations` | 供应商要求的 JSON 字段 |
| 语音合成 | `POST /v1/audio/speech` | 供应商要求的 JSON 字段，可返回音频二进制 |
| 语音识别 | `POST /v1/audio/transcriptions` | 文件表单或供应商要求的 JSON 字段 |
| 语音翻译 | `POST /v1/audio/translations` | 文件表单或供应商要求的 JSON 字段 |

将供应商文档要求的 JSON 保存为 `request.json`，把 `model` 改为 JunoHub 调用名，然后调用模型详情中的地址：

```bash
curl "$JUNOHUB_ORIGIN/v1/images/generations" \
  -H "Authorization: Bearer $JUNOHUB_API_KEY" \
  -H "Content-Type: application/json" \
  --data-binary @request.json
```

### 文件表单请求

API Key 供应商的图片编辑、语音转写和语音翻译支持 `multipart/form-data`，常见兼容接口示例：

```bash
curl "$JUNOHUB_ORIGIN/v1/images/edits" \
  -H "Authorization: Bearer $JUNOHUB_API_KEY" \
  --form-string "model=MODEL_ALIAS" \
  -F "image[]=@image.png;type=image/png"
```

```bash
curl "$JUNOHUB_ORIGIN/v1/audio/transcriptions" \
  -H "Authorization: Bearer $JUNOHUB_API_KEY" \
  --form-string "model=MODEL_ALIAS" \
  -F "file=@audio.mp3;type=audio/mpeg"
```

```bash
curl "$JUNOHUB_ORIGIN/v1/audio/translations" \
  -H "Authorization: Bearer $JUNOHUB_API_KEY" \
  --form-string "model=MODEL_ALIAS" \
  -F "file=@audio.mp3;type=audio/mpeg"
```

文件字段名、prompt、language、返回格式等参数按对应模型文档添加。多个图片可重复使用同一文件字段名，网关会保留每个文件。不要手动设置 Content-Type 或 boundary，交给 curl/SDK 生成。

请求体（所有文件与字段合计）最多 32MiB，最多 64 个表单项，单个文本字段最多 1MiB；必须有唯一文本 model 和至少一个非空文件。文件内容不持久化到服务器或 OSS，model 被替换为上游模型，其余表单字段直接转发。账户授权引擎暂不支持文件表单，JSON 与文件表单之间不做自动转换。图片、音频请求失败后不自动切换账户重发，以免重复计费。

文件管理、实时 WebSocket 音频及厂商专属签名鉴权尚未接入。音频二进制请保存为文件，图片和视频的资源链接请在过期前下载。

## 向量、搜索与重排

| 类型 | 客户端接口 | 请求字段 |
| --- | --- | --- |
| 向量化 | `POST /v1/embeddings` | 兼容模型使用 input；多模态字段须核对供应商 |
| 搜索 | `POST /v1/search` | 供应商要求的 JSON 查询字段 |
| 文本重排 | `POST /v1/rerank` | 供应商要求的 query、documents 等字段 |

兼容向量模型示例：

```bash
curl "$JUNOHUB_ORIGIN/v1/embeddings" \
  -H "Authorization: Bearer $JUNOHUB_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"model":"MODEL_ALIAS","input":"需要向量化的文本"}'
```

向量模型只配置输入 Token 价格，不按向量维度或结果数组长度计费。搜索与重排的字段并非所有厂商一致；请按具体模型文档填写。

## 异步任务

模型详情显示“异步任务”时，客户端需要处理提交和查询两个步骤。

1. 向模型的 POST 接口提交一次生成请求。
2. 收到 HTTP 202 后保存 `task_id`、`query_url` 和 `expires_at`。
3. 使用**提交时的同一把访问密钥** GET 查询，建议间隔 5–10 秒。
4. `queued` 或 `processing` 继续等待；`succeeded` 读取结果，`failed` 查看错误。

```bash
curl "$JUNOHUB_ORIGIN/v1/tasks/$TASK_ID" \
  -H "Authorization: Bearer $JUNOHUB_API_KEY"
```

后台会持续查询并幂等结算，关闭页面不影响处理。重复查询不会新增生成请求或重复计费；最终用量更新原提交日志。任务使用提交时的供应商凭证和价格快照。路由删除后已接受任务仍能按原密钥查询，撤销密钥、密钥过期或停用用户会阻止客户端读取。

任务有效期为 1 小时。超时后未知用量标记未计价，不重新提交。POST 超时可能代表供应商已开始处理，先核对日志及供应商记录，再决定是否重试。任务接口不能直接接入只接受立即返回图片 data 的 SDK。

## 价格与实际用量

| 计费方式 | 实际用量来源 |
| --- | --- |
| Token | 供应商响应或流式最终事件报告的输入、输出 Token |
| 向量化 Token | 实际输入 Token；兼容响应的 total_tokens 可作为输入读取 |
| 按张 | 实际有效返回图片数，或管理员配置的实际张数字段 |
| 按秒 | 管理员指定的供应商响应中的实际秒数字段 |
| 按字符 | 管理员指定的供应商响应中的实际字符数字段 |
| 按次 | 成功直接响应或成功任务终态 |

费用按调用时的价格快照计算，异步任务在完成时结算。单价留空表示未配置；0 表示免费。缺失实际用量时标记未计价，不按请求 n、文本长度、向量维度或估算时长补账。

「请求日志 → 详情」显示计费用量、单位、单价、金额及未计价原因。当前价格为单一计费规则，不支持按图片质量、尺寸、缓存或多模态分别计价；这类模型请由管理员确认能否准确配置后再使用。

## 错误处理与排障

| 现象 | 检查方向 |
| --- | --- |
| 400 / 格式错误 | 请求字段、model 名称与模型配置的接口是否匹配 |
| 401 | 访问密钥是否正确、过期或已撤销，账户是否启用 |
| 403 / 无权限 | 密钥是否允许当前模型，是否访问了管理员功能 |
| 模型不存在 | 使用对外别名，核对管理员和路由是否启用 |
| 404 | 客户端地址、上游路径；任务查询的原密钥及有效期 |
| 413 | 请求体超过 32MiB，缩小文件或拆分请求 |
| 429 | 请求限制、供应商容量和上游限流；避免高频轮询 |
| 502 / 504 | 请求日志中的具体错误、供应商连接和超时；生成请求先核对再重试 |
| 未计价 | 模型价格未配置、实际用量缺失、响应取消或失败 |

失败后保留状态码、模型调用名和请求发生时间，通过请求日志定位。不要向他人发送访问密钥或供应商凭证。

## 管理员配置路由

普通用户只需完成快速开始。管理员先添加供应商，再分别配置两侧：

- **上游平台**：供应商、上游模型、请求格式、请求路径、鉴权和响应方式。
- **JunoHub 调用**：对外别名和客户端接口。调用地址由接口预设固定。

常见对话格式支持 Chat Completions、Responses、Messages；其他供应商路径使用自定义。自定义路径替换 Base URL 的路径部分，保留域名，例如 `/api/generate`；不可把供应商域名填入路径。

上游任务模式需要配置含 `{task_id}` 的 GET 查询路径。测试上游会发送真实请求，可能产生费用；通过后再保存。配置存在或测试列表成功不代表所有模型都可调用。

接入指引支持管理员编辑 Markdown 和下载；普通用户只能阅读。模型目录的价格、接口和示例来自当前路由配置，不会因为编辑文档而改变。
