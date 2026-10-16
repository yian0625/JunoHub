"use client";

import { useEffect, useState } from "react";
import { toast } from "sonner";
import Image from "next/image";
import {
  X,
  ArrowUpAZ,
  Check,
  Code2,
  Eye,
  EyeOff,
  Layers,
  Loader2,
  Plus,
  Search,
  SearchIcon,
  Settings2,
  Trash2,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/coss-ui/input-group";
import { SelectField } from "@/components/ui/select-field";
import {
  FetchNativeModels,
  NativeHeaders,
  NativeModelCapabilities,
} from "./native-provider-options";
import { Label } from "@/components/ui/label";
import { AppTooltip } from "@/components/ui/tooltip";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogFooter,
} from "@/components/ui/dialog";
import {
  consoleAPI,
  type ConsoleKey,
  type IntegrationProfileInput,
} from "@/lib/api";
import {
  additiveClients,
  createNativeForm,
  nativeFields,
  nativePayload,
  presets,
  record,
  str,
  updateNativeField,
  updateClaudeAuth,
  updateCodexOption,
  toml,
  type Config,
  type Preset,
} from "@/lib/cc-switch/native-config";
import { ProviderEndpoints } from "./provider-endpoints";
import { Gauge } from "lucide-react";
import { CodexCatalog, CodexReasoning, NativePricing } from "./native-advanced";
import { ManagedAccounts, authNames } from "./managed-accounts";
import iconAssets from "@/lib/cc-switch/icon-assets.json";
import { cn } from "@/lib/utils";
import { BrandLogo } from "@/components/brand-logo";

export function ProviderBrand({
  icon,
  name,
  size = 20,
}: {
  icon?: string;
  name: string;
  size?: number;
}) {
  if (icon === "junohub" || (!icon && name.toLowerCase() === "junohub")) {
    return <span className="shrink-0" style={{width: size, height: size}}><BrandLogo className="size-full" /></span>;
  }
  const asset = icon ? (iconAssets as Record<string, string>)[icon] : undefined;
  return asset ? (
    <Image
      src={`/provider-icons/${asset}`}
      alt=""
      width={size}
      height={size}
      unoptimized
      className={cn(
        "shrink-0 object-contain",
        ["openai", "grok", "pi", "opencode"].includes(icon || "") &&
          "dark:brightness-0 dark:invert",
      )}
    />
  ) : (
    <Layers
      aria-hidden="true"
      className="shrink-0 text-muted-foreground"
      style={{ width: size, height: size }}
    />
  );
}

const categoryLabels: Record<string, string> = {
  official: "官方",
  cn_official: "国内官方",
  aggregator: "聚合服务",
  third_party: "第三方",
  custom: "自定义",
  others: "其他",
};
const editorClass =
  "w-full rounded-lg border border-border-default bg-bg-sunken px-3 py-2.5 font-mono text-xs leading-6 outline-none focus:border-accent-solid focus:ring-2 focus:ring-accent-subtle";

export function NativeProviderPanel({
  form,
  keys,
  busy,
  onChange,
  onClose,
  onSave,
  onJunoHub,
}: {
  form: IntegrationProfileInput;
  keys: ConsoleKey[];
  busy: boolean;
  onChange: (form: IntegrationProfileInput) => void;
  onClose: () => void;
  onSave: () => void;
  onJunoHub: () => void;
}) {
  const client = form.metadata!.nativeClient!;
  useEffect(() => {
    toast.dismiss();
  }, []);
  const [endpointsOpen, setEndpointsOpen] = useState(false);
  const [accountsOpen, setAccountsOpen] = useState(false);
  const binding = form.metadata?.authBinding;
  const [searchOpen, setSearchOpen] = useState(false);
  const [search, setSearch] = useState("");
  const [sort, setSort] = useState(false);
  const [showKey, setShowKey] = useState(false);
  const [raw, setRaw] = useState(JSON.stringify(form.settingsConfig, null, 2));
  const [error, setError] = useState("");
  const [modelDraft, setModelDraft] = useState<{
    index: number;
    value: string;
  } | null>(null);
  const [templateValues, setTemplateValues] = useState<Record<string, string>>(
    {},
  );
  const [originalPreset, setOriginalPreset] = useState<Preset | null>(null);
  const config = form.settingsConfig || {};
  const isJunoHub = form.metadata?.presetId === "junohub";
  const [keyLoading, setKeyLoading] = useState(false);
  let codexOptions: Config = {};
  let fields = { baseUrl: "", apiKey: "", model: "", protocol: "" };
  try {
    fields = nativeFields(client, config);
    if (client === "codex") codexOptions = toml(config);
  } catch {
    /* The editable document exposes its parse error below. */
  }
  const discoveryProtocol =
    (
      {
        anthropic:
          client === "claude" &&
          record(config.env).ANTHROPIC_API_KEY === undefined
            ? "anthropic-bearer"
            : "anthropic-messages",
        anthropic_messages:
          client === "claude" &&
          record(config.env).ANTHROPIC_API_KEY === undefined
            ? "anthropic-bearer"
            : "anthropic-messages",
        openai_chat: "openai-completions",
        openai_responses: "openai-responses",
        gemini_native: "google-generative-ai",
      } as Record<string, string>
    )[form.metadata?.apiFormat || ""] ||
    (client === "gemini"
      ? "google-generative-ai"
      : client === "claude" &&
          record(config.env).ANTHROPIC_API_KEY !== undefined
        ? "anthropic-messages"
        : fields.protocol === "@ai-sdk/anthropic"
          ? "anthropic-messages"
          : fields.protocol === "@ai-sdk/google"
            ? "google-generative-ai"
            : client === "claude"
              ? "anthropic-bearer"
              : fields.protocol);
  const selectedPreset = presets[client].find(
    (_, index) => form.metadata?.presetId === `${client}-${index}`,
  );
  const official = form.metadata?.category === "official";
  const entries = presets[client]
    .map((preset, index) => ({ preset, id: `${client}-${index}` }))
    .filter(
      ({ preset }) =>
        !search.trim() ||
        `${preset.name} ${preset.websiteUrl || ""}`
          .toLocaleLowerCase()
          .includes(search.trim().toLocaleLowerCase()),
    );
  if (sort) entries.sort((a, b) => a.preset.name.localeCompare(b.preset.name));
  const change = (patch: Partial<IntegrationProfileInput>) => {
    setError("");
    onChange({ ...form, ...patch });
  };
  const setConfig = (next: Config) => {
    setRaw(JSON.stringify(next, null, 2));
    setError("");
    change({ settingsConfig: next });
  };
  const setField = (
    field: "baseUrl" | "apiKey" | "model" | "protocol",
    value: string,
  ) => {
    try {
      setConfig(updateNativeField(client, config, field, value));
    } catch {
      setError("请先修正配置中的 TOML 格式");
    }
  };
  const selectPreset = (preset?: Preset, id = "custom") => {
    const next = createNativeForm(client, preset, id);
    onChange(next);
    setRaw(JSON.stringify(next.settingsConfig, null, 2));
    setError("");
    setShowKey(false);
    setTemplateValues({});
    setOriginalPreset(preset || null);
  };
  const parseRaw = (value: string) => {
    setRaw(value);
    try {
      const next: unknown = JSON.parse(value);
      if (!next || typeof next !== "object" || Array.isArray(next))
        throw new Error("配置必须是 JSON 对象");
      nativeFields(client, next as Config);
      change({ settingsConfig: next as Config });
      setError("");
    } catch (error) {
      setError(error instanceof Error ? error.message : "配置格式无效");
    }
  };
  const save = () => {
    try {
      const parsed: unknown = JSON.parse(raw);
      if (!parsed || typeof parsed !== "object" || Array.isArray(parsed))
        throw Error("配置必须是 JSON 对象");
      nativeFields(client, parsed as Config);
    } catch (error) {
      setError(error instanceof Error ? error.message : "配置格式无效");
      return;
    }
    if (isJunoHub && !fields.apiKey) {
      setError("请选择 JunoHub 访问密钥");
      return;
    }
    if (!form.name.trim()) {
      setError("请输入供应商名称");
      return;
    }
    if (
      additiveClients.includes(client) &&
      !form.metadata?.providerKey?.trim()
    ) {
      setError("请输入供应商标识");
      return;
    }
    if (selectedPreset?.requiresOAuth && !binding?.accountId) {
      setError("请先登录并选择此供应商使用的托管账号。");
      return;
    }
    if (JSON.stringify(config).match(/\{\{[^}]+\}\}/)) {
      setError("请填写预设中的模板参数");
      return;
    }
    const catalog = Array.isArray(config.modelCatalog)
      ? config.modelCatalog
      : record(config.modelCatalog).models;
    if (client === "codex" && Array.isArray(catalog)) {
      const ids = catalog.map((m) => str(record(m).model) || str(record(m).id));
      if (ids.some((id) => !id.trim()) || new Set(ids).size !== ids.length) {
        setError("模型目录的 ID 不能为空或重复");
        return;
      }
    }
    try {
      nativePayload(form);
      setError("");
      onSave();
    } catch (error) {
      setError(error instanceof Error ? error.message : "配置格式无效");
    }
  };
  const multi = additiveClients.includes(client);
  const modelEntries = Array.isArray(config.models)
    ? config.models.map((value, index) => ({
        id: str(record(value).id),
        value: record(value),
        index,
      }))
    : Object.entries(record(config.models)).map(([id, value], index) => ({
        id,
        value: record(value),
        index,
      }));
  const writeModels = (entries: typeof modelEntries) => {
    if (new Set(entries.map((item) => item.id)).size !== entries.length) {
      setError("模型 ID 不能重复");
      return;
    }
    setConfig({
      ...config,
      models: Array.isArray(config.models)
        ? entries.map(({ id, value }) => ({ ...value, id }))
        : Object.fromEntries(entries.map(({ id, value }) => [id, value])),
    });
  };
  return (
    <Dialog open onOpenChange={(open) => !open && !busy && onClose()}>
      <DialogContent
        showCloseButton={false}
        className="h-[min(88dvh,54rem)] w-[calc(100vw-1.5rem)] min-w-0 max-w-[52rem] grid-cols-[minmax(0,1fr)] grid-rows-[auto_minmax(0,1fr)_auto] gap-0 overflow-hidden rounded-2xl bg-bg-surface p-0 sm:max-w-[52rem]"
      >
        <DialogHeader className="flex min-h-16 flex-row items-center justify-between gap-3 border-b border-border-default px-4 py-3 sm:px-6">
          <DialogTitle className="text-lg font-semibold">
            {form.id ? "编辑供应商" : "添加供应商"}
          </DialogTitle>
          <Button
            variant="ghost"
            size="icon-sm"
            onClick={onClose}
            disabled={busy}
            aria-label="关闭"
          >
            <X />
          </Button>
        </DialogHeader>
        <div className="min-h-0 min-w-0 overflow-y-auto overscroll-contain px-4 py-5 sm:px-6">
          <div className="space-y-5">
            {!form.id && (
              <section className="space-y-3" aria-label="供应商预设">
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <Label>供应商预设</Label>
                  <div className="flex items-center gap-1">
                    {searchOpen && (
                      <InputGroup className="w-40 sm:w-60"><InputGroupInput
                        type="search"
                        autoFocus
                        aria-label="搜索供应商预设"
                        placeholder="搜索预设…"
                        value={search}
                        onChange={(event) => setSearch(event.target.value)}
                        onKeyDown={(event) => {
                          if (event.key === "Escape") {
                            setSearch("");
                            setSearchOpen(false);
                            event.stopPropagation();
                          }
                        }}
                      /><InputGroupAddon><SearchIcon aria-hidden="true" /></InputGroupAddon></InputGroup>
                    )}
                    <AppTooltip content="搜索预设">
                      <Button
                        variant="ghost"
                        size="icon-sm"
                        className="size-11"
                        aria-label="搜索预设"
                        aria-pressed={searchOpen}
                        onClick={() => {
                          setSearchOpen(!searchOpen);
                          setSearch("");
                        }}
                      >
                        <Search />
                      </Button>
                    </AppTooltip>
                    <AppTooltip content={sort ? "恢复默认顺序" : "按名称排序"}>
                      <Button
                        variant="ghost"
                        size="icon-sm"
                        className="size-11"
                        aria-label="按名称排序"
                        aria-pressed={sort}
                        onClick={() => setSort(!sort)}
                      >
                        <ArrowUpAZ />
                      </Button>
                    </AppTooltip>
                  </div>
                </div>
                <div className="grid max-h-44 grid-cols-[repeat(auto-fill,minmax(140px,1fr))] gap-2 overflow-y-auto overscroll-contain">
                  <button
                    type="button"
                    aria-pressed={form.metadata?.presetId === "custom"}
                    onClick={() => selectPreset()}
                    className={cn(
                      "flex min-h-9 items-center gap-2 rounded-lg px-3 py-2 text-left text-sm font-medium",
                      form.metadata?.presetId === "custom"
                        ? "bg-accent-solid text-white"
                        : "bg-bg-sunken text-muted-foreground",
                    )}
                  >
                    <Settings2 className="size-4" />
                    自定义
                  </button>
                  {(!search.trim() ||
                    "junohub".includes(search.trim().toLowerCase())) && (
                    <button
                      type="button"
                      onClick={onJunoHub}
                      className="flex min-h-9 items-center gap-2 rounded-lg bg-bg-sunken px-3 py-2 text-left text-sm font-medium text-muted-foreground"
                    >
                      <ProviderBrand icon="junohub" name="JunoHub" size={20} />
                      JunoHub
                    </button>
                  )}
                  {entries.map(({ preset, id }) => (
                    <button
                      type="button"
                      key={id}
                      aria-pressed={form.metadata?.presetId === id}
                      onClick={() => selectPreset(preset, id)}
                      className={cn(
                        "flex min-h-9 min-w-0 items-center gap-2 rounded-lg px-3 py-2 text-left text-sm font-medium transition-colors",
                        form.metadata?.presetId === id
                          ? "bg-accent-solid text-white"
                          : "bg-bg-sunken text-muted-foreground hover:bg-bg-raised",
                      )}
                    >
                      <ProviderBrand
                        icon={preset.icon}
                        name={preset.name}
                        size={16}
                      />
                      <span className="truncate">{preset.name}</span>
                    </button>
                  ))}
                  {!entries.length &&
                    search.trim() &&
                    !"junohub".includes(search.trim().toLowerCase()) && (
                      <p className="col-span-full p-3 text-sm text-muted-foreground">
                        没有匹配的预设
                      </p>
                    )}
                </div>
                <p className="text-xs text-muted-foreground">
                  {categoryLabels[form.metadata?.category || "custom"] ||
                    "自定义"}{" "}
                  · 选择预设自动填写配置，也可以手动编辑。
                </p>
              </section>
            )}
            <div className="grid gap-4 sm:grid-cols-2">
              <Field label="供应商名称" id="native-name">
                <Input
                  id="native-name"
                  value={form.name}
                  onChange={(event) => change({ name: event.target.value })}
                  placeholder="输入供应商名称"
                />
              </Field>
              <Field label="官网地址" id="native-website">
                <Input
                  id="native-website"
                  value={form.websiteUrl || ""}
                  onChange={(event) =>
                    change({ websiteUrl: event.target.value })
                  }
                  placeholder="https://example.com（可选）"
                />
              </Field>
            </div>
            <Field label="备注" id="native-notes">
              <Input
                id="native-notes"
                value={form.notes || ""}
                onChange={(event) => change({ notes: event.target.value })}
                placeholder="可选：添加备注信息"
              />
            </Field>
            {multi && (
              <Field label="供应商标识" id="native-provider-key">
                <Input
                  id="native-provider-key"
                  disabled={Boolean(form.id)}
                  value={form.metadata?.providerKey || ""}
                  onChange={(event) =>
                    change({
                      metadata: {
                        ...form.metadata,
                        providerKey: event.target.value,
                      },
                    })
                  }
                  placeholder="例如 openai；用于客户端配置中的供应商名称"
                />
                {form.id && (
                  <p className="text-xs text-muted-foreground">
                    供应商标识创建后保持固定；需要另一个标识时可复制供应商。
                  </p>
                )}
              </Field>
            )}
            {official ? (
              <p className="rounded-lg border border-border-default bg-bg-sunken px-4 py-3 text-sm text-muted-foreground">
                使用客户端已有的官方登录。启用时恢复官方入口。
              </p>
            ) : (
              <>
                {Object.entries(originalPreset?.templateValues || {}).map(
                  ([key, descriptor]) => (
                    <Field
                      key={key}
                      label={descriptor.label || key}
                      id={`template-${key}`}
                    >
                      <Input
                        id={`template-${key}`}
                        value={templateValues[key] || ""}
                        placeholder={
                          descriptor.placeholder ||
                          descriptor.defaultValue ||
                          key
                        }
                        onChange={(event) => {
                          const values = {
                            ...templateValues,
                            [key]: event.target.value,
                          };
                          setTemplateValues(values);
                          let next = JSON.stringify(
                            originalPreset?.settingsConfig || config,
                          );
                          for (const [name, value] of Object.entries(values))
                            next = next
                              .split(`{{${name}}}`)
                              .join(JSON.stringify(value).slice(1, -1));
                          setConfig(JSON.parse(next));
                        }}
                      />
                    </Field>
                  ),
                )}
                {(client === "claude" || client === "codex") && (
                  <Field label="供应商 API 协议" id="native-api-format">
                    <SelectField
                      id="native-api-format"
                      value={
                        (form.metadata?.apiFormat === "anthropic_messages"
                          ? "anthropic"
                          : form.metadata?.apiFormat) ||
                        (client === "claude" ? "anthropic" : "openai_responses")
                      }
                      options={[
                        {
                          value: "anthropic",
                          label: "Anthropic Messages",
                        },
                        {
                          value: "openai_chat",
                          label: "OpenAI Chat Completions",
                        },
                        {
                          value: "openai_responses",
                          label: "OpenAI Responses",
                        },
                        ...(client === "claude"
                          ? [
                              {
                                value: "gemini_native",
                                label: "Gemini Native",
                              },
                            ]
                          : []),
                      ]}
                      onValueChange={(apiFormat) =>
                        change({ metadata: { ...form.metadata, apiFormat } })
                      }
                    />
                    {form.metadata?.apiFormat &&
                      form.metadata.apiFormat !==
                        (client === "claude"
                          ? "anthropic"
                          : "openai_responses") && (
                        <p className="text-xs text-muted-foreground">
                          启用时由本地路由转换请求与流式响应。
                        </p>
                      )}
                  </Field>
                )}
                {isJunoHub ? (
                  <Field label="JunoHub 访问密钥" id="native-junohub-key">
                    <SelectField
                      id="native-junohub-key"
                      value={String(form.metadata?.junohubKeyId || "")}
                      disabled={keyLoading}
                      options={keys
                        .filter((key) => key.status === 1)
                        .map((key) => ({
                          value: String(key.id),
                          label: `${key.name} · ${key.key_prefix}`,
                        }))}
                      placeholder="选择访问密钥"
                      onValueChange={(value) => {
                        setKeyLoading(true);
                        void consoleAPI
                          .keyReveal(Number(value))
                          .then(({ key }) => {
                            const settingsConfig = updateNativeField(
                              client,
                              config,
                              "apiKey",
                              key,
                            );
                            setRaw(JSON.stringify(settingsConfig, null, 2));
                            change({
                              settingsConfig,
                              metadata: {
                                ...form.metadata,
                                junohubKeyId: Number(value),
                              },
                            });
                          })
                          .catch((error) =>
                            setError(
                              error instanceof Error
                                ? error.message
                                : "读取访问密钥失败",
                            ),
                          )
                          .finally(() => setKeyLoading(false));
                      }}
                    />
                  </Field>
                ) : binding ? (
                  <Field label="托管账号" id="native-managed-account">
                    <Button
                      id="native-managed-account"
                      variant="outline"
                      className="w-full justify-between"
                      onClick={() => setAccountsOpen(true)}
                    >
                      <span className="truncate">
                        {authNames[binding.authProvider]} ·{" "}
                        {binding.accountName ||
                          (binding.accountId ? binding.accountId : "选择账号")}
                      </span>
                      <Settings2 />
                    </Button>
                  </Field>
                ) : (
                  <Field label="API Key" id="native-api-key">
                    <div className="relative">
                      <Input
                        id="native-api-key"
                        type={showKey ? "text" : "password"}
                        autoComplete="new-password"
                        value={fields.apiKey}
                        onChange={(event) =>
                          setField("apiKey", event.target.value)
                        }
                        placeholder="输入 API Key"
                        className="pe-10"
                      />
                      <Button
                        type="button"
                        variant="ghost"
                        size="icon-sm"
                        aria-label={showKey ? "隐藏 API Key" : "显示 API Key"}
                        className="absolute end-1 top-1/2 -translate-y-1/2"
                        onClick={() => setShowKey(!showKey)}
                      >
                        {showKey ? <EyeOff /> : <Eye />}
                      </Button>
                    </div>
                  </Field>
                )}
                {client === "claude" && !binding && (
                  <Field label="认证方式" id="native-auth-field">
                    <SelectField
                      id="native-auth-field"
                      value={
                        record(config.env).ANTHROPIC_API_KEY !== undefined
                          ? "ANTHROPIC_API_KEY"
                          : "ANTHROPIC_AUTH_TOKEN"
                      }
                      options={[
                        {
                          value: "ANTHROPIC_AUTH_TOKEN",
                          label: "Bearer Token",
                        },
                        {
                          value: "ANTHROPIC_API_KEY",
                          label: "API Key（x-api-key）",
                        },
                      ]}
                      onValueChange={(value) =>
                        setConfig(updateClaudeAuth(config, value))
                      }
                    />
                  </Field>
                )}
                <Field label="API 地址" id="native-base-url">
                  <div className="flex items-center gap-2">
                    <Input
                      id="native-base-url"
                      value={fields.baseUrl}
                      onChange={(event) =>
                        setField("baseUrl", event.target.value)
                      }
                      placeholder="https://api.example.com/v1"
                    />
                    {!binding && (
                      <Button
                        variant="outline"
                        size="icon"
                        aria-label="管理 API 端点"
                        onClick={() => setEndpointsOpen(true)}
                      >
                        <Gauge />
                      </Button>
                    )}
                  </div>
                </Field>
                <FetchNativeModels
                  authBinding={binding}
                  multiple={multi}
                  key={`${fields.baseUrl}:${fields.apiKey}:${fields.protocol}`}
                  baseUrl={fields.baseUrl}
                  apiKey={fields.apiKey}
                  protocol={discoveryProtocol}
                  headers={record(
                    client === "opencode"
                      ? record(config.options).headers
                      : config.headers,
                  )}
                  onSelect={(ids) => {
                    if (!ids.length) return;
                    if (multi)
                      writeModels([
                        ...modelEntries,
                        ...ids
                          .filter(
                            (id) => !modelEntries.some((m) => m.id === id),
                          )
                          .map((id, index) => ({
                            id,
                            value: { name: id },
                            index: modelEntries.length + index,
                          })),
                      ]);
                    else setField("model", ids[0]);
                  }}
                />
                {!multi && (
                  <Field label="默认模型" id="native-model">
                    <Input
                      id="native-model"
                      value={fields.model}
                      onChange={(event) =>
                        setField("model", event.target.value)
                      }
                      placeholder="可留空，使用客户端默认模型"
                    />
                  </Field>
                )}
              </>
            )}
            {!official && client === "claude" && (
              <details className="rounded-lg border border-border-default">
                <summary className="cursor-pointer px-4 py-3 text-sm font-medium">
                  模型映射
                </summary>
                <div className="grid gap-3 border-t border-border-default p-4 sm:grid-cols-2">
                  {["HAIKU", "SONNET", "OPUS", "FABLE"].map((tier) => (
                    <div key={tier} className="space-y-3">
                      <Field
                        label={`${tier.charAt(0) + tier.slice(1).toLowerCase()} 模型`}
                        id={`native-${tier}`}
                      >
                        <Input
                          id={`native-${tier}`}
                          value={str(
                            record(config.env)[
                              `ANTHROPIC_DEFAULT_${tier}_MODEL`
                            ],
                          )}
                          onChange={(e) => {
                            const env = { ...record(config.env) };
                            if (e.target.value)
                              env[`ANTHROPIC_DEFAULT_${tier}_MODEL`] =
                                e.target.value;
                            else delete env[`ANTHROPIC_DEFAULT_${tier}_MODEL`];
                            setConfig({ ...config, env });
                          }}
                        />
                      </Field>
                      <Field
                        label={`${tier.charAt(0) + tier.slice(1).toLowerCase()} 显示名称`}
                        id={`native-${tier}-name`}
                      >
                        <Input
                          id={`native-${tier}-name`}
                          value={str(
                            record(config.env)[
                              `ANTHROPIC_DEFAULT_${tier}_MODEL_NAME`
                            ],
                          )}
                          onChange={(e) => {
                            const env = { ...record(config.env) };
                            if (e.target.value)
                              env[`ANTHROPIC_DEFAULT_${tier}_MODEL_NAME`] =
                                e.target.value;
                            else
                              delete env[
                                `ANTHROPIC_DEFAULT_${tier}_MODEL_NAME`
                              ];
                            setConfig({ ...config, env });
                          }}
                        />
                      </Field>
                    </div>
                  ))}
                  <Field label="子代理模型" id="native-subagent">
                    <Input
                      id="native-subagent"
                      value={str(record(config.env).CLAUDE_CODE_SUBAGENT_MODEL)}
                      onChange={(e) => {
                        const env = { ...record(config.env) };
                        if (e.target.value)
                          env.CLAUDE_CODE_SUBAGENT_MODEL = e.target.value;
                        else delete env.CLAUDE_CODE_SUBAGENT_MODEL;
                        setConfig({ ...config, env });
                      }}
                    />
                  </Field>
                </div>
              </details>
            )}
            {!official && client === "codex" && (
              <Field label="推理强度" id="native-reasoning-effort">
                <SelectField
                  id="native-reasoning-effort"
                  value={str(codexOptions.model_reasoning_effort) || "default"}
                  options={[
                    { value: "default", label: "使用默认值" },
                    ...[
                      "none",
                      "minimal",
                      "low",
                      "medium",
                      "high",
                      "xhigh",
                      "max",
                      "ultra",
                    ].map((value) => ({ value, label: value })),
                  ]}
                  onValueChange={(v) =>
                    setConfig(
                      updateCodexOption(
                        config,
                        "model_reasoning_effort",
                        v === "default" ? "" : v,
                      ),
                    )
                  }
                />
              </Field>
            )}
            {!official && client === "codex" && (
              <>
                <CodexCatalog
                  value={config.modelCatalog}
                  onChange={(modelCatalog) =>
                    setConfig({ ...config, modelCatalog })
                  }
                />
                {form.metadata?.apiFormat === "openai_chat" && (
                  <CodexReasoning
                    value={form.metadata.codexChatReasoning || {}}
                    onChange={(codexChatReasoning) =>
                      change({
                        metadata: { ...form.metadata, codexChatReasoning },
                      })
                    }
                  />
                )}
              </>
            )}
            {!official && (
              <NativePricing
                value={form.metadata?.pricing || {}}
                onChange={(pricing) =>
                  change({
                    metadata: {
                      ...form.metadata,
                      pricing: pricing as Record<string, number>,
                    },
                  })
                }
              />
            )}
            {multi && (
              <NativeHeaders
                value={record(
                  client === "opencode"
                    ? record(config.options).headers
                    : config.headers,
                )}
                onChange={(headers) =>
                  setConfig(
                    client === "opencode"
                      ? {
                          ...config,
                          options: { ...record(config.options), headers },
                        }
                      : { ...config, headers },
                  )
                }
              />
            )}
            {multi && (
              <section className="space-y-3">
                <div className="flex items-center justify-between">
                  <Label>
                    模型配置{" "}
                    <span className="text-muted-foreground">
                      ({modelEntries.length})
                    </span>
                  </Label>
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => {
                      let id = "new-model";
                      while (modelEntries.some((m) => m.id === id))
                        id += "-new";
                      writeModels([
                        ...modelEntries,
                        {
                          id,
                          value: { name: id },
                          index: modelEntries.length,
                        },
                      ]);
                    }}
                  >
                    <Plus />
                    添加模型
                  </Button>
                </div>
                <Field
                  label={client === "opencode" ? "SDK 类型" : "API 协议"}
                  id="native-protocol"
                >
                  <SelectField
                    id="native-protocol"
                    value={fields.protocol}
                    onValueChange={(value) => setField("protocol", value)}
                    options={Array.from(
                      new Set([
                        fields.protocol,
                        ...(client === "opencode"
                          ? [
                              "@ai-sdk/openai",
                              "@ai-sdk/openai-compatible",
                              "@ai-sdk/anthropic",
                              "@ai-sdk/google",
                            ]
                          : client === "hermes"
                            ? [
                                "chat_completions",
                                "codex_responses",
                                "anthropic_messages",
                              ]
                            : [
                                "openai-completions",
                                "openai-responses",
                                "anthropic-messages",
                                "google-generative-ai",
                                "bedrock-converse-stream",
                              ]),
                      ]),
                    )
                      .filter(Boolean)
                      .map((value) => ({ value, label: value }))}
                  />
                </Field>
                <p className="text-xs text-muted-foreground">
                  模型 ID 用于实际请求，须与上游一致；显示名称可自定义。
                </p>
                <div className="divide-y divide-border-default rounded-lg border border-border-default">
                  {modelEntries.map((model, index) => (
                    <div
                      key={index}
                      className="flex flex-wrap items-center gap-2 p-3"
                    >
                      <label className="min-w-0 flex-1 space-y-1">
                        <span className="text-xs text-muted-foreground">显示名称</span>
                      <Input
                        aria-label={`模型 ${index + 1} 名称`}
                        title="显示名称"
                        placeholder="显示名称（可选）"
                        value={str(model.value.name)}
                        className="w-full"
                        onChange={(event) =>
                          writeModels(
                            modelEntries.map((item, i) =>
                              i === index
                                ? {
                                    ...item,
                                    value: {
                                      ...item.value,
                                      name: event.target.value,
                                    },
                                  }
                                : item,
                            ),
                          )
                        }
                      />
                      </label>
                      <label className="min-w-0 flex-1 space-y-1">
                        <span className="text-xs text-muted-foreground">请求模型 ID</span>
                      <Input
                        aria-label={`模型 ${index + 1} ID`}
                        title="模型 ID（实际请求）"
                        placeholder="模型 ID，如 gpt-5.6-terra"
                        value={model.id}
                        className="w-full"
                        onChange={(event) =>
                          writeModels(
                            modelEntries.map((item, i) =>
                              i === index
                                ? { ...item, id: event.target.value }
                                : item,
                            ),
                          )
                        }
                      />
                      </label>
                      <Button
                        variant="ghost"
                        size="icon-sm"
                        aria-label={`编辑模型 ${model.id} 参数`}
                        onClick={() =>
                          setModelDraft({
                            index,
                            value: JSON.stringify(model.value, null, 2),
                          })
                        }
                      >
                        <Settings2 />
                      </Button>
                      <Button
                        variant="ghost"
                        size="icon-sm"
                        aria-label={`移除模型 ${model.id}`}
                        onClick={() =>
                          writeModels(
                            modelEntries.filter((_, i) => i !== index),
                          )
                        }
                      >
                        <Trash2 />
                      </Button>
                    </div>
                  ))}
                  {!modelEntries.length && (
                    <p className="p-4 text-sm text-muted-foreground">
                      添加模型，或在配置 JSON 中填写完整模型目录。
                    </p>
                  )}
                </div>
                {modelDraft && (
                  <div className="space-y-2 rounded-lg border border-border-default p-3">
                    <Label htmlFor="native-model-params">模型参数</Label>
                    {(() => {
                      try {
                        const parsed = JSON.parse(modelDraft.value);
                        return (
                          <NativeModelCapabilities
                            client={client}
                            value={record(parsed)}
                            onChange={(value) =>
                              setModelDraft({
                                ...modelDraft,
                                value: JSON.stringify(value, null, 2),
                              })
                            }
                          />
                        );
                      } catch {
                        return null;
                      }
                    })()}

                    <textarea
                      id="native-model-params"
                      rows={10}
                      className={editorClass}
                      value={modelDraft.value}
                      onChange={(event) =>
                        setModelDraft({
                          ...modelDraft,
                          value: event.target.value,
                        })
                      }
                    />
                    <div className="flex justify-end gap-2">
                      <Button
                        size="sm"
                        variant="ghost"
                        onClick={() => setModelDraft(null)}
                      >
                        取消
                      </Button>
                      <Button
                        size="sm"
                        onClick={() => {
                          try {
                            const value: unknown = JSON.parse(modelDraft.value);
                            if (
                              !value ||
                              typeof value !== "object" ||
                              Array.isArray(value)
                            )
                              throw Error("模型参数必须是 JSON 对象");
                            writeModels(
                              modelEntries.map((item, index) =>
                                index === modelDraft.index
                                  ? { ...item, value: value as Config }
                                  : item,
                              ),
                            );
                            setModelDraft(null);
                          } catch {
                            setError("模型参数必须是有效 JSON 对象");
                          }
                        }}
                      >
                        <Check />
                        应用参数
                      </Button>
                    </div>
                  </div>
                )}
              </section>
            )}
            <details
              className="rounded-lg border border-border-default"
              open={Boolean(error) || undefined}
            >
              <summary className="flex cursor-pointer items-center gap-2 px-4 py-3 text-sm font-medium">
                <Code2 className="size-4" />
                配置 JSON
              </summary>
              <div className="space-y-2 border-t border-border-default p-3">
                <p className="text-xs text-muted-foreground">
                  完整原生配置。模型能力、自定义请求头和其他参数会一并保存。
                </p>
                <textarea
                  aria-label="配置 JSON"
                  spellCheck={false}
                  rows={16}
                  className={editorClass}
                  value={raw}
                  onChange={(event) => parseRaw(event.target.value)}
                />
              </div>
            </details>
            {error && (
              <p role="alert" className="break-words text-sm text-danger-text">
                {error}
              </p>
            )}
          </div>
        </div>
        <DialogFooter className="m-0 flex flex-row items-center justify-end gap-2 rounded-none border-t border-border-default bg-bg-surface px-4 py-3 sm:px-6">
          <p className="me-auto hidden text-xs text-muted-foreground sm:block">
            添加后可在供应商列表中启用
          </p>
          <Button variant="outline" onClick={onClose} disabled={busy}>
            取消
          </Button>
          <Button
            onClick={save}
            disabled={busy || keyLoading || Boolean(error)}
          >
            {busy ? (
              <Loader2 className="animate-spin" />
            ) : form.id ? (
              <Check />
            ) : (
              <Plus />
            )}
            {form.id ? "保存" : "添加"}
          </Button>
        </DialogFooter>
      </DialogContent>
      {endpointsOpen && (
        <ProviderEndpoints
          current={fields.baseUrl}
          candidates={selectedPreset?.endpointCandidates || []}
          endpoints={form.metadata?.customEndpoints || {}}
          apiKey={fields.apiKey}
          protocol={discoveryProtocol}
          headers={record(
            client === "opencode"
              ? record(config.options).headers
              : config.headers,
          )}
          onChange={(customEndpoints) =>
            change({ metadata: { ...form.metadata, customEndpoints } })
          }
          onSelect={(url, customEndpoints) => {
            const settingsConfig = updateNativeField(
              client,
              config,
              "baseUrl",
              url,
            );
            setRaw(JSON.stringify(settingsConfig, null, 2));
            change({
              settingsConfig,
              metadata: { ...form.metadata, customEndpoints },
            });
            setEndpointsOpen(false);
          }}
          onClose={() => setEndpointsOpen(false)}
        />
      )}
      {accountsOpen && (
        <ManagedAccounts
          initialProvider={binding?.authProvider}
          onClose={() => setAccountsOpen(false)}
          onSelect={(authBinding) => {
            change({ metadata: { ...form.metadata, authBinding } });
            setAccountsOpen(false);
          }}
        />
      )}
    </Dialog>
  );
}

function Field({
  label,
  id,
  children,
}: {
  label: string;
  id: string;
  children: React.ReactNode;
}) {
  return (
    <div className="space-y-2">
      <Label htmlFor={id}>{label}</Label>
      {children}
    </div>
  );
}
