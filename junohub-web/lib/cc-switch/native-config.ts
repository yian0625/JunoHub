import { parse, stringify } from "smol-toml";
import catalog from "./presets.json";
import type { IntegrationClient, IntegrationProfileInput } from "@/lib/api";

export type Config = Record<string, unknown>;
export type Preset = {
  name: string;
  nameKey?: string;
  apiFormat?: string;
  endpointCandidates?: string[];
  modelCatalog?: Config[] | Config;
  codexChatReasoning?: Config;
  websiteUrl?: string;
  apiKeyUrl?: string;
  settingsConfig?: Config;
  auth?: Config;
  config?: string;
  providerKey?: string;
  icon?: string;
  iconColor?: string;
  category?: string;
  isOfficial?: boolean;
  requiresOAuth?: boolean;
  providerType?: string;
  theme?: { backgroundColor?: string; textColor?: string };
  templateValues?: Record<
    string,
    { label?: string; defaultValue?: string; placeholder?: string }
  >;
};
export const presets = catalog as Record<IntegrationClient, Preset[]>;
export const clientNames: Record<IntegrationClient, string> = {
  claude: "Claude Code",
  codex: "Codex",
  gemini: "Gemini CLI",
  grok: "Grok Build",
  opencode: "OpenCode",
  openclaw: "OpenClaw",
  hermes: "Hermes",
  pi: "Pi",
};
export const clientBrands: Record<IntegrationClient, string> = {
  claude: "claude",
  codex: "openai",
  gemini: "gemini",
  grok: "grok",
  opencode: "opencode",
  openclaw: "claw",
  hermes: "hermes",
  pi: "pi",
};
export const additiveClients: IntegrationClient[] = [
  "opencode",
  "openclaw",
  "hermes",
  "pi",
];
export function record(value: unknown): Config {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Config)
    : {};
}
export function str(value: unknown): string {
  return typeof value === "string" ? value : "";
}
export function toml(config: Config): Config {
  return parse(str(config.config));
}
export function nativeFields(client: IntegrationClient, config: Config) {
  const env = record(config.env);
  const options = record(config.options);
  let baseUrl = "",
    apiKey = "",
    model = "",
    protocol = "";
  if (client === "claude") {
    baseUrl = str(env.ANTHROPIC_BASE_URL);
    apiKey = str(env.ANTHROPIC_AUTH_TOKEN || env.ANTHROPIC_API_KEY);
    model = str(env.ANTHROPIC_MODEL);
  } else if (client === "gemini") {
    baseUrl = str(env.GOOGLE_GEMINI_BASE_URL);
    apiKey = str(env.GEMINI_API_KEY);
    model = str(env.GEMINI_MODEL);
  } else if (client === "codex" || client === "grok") {
    const doc = toml(config);
    if (client === "grok" && doc.models) {
      model = str(record(doc.models).default);
      const item = record(record(doc.model)[model]);
      baseUrl = str(item.base_url);
      apiKey = str(item.api_key);
      protocol = str(item.api_backend);
    } else {
      model = str(doc.model);
      const item = record(
        record(doc.model_providers)[str(doc.model_provider) || "custom"],
      );
      baseUrl = str(item.base_url);
      apiKey = str(
        item.experimental_bearer_token || record(config.auth).OPENAI_API_KEY,
      );
      protocol = str(item.wire_api);
    }
  } else if (client === "opencode") {
    baseUrl = str(options.baseURL);
    apiKey = str(options.apiKey);
    model = Object.keys(record(config.models))[0] || "";
    protocol = str(config.npm);
  } else {
    baseUrl = str(config.baseUrl || config.base_url);
    apiKey = str(config.apiKey || config.api_key);
    protocol = str(config.api || config.api_mode);
    model = Array.isArray(config.models)
      ? str(record(config.models[0]).id)
      : Object.keys(record(config.models))[0] || "";
  }
  return { baseUrl, apiKey, model, protocol };
}
export function updateNativeField(
  client: IntegrationClient,
  source: Config,
  field: "baseUrl" | "apiKey" | "model" | "protocol",
  value: string,
): Config {
  const config = structuredClone(source);
  if (client === "claude" || client === "gemini") {
    const env = record(config.env);
    config.env = env;
    const keys =
      client === "claude"
        ? {
            baseUrl: "ANTHROPIC_BASE_URL",
            apiKey:
              env.ANTHROPIC_API_KEY !== undefined
                ? "ANTHROPIC_API_KEY"
                : "ANTHROPIC_AUTH_TOKEN",
            model: "ANTHROPIC_MODEL",
            protocol: "",
          }
        : {
            baseUrl: "GOOGLE_GEMINI_BASE_URL",
            apiKey: "GEMINI_API_KEY",
            model: "GEMINI_MODEL",
            protocol: "",
          };
    if (keys[field]) {
      if (value) env[keys[field]] = value;
      else delete env[keys[field]];
    }
  } else if (client === "codex" || client === "grok") {
    const doc = toml(config);
    if (client === "grok" && doc.models) {
      const current = str(record(doc.models).default) || "custom";
      const models = record(doc.model);
      const item = record(models[current]);
      if (field === "model") {
        doc.models = { ...record(doc.models), default: value };
        delete models[current];
        models[value] = { ...item, model: value };
      } else {
        item[
          { baseUrl: "base_url", apiKey: "api_key", protocol: "api_backend" }[
            field
          ]
        ] = value;
        models[current] = item;
      }
      doc.model = models;
    } else {
      const id = str(doc.model_provider) || "custom";
      const providers = record(doc.model_providers);
      const item = record(providers[id]);
      if (field === "apiKey") {
        config.auth = { ...record(config.auth), OPENAI_API_KEY: value };
        if ("experimental_bearer_token" in item)
          item.experimental_bearer_token = value;
      } else if (field === "model") doc.model = value;
      else {
        item[field === "baseUrl" ? "base_url" : "wire_api"] = value;
      }
      providers[id] = item;
      doc.model_providers = providers;
      doc.model_provider = id;
    }
    config.config = stringify(doc);
  } else if (client === "opencode") {
    if (field === "protocol") config.npm = value;
    else if (field !== "model")
      config.options = {
        ...record(config.options),
        [field === "baseUrl" ? "baseURL" : "apiKey"]: value,
      };
  } else {
    const keys =
      client === "hermes"
        ? {
            baseUrl: "base_url",
            apiKey: "api_key",
            protocol: "api_mode",
            model: "model",
          }
        : {
            baseUrl: "baseUrl",
            apiKey: "apiKey",
            protocol: "api",
            model: "model",
          };
    if (field !== "model") config[keys[field]] = value;
  }
  return config;
}
export function emptyNative(client: IntegrationClient): Config {
  if (client === "claude" || client === "gemini") return { env: {} };
  if (client === "codex" || client === "grok")
    return {
      auth: { OPENAI_API_KEY: "" },
      config:
        'model_provider = "custom"\nmodel = ""\n\n[model_providers.custom]\nname = "Custom"\nbase_url = ""\nwire_api = "responses"\nrequires_openai_auth = true\n',
    };
  if (client === "opencode")
    return {
      npm: "@ai-sdk/openai-compatible",
      options: { baseURL: "", apiKey: "" },
      models: {},
    };
  if (client === "hermes")
    return {
      name: "",
      base_url: "",
      api_key: "",
      api_mode: "chat_completions",
      models: [],
    };
  return { baseUrl: "", apiKey: "", api: "openai-completions", models: [] };
}
export function createNativeForm(
  client: IntegrationClient,
  preset?: Preset,
  presetId = "custom",
): IntegrationProfileInput {
  const settingsConfig = structuredClone(
    preset?.settingsConfig ||
      (preset?.config !== undefined
        ? { auth: preset.auth || {}, config: preset.config }
        : emptyNative(client)),
  );
  if (preset?.modelCatalog)
    settingsConfig.modelCatalog = structuredClone(preset.modelCatalog);
  const fields = nativeFields(client, settingsConfig);
  return {
    name: preset?.name || "",
    providerType: "custom_gateway",
    keyId: 0,
    apps: Object.fromEntries(
      Object.keys(clientNames).map((key) => [key, key === client]),
    ) as IntegrationProfileInput["apps"],
    models: { [client]: { model: fields.model } },
    baseUrl: fields.baseUrl,
    apiKey: fields.apiKey,
    websiteUrl: preset?.websiteUrl || "",
    notes: "",
    settingsConfig,
    metadata: {
      scope: "client",
      nativeClient: client,
      apiFormat: preset?.apiFormat,
      authBinding:
        preset?.requiresOAuth && preset.providerType
          ? { authProvider: preset.providerType }
          : undefined,
      codexChatReasoning: preset?.codexChatReasoning,
      providerKey:
        preset?.providerKey ||
        str(settingsConfig.name) ||
        preset?.name
          .toLowerCase()
          .replace(/[^a-z0-9]+/g, "-")
          .replace(/^-|-$/g, "") ||
        "",
      category: preset?.category || "custom",
      icon: preset?.icon,
      presetId,
    },
  };
}
export function nativePayload(
  form: IntegrationProfileInput,
): IntegrationProfileInput {
  const client = form.metadata!.nativeClient!;
  const fields = nativeFields(client, form.settingsConfig!);
  return {
    ...form,
    baseUrl: fields.baseUrl,
    apiKey: fields.apiKey,
    models: { [client]: { model: fields.model, protocol: fields.protocol } },
  };
}

export function updateClaudeAuth(config: Config, field: string): Config {
  const env = { ...record(config.env) };
  const token = str(env.ANTHROPIC_AUTH_TOKEN || env.ANTHROPIC_API_KEY);
  delete env.ANTHROPIC_AUTH_TOKEN;
  delete env.ANTHROPIC_API_KEY;
  env[field] = token;
  return { ...config, env };
}
export function updateCodexOption(
  config: Config,
  key: string,
  value: string,
): Config {
  const doc = toml(config);
  if (value) doc[key] = value;
  else delete doc[key];
  return { ...config, config: stringify(doc) };
}
