"use client";

import { PiThinkingMap } from "./native-advanced";
import { useState } from "react";
import { Download, Loader2, Plus, SearchIcon, Trash2 } from "lucide-react";
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/coss-ui/input-group";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import { SelectField } from "@/components/ui/select-field";
import { record, str, type Config } from "@/lib/cc-switch/native-config";

export function NativeHeaders({
  value,
  onChange,
}: {
  value: Config;
  onChange: (value: Config) => void;
}) {
  const entries = Object.entries(value);
  return (
    <details className="rounded-lg border border-border-default">
      <summary className="cursor-pointer px-4 py-3 text-sm font-medium">
        请求头{entries.length ? `（${entries.length}）` : ""}
      </summary>
      <div className="space-y-2 border-t border-border-default p-3">
        {entries.map(([key, val], index) => (
          <div key={index} className="flex items-center gap-2">
            <Input
              aria-label={`请求头 ${index + 1} 名称`}
              value={key}
              className="min-w-0 flex-1"
              onChange={(e) =>
                onChange(
                  Object.fromEntries(
                    entries.map((entry, i) =>
                      i === index ? [e.target.value, entry[1]] : entry,
                    ),
                  ),
                )
              }
            />
            <Input
              aria-label={`请求头 ${index + 1} 值`}
              value={str(val)}
              className="min-w-0 flex-1"
              onChange={(e) => onChange({ ...value, [key]: e.target.value })}
            />
            <Button
              variant="ghost"
              size="icon-sm"
              aria-label={`移除请求头 ${index + 1}`}
              onClick={() =>
                onChange(
                  Object.fromEntries(entries.filter((_, i) => i !== index)),
                )
              }
            >
              <Trash2 />
            </Button>
          </div>
        ))}
        <Button
          size="sm"
          variant="outline"
          onClick={() => {
            let key = "X-Custom-Header";
            while (key in value) key += "-New";
            onChange({ ...value, [key]: "" });
          }}
        >
          <Plus />
          添加请求头
        </Button>
      </div>
    </details>
  );
}

export function NativeModelCapabilities({
  value,
  client,
  onChange,
}: {
  value: Config;
  client: string;
  onChange: (value: Config) => void;
}) {
  const number = (key: string, label: string, nested?: string) => {
    const object = nested ? record(value[nested]) : value;
    return (
      <label className="space-y-1.5 text-sm" key={key}>
        {label}
        <Input
          aria-label={label}
          type="number"
          min={key.includes("context") || key === "maxTokens" ? 1 : 0}
          step="any"
          value={typeof object[key] === "number" ? String(object[key]) : ""}
          placeholder="使用默认值"
          onChange={(e) => {
            const next = { ...object };
            if (e.target.value === "") delete next[key];
            else next[key] = Number(e.target.value);
            onChange(nested ? { ...value, [nested]: next } : next);
          }}
        />
      </label>
    );
  };
  return (
    <div className="space-y-4">
      <div className="grid gap-3 sm:grid-cols-2">
        {number(
          client === "opencode" ? "context" : "contextWindow",
          "上下文窗口",
          client === "opencode" ? "limit" : undefined,
        )}
        {number(
          client === "opencode" ? "output" : "maxTokens",
          "最大输出 Token",
          client === "opencode" ? "limit" : undefined,
        )}
        <label className="space-y-1.5 text-sm">
          推理能力
          <SelectField
            id="model-reasoning"
            value={
              value.reasoning === true
                ? "true"
                : value.reasoning === false
                  ? "false"
                  : "default"
            }
            options={[
              { value: "default", label: "使用默认值" },
              { value: "true", label: "支持" },
              { value: "false", label: "不支持" },
            ]}
            onValueChange={(v) => {
              const next = { ...value };
              if (v === "default") delete next.reasoning;
              else next.reasoning = v === "true";
              onChange(next);
            }}
          />
        </label>
        <fieldset className="space-y-2">
          <legend className="text-sm">输入类型</legend>
          <div className="flex flex-wrap gap-3">
            {(client === "opencode"
              ? ["text", "image", "audio"]
              : ["text", "image"]
            ).map((type) => {
              const inputs =
                client === "opencode"
                  ? record(value.modalities).input
                  : value.input;
              return (
                <label key={type} className="flex items-center gap-2 text-sm">
                  <input
                    type="checkbox"
                    checked={Array.isArray(inputs) && inputs.includes(type)}
                    onChange={(e) => {
                      const next = Array.isArray(inputs) ? inputs : [];
                      const input = e.target.checked
                        ? [...next, type]
                        : next.filter((v) => v !== type);
                      onChange(
                        client === "opencode"
                          ? {
                              ...value,
                              modalities: {
                                ...record(value.modalities),
                                input,
                              },
                            }
                          : { ...value, input },
                      );
                    }}
                  />
                  {{ text: "文本", image: "图片", audio: "音频" }[type]}
                </label>
              );
            })}
          </div>
        </fieldset>
      </div>
      {client === "pi" && (
        <PiThinkingMap
          value={record(value.thinkingLevelMap)}
          onChange={(thinkingLevelMap) =>
            onChange({ ...value, thinkingLevelMap })
          }
        />
      )}
      <fieldset className="grid gap-3 sm:grid-cols-2">
        <legend className="mb-2 text-sm font-medium">
          价格（美元 / 百万 Token）
        </legend>
        {number("input", "输入价格", "cost")}
        {number("output", "输出价格", "cost")}
        {number("cacheRead", "缓存读取价格", "cost")}
        {number("cacheWrite", "缓存写入价格", "cost")}
      </fieldset>
    </div>
  );
}

export function FetchNativeModels({
  authBinding,
  baseUrl,
  apiKey,
  protocol,
  headers,
  multiple,
  onSelect,
}: {
  authBinding?: { authProvider: string; accountId?: string };
  baseUrl: string;
  apiKey: string;
  protocol: string;
  headers: Config;
  multiple: boolean;
  onSelect: (ids: string[]) => void;
}) {
  const [busy, setBusy] = useState(false),
    [error, setError] = useState(""),
    [models, setModels] = useState<string[]>([]),
    [query, setQuery] = useState("");
  const fetchModels = async () => {
    setBusy(true);
    setError("");
    setModels([]);
    try {
      const response = await fetch("http://127.0.0.1:17372/models", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          baseUrl,
          authBinding,
          apiKey,
          apiFormat: protocol,
          requestHeaders: headers,
        }),
        signal: AbortSignal.timeout(55000),
      });
      const result = await response.json();
      if (!result.success) throw Error(result.error || "获取模型失败");
      const ids = (result.models as { id: string }[]).map((m) => m.id);
      setModels(ids);
      if (!ids.length) setError("接口未返回模型，可手动填写。");
    } catch (error) {
      setError(
        error instanceof TypeError
          ? "无法连接本地组件，请检查本地组件是否运行。"
          : error instanceof Error
            ? error.message
            : "获取模型失败",
      );
    } finally {
      setBusy(false);
    }
  };
  const shown = models.filter((id) =>
    id.toLowerCase().includes(query.toLowerCase()),
  );
  return (
    <div className="space-y-2">
      <Button
        size="sm"
        variant="outline"
        disabled={busy || !baseUrl}
        onClick={() => void fetchModels()}
      >
        {busy ? <Loader2 className="animate-spin" /> : <Download />}
        {busy ? "获取中…" : "获取上游模型"}
      </Button>
      {error && (
        <p role="alert" className="break-words text-sm text-danger-text">
          {error}
        </p>
      )}
      {models.length > 0 && (
        <div className="space-y-2 rounded-lg border border-border-default p-3">
          <InputGroup><InputGroupInput
            type="search"
            aria-label="搜索上游模型"
            placeholder="搜索模型…"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          /><InputGroupAddon><SearchIcon aria-hidden="true" /></InputGroupAddon></InputGroup>
          <div className="max-h-40 overflow-y-auto overscroll-contain">
            {shown.map((id) => (
              <button
                key={id}
                type="button"
                className="block w-full break-all rounded px-2 py-2 text-left text-sm hover:bg-bg-sunken"
                onClick={() => {
                  onSelect([id]);
                  setModels([]);
                }}
              >
                {id}
              </button>
            ))}
            {!shown.length && (
              <p className="p-2 text-sm text-muted-foreground">
                没有匹配的模型
              </p>
            )}
          </div>
          {multiple && (
            <Button
              size="sm"
              variant="outline"
              onClick={() => {
                onSelect(shown);
                setModels([]);
              }}
            >
              使用搜索结果（{shown.length}）
            </Button>
          )}
        </div>
      )}
    </div>
  );
}
