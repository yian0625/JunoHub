"use client";

import { useCallback, useEffect, useId, useRef, useState } from "react";
import {
  AlertCircle,
  Activity,
  ArrowUpCircle,
  Check,
  CheckCircle2,
  Copy,
  Download,
  Edit2,
  Eye,
  EyeOff,
  GripVertical,
  Layers,
  Loader2,
  Network,
  Plus,
  RefreshCw,
  Trash2,
  X,
} from "lucide-react";
import { toast } from "sonner";

import {
  ErrorState,
  LoadingState,
  PageHead,
} from "../../_components/console-ui";
import { useConsoleGuard } from "../../_components/use-console-guard";
import { ProxyControls } from "./proxy-controls";
import { LocalProxyPanel } from "./local-proxy-panel";
import { ConfirmDialog } from "@/components/confirm-dialog";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { SelectField } from "@/components/ui/select-field";
import { RadioGroupPrimitive, RadioPrimitive } from "@/components/ui/radio-group";
import { segmentedControlItemVariants, segmentedControlRootClassName } from "@/lib/segmented-control";
import { cn } from "@/lib/utils";
import {
  consoleAPI,
  type ConsoleKey,
  type AvailableModel,
  type IntegrationModels,
  type IntegrationClient,
  type IntegrationProfile,
  type IntegrationProfileInput,
} from "@/lib/api";
import { useAuthStore } from "@/lib/store";

import { LocalUsageDialog } from "./local-usage-dialog";
import { ChartNoAxesColumn } from "lucide-react";
import { NativeProviderPanel, ProviderBrand } from "./native-provider-panel";
import {
  additiveClients,
  clientBrands,
  createNativeForm,
  nativePayload,
  updateNativeField,
} from "@/lib/cc-switch/native-config";

type ClientKind = IntegrationClient;
type ToolKind = ClientKind | "grok" | "opencode" | "openclaw" | "hermes" | "pi";
type ToolStatus = {
  client: ToolKind;
  label: string;
  current_version: string | null;
  latest_version: string | null;
  installed_but_broken: boolean;
  error: string | null;
  env_type: "windows" | "wsl" | "macos" | "linux" | "unknown";
  wsl_distro: string | null;
  installations: Array<{
    source: string;
    path: string;
    version: string | null;
    runnable: boolean;
    is_path_default: boolean;
  }>;
};
type NativeResult = {
  action: "inspect" | "install" | "update" | "connect";
  success: boolean;
  tools: ToolStatus[];
  error?: string;
  backup_id?: string;
  active_providers: Record<string, ActiveProvider>;
};
type ActiveProvider = { id: string; name: string; backup_id: string };
type FormState = IntegrationProfileInput;
const LOCAL_HELPER_URL = "http://127.0.0.1:17372";

async function callLocalHelper(
  path: "/tool" | "/connect" | "/adopt" | "/remove-provider",
  body: Record<string, string>,
): Promise<NativeResult> {
  const response = await fetch(`${LOCAL_HELPER_URL}${path}`, {
    method: "POST",
    cache: "no-store",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  if (!response.ok) throw new Error(`本机组件返回 HTTP ${response.status}`);
  const result = (await response.json()) as NativeResult;
  if (!result.success) throw new Error(result.error || "本机操作失败");
  return result;
}
const clientItems: Array<{ value: ClientKind; label: string }> = [
  { value: "claude", label: "Claude Code" },
  { value: "codex", label: "Codex" },
  { value: "gemini", label: "Gemini CLI" },
  { value: "grok", label: "Grok Build" },
  { value: "opencode", label: "OpenCode" },
  { value: "openclaw", label: "OpenClaw" },
  { value: "hermes", label: "Hermes" },
  { value: "pi", label: "Pi" },
];
const toolItems: Array<{ value: ToolKind; label: string }> = [...clientItems];
const protocolOptions: Partial<
  Record<ClientKind, Array<{ value: string; label: string }>>
> = {
  grok: [
    { value: "responses", label: "Responses API" },
    { value: "chat", label: "Chat Completions" },
  ],
  opencode: [
    { value: "@ai-sdk/openai", label: "OpenAI Responses" },
    { value: "@ai-sdk/openai-compatible", label: "OpenAI Compatible" },
    { value: "@ai-sdk/anthropic", label: "Anthropic" },
  ],
  openclaw: [
    { value: "openai-responses", label: "OpenAI Responses" },
    { value: "openai-completions", label: "OpenAI Chat Completions" },
    { value: "anthropic-messages", label: "Anthropic Messages" },
  ],
  hermes: [
    { value: "codex_responses", label: "Codex Responses" },
    { value: "chat_completions", label: "Chat Completions" },
    { value: "anthropic_messages", label: "Anthropic Messages" },
  ],
  pi: [
    { value: "openai-responses", label: "OpenAI Responses" },
    { value: "openai-completions", label: "OpenAI Chat Completions" },
    { value: "anthropic-messages", label: "Anthropic Messages" },
  ],
};

function defaultProtocol(client: ClientKind) {
  return protocolOptions[client]?.[0]?.value;
}

function configuredProtocol(models: IntegrationModels, client: ClientKind) {
  return (models[client] as { protocol?: string } | undefined)?.protocol;
}

function decodeNativeResult(): NativeResult | null {
  const prefix = "#junohub-integration=";
  if (!window.location.hash.startsWith(prefix)) return null;
  try {
    const encoded = window.location.hash
      .slice(prefix.length)
      .replaceAll("-", "+")
      .replaceAll("_", "/");
    const padded = encoded.padEnd(Math.ceil(encoded.length / 4) * 4, "=");
    return JSON.parse(
      new TextDecoder().decode(
        Uint8Array.from(atob(padded), (character) => character.charCodeAt(0)),
      ),
    ) as NativeResult;
  } catch {
    return null;
  }
}

function hasUpdate(status: ToolStatus) {
  if (!status.current_version || !status.latest_version) return false;
  const current =
    status.current_version.match(/\d+/g)?.slice(0, 3).map(Number) || [];
  const latest =
    status.latest_version.match(/\d+/g)?.slice(0, 3).map(Number) || [];
  for (
    let index = 0;
    index < Math.max(current.length, latest.length);
    index += 1
  ) {
    if ((latest[index] || 0) !== (current[index] || 0))
      return (latest[index] || 0) > (current[index] || 0);
  }
  return false;
}

export function ConnectWorkspace({
  section,
}: {
  section: "providers" | "clients";
}) {
  const ready = useConsoleGuard();
  const userId = useAuthStore((state) => state.user?.uid);
  const [keys, setKeys] = useState<ConsoleKey[]>([]);
  const [models, setModels] = useState<AvailableModel[]>([]);
  const [profiles, setProfiles] = useState<IntegrationProfile[]>([]);
  const [form, setForm] = useState<FormState | null>(null);
  const [loading, setLoading] = useState(true);
  const [inspection, setInspection] = useState<{client: string; status: "loading" | "ready" | "error"} | null>(null);
  const [inspectionRetry, setInspectionRetry] = useState(0);
  const [usageOpen, setUsageOpen] = useState(false);
  const [proxyOpen, setProxyOpen] = useState<false | "settings" | "queue">(
    false,
  );
  const [busy, setBusy] = useState("");
  const [inspectionError, setInspectionError] = useState("");
  const [loadError, setLoadError] = useState("");
  const [toolStatuses, setToolStatuses] = useState<Record<string, ToolStatus>>(
    {},
  );
  const [activeProviders, setActiveProviders] = useState<
    Record<string, ActiveProvider>
  >({});
  const [selectedClient, setSelectedClient] = useState<ClientKind>("claude");
  const [draggedProfileId, setDraggedProfileId] = useState<string | null>(null);
  const [removeConfirm, setRemoveConfirm] = useState<IntegrationProfile | null>(
    null,
  );
  const [deleteConfirm, setDeleteConfirm] = useState<IntegrationProfile | null>(
    null,
  );

  const load = useCallback(async () => {
    setLoading(true);
    setLoadError("");
    try {
      const [keyResult, modelResult, profileResult] = await Promise.all([
        consoleAPI.keyOwnList(),
        consoleAPI.availableModels(),
        consoleAPI.integrationList(),
      ]);
      const now = Math.floor(Date.now() / 1000);
      setKeys(
        keyResult.list.filter(
          (key) =>
            String(key.owner_id) === String(userId) &&
            key.status === 1 &&
            key.can_reveal &&
            (!key.expires_at || key.expires_at > now),
        ),
      );
      setModels(modelResult.list);
      setProfiles(profileResult.list);
    } catch (error) {
      const message = error instanceof Error ? error.message : "无法加载供应商";
      setLoadError(message);
      toast.error(message);
    } finally {
      setLoading(false);
    }
  }, [userId]);

  useEffect(() => {
    if (ready && userId !== undefined) void load();
  }, [ready, userId, load]);
  useEffect(() => {
    const result = decodeNativeResult();
    if (!result) return;
    if (result.tools.length)
      setToolStatuses((current) => ({
        ...current,
        ...Object.fromEntries(result.tools.map((tool) => [tool.client, tool])),
      }));
    setActiveProviders(result.active_providers || {});
    if (result.success)
      toast.success(
        result.action === "connect"
          ? "供应商配置已应用"
          : result.action === "inspect"
            ? "客户端检测完成"
            : result.action === "install"
              ? "客户端安装完成"
              : "客户端更新完成",
        result.backup_id
          ? { description: `备份编号：${result.backup_id}` }
          : undefined,
      );
    else
      toast.error("本机操作失败", {
        description: result.error || "请查看本机组件日志",
      });
    window.history.replaceState(
      null,
      "",
      `${window.location.pathname}${window.location.search}`,
    );
  }, []);
  useEffect(() => {
    if (section !== "providers" || proxyOpen) return;
    const controller = new AbortController();
    setInspection({client: selectedClient, status: "loading"});
    void fetch(`${LOCAL_HELPER_URL}/configuration-status?client=${selectedClient}`, {
      cache: "no-store",
      signal: controller.signal,
    })
      .then((response) => (response.ok ? response.json() : null))
      .then((result: NativeResult | null) => {
        if (controller.signal.aborted) return;
        if (result?.success) {
          setActiveProviders(result.active_providers || {});
          setInspection({client: selectedClient, status: "ready"});
        } else setInspection({client: selectedClient, status: "error"});
      })
      .catch(() => {
        if (!controller.signal.aborted) setInspection({client: selectedClient, status: "error"});
      });
    return () => controller.abort();
  }, [section, selectedClient, proxyOpen, inspectionRetry]);
  const inspectTools = async () => {
    if (busy) return;
    setBusy("inspect");
    setInspectionError("");
    const controller = new AbortController();
    const timeout = window.setTimeout(() => controller.abort(), 30_000);
    try {
      const response = await fetch(`${LOCAL_HELPER_URL}/inspect?client=all`, {
        cache: "no-store",
        signal: controller.signal,
      });
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      const result = (await response.json()) as NativeResult;
      if (!result.success) throw new Error(result.error || "检测失败");
      setToolStatuses((current) => ({
        ...current,
        ...Object.fromEntries(result.tools.map((tool) => [tool.client, tool])),
      }));
      setActiveProviders(result.active_providers || {});
      toast.success("客户端检测完成");
    } catch (error) {
      const message = error instanceof Error && error.name === "AbortError"
        ? "检测请求超时，请稍后重试。已显示的检测结果会保留。"
        : error instanceof TypeError
          ? "无法连接本机检测组件，请确认组件正在运行后重试。"
          : error instanceof Error ? error.message : "检测失败，请重试。";
      setInspectionError(message);
      toast.error("检测未完成", { description: message });
    } finally {
      window.clearTimeout(timeout);
      setBusy("");
    }
  };
  const runToolAction = async (
    target: string,
    action: "install" | "update",
  ) => {
    setBusy(`${target}-${action}`);
    try {
      const result = await callLocalHelper("/tool", { client: target, action });
      setToolStatuses((current) => ({
        ...current,
        ...Object.fromEntries(result.tools.map((tool) => [tool.client, tool])),
      }));
      setActiveProviders(result.active_providers || {});
      toast.success(action === "install" ? "客户端安装完成" : "客户端更新完成");
    } catch (error) {
      toast.error(action === "install" ? "客户端安装失败" : "客户端更新失败", {
        description:
          error instanceof TypeError
            ? "无法连接本机组件，请重新运行 ./deploy.sh client"
            : error instanceof Error
              ? error.message
              : "本机操作失败",
      });
    } finally {
      setBusy("");
    }
  };
  const syncProfile = async (
    id: string,
    client?: ClientKind,
    propagateError = false,
  ) => {
    setBusy(`sync-${id}`);
    try {
      const pairing = await consoleAPI.integrationSync(id, client);
      const result = await callLocalHelper("/connect", {
        pair_url: `${window.location.origin}/client-connect/redeem?token=${pairing.token}`,
      });
      setActiveProviders(result.active_providers || {});
      toast.success("供应商配置已应用", {
        description: result.backup_id
          ? `备份编号：${result.backup_id}`
          : undefined,
      });
    } catch (error) {
      toast.error("无法应用供应商配置", {
        description:
          error instanceof TypeError
            ? "无法连接本机组件，请重新运行 ./deploy.sh client"
            : error instanceof Error
              ? error.message
              : "同步配置失败",
      });
      if (propagateError) throw error;
    } finally {
      setBusy("");
    }
  };
  const saveProfile = async (sync: boolean) => {
    if (!form) return;
    setBusy("save");
    try {
      const saved = await consoleAPI.integrationUpsert(
        form.settingsConfig ? nativePayload(form) : form,
      );
      const editing = Boolean(form.id);
      setForm(null);
      await load();
      let applying = false;
      const nativeClient = form.metadata?.nativeClient;
      if (
        nativeClient &&
        form.id &&
        (activeProviders[nativeClient]?.id === form.id ||
          activeProviders[`${nativeClient}:${form.id}`])
      ) {
        applying = true;
        await syncProfile(saved.id, nativeClient);
      } else if (sync) {
        for (const item of clientItems)
          if (
            form.apps[item.value] &&
            activeProviders[item.value]?.id === saved.id
          ) {
            applying = true;
            await syncProfile(saved.id, item.value);
          }
      }
      if (!applying) toast.success(editing ? "供应商已更新" : "供应商已添加");
    } catch (error) {
      toast.error(error instanceof Error ? error.message : "保存失败");
    } finally {
      setBusy("");
    }
  };
  const junoHubForm = async (
    existing?: IntegrationProfile,
  ): Promise<FormState> => {
    const keyId =
      existing?.keyId || keys.find((key) => key.status === 1)?.id || 0;
    const next = createNativeForm(selectedClient);
    next.name = existing?.name || "JunoHub";
    next.id = existing?.id;
    next.notes = existing?.notes || "";
    next.websiteUrl = window.location.origin;
    next.metadata = {
      ...next.metadata,
      ...existing?.metadata,
      scope: "client",
      nativeClient: selectedClient,
      category: "custom",
      presetId: "junohub",
      icon: "junohub",
      providerKey: existing?.metadata?.providerKey || "junohub",
      junohubKeyId: keyId,
    };
    let config = updateNativeField(
      selectedClient,
      next.settingsConfig!,
      "baseUrl",
      window.location.origin +
        (selectedClient === "claude" || selectedClient === "gemini"
          ? ""
          : "/v1"),
    );
    if (keyId)
      config = updateNativeField(
        selectedClient,
        config,
        "apiKey",
        (await consoleAPI.keyReveal(keyId)).key,
      );
    const model = existing?.models[selectedClient]?.model || "";
    if (model) {
      config = updateNativeField(selectedClient, config, "model", model);
      if (additiveClients.includes(selectedClient))
        config.models =
          selectedClient === "opencode"
            ? { [model]: { name: model } }
            : [{ id: model, name: model }];
    }
    next.settingsConfig = config;
    return next;
  };
  const editProfile = async (profile: IntegrationProfile) => {
    setBusy(`edit-${profile.id}`);
    try {
      if (profile.providerType === "junohub" && !profile.hasNativeConfig) {
        setForm(await junoHubForm(profile));
        return;
      }
      const detail = profile.hasNativeConfig
        ? await consoleAPI.integrationDetail(profile.id)
        : null;
      setForm({
        ...profile,
        apps: Object.fromEntries(
          clientItems.map((item) => [
            item.value,
            item.value === selectedClient,
          ]),
        ) as FormState["apps"],
        metadata: {
          ...profile.metadata,
          scope: "client",
          nativeClient: selectedClient,
        },
        models: structuredClone(profile.models),
        ...(detail?.settingsConfig
          ? { settingsConfig: detail.settingsConfig }
          : {}),
      });
    } catch (error) {
      toast.error(
        error instanceof Error ? error.message : "无法读取供应商配置",
      );
    } finally {
      setBusy("");
    }
  };
  const duplicateProfile = async (profile: IntegrationProfile) => {
    setBusy(`duplicate-${profile.id}`);
    try {
      const detail = profile.hasNativeConfig
        ? await consoleAPI.integrationDetail(profile.id)
        : null;
      const copy: FormState = {
        ...profile,
        ...(detail?.settingsConfig
          ? { settingsConfig: detail.settingsConfig }
          : {}),
        metadata: {
          ...profile.metadata,
          providerKey: profile.metadata?.providerKey
            ? `${profile.metadata.providerKey}-copy-${Date.now()}`
            : undefined,
        },
        id: undefined,
        copyFromId: profile.id,
        name: `${profile.name} 副本`,
        apps: { ...profile.apps },
        models: structuredClone(profile.models),
      };
      await consoleAPI.integrationUpsert(
        copy.settingsConfig ? nativePayload(copy) : copy,
      );
      await load();
      toast.success("供应商已复制");
    } catch (error) {
      toast.error(error instanceof Error ? error.message : "复制供应商失败");
    } finally {
      setBusy("");
    }
  };
  const testingProfile = useRef(false);
  const testProfile = async (
    profile: IntegrationProfile,
    client: ClientKind,
  ) => {
    if (testingProfile.current) return;
    testingProfile.current = true;
    const toastId = "integration-connection-test";
    setBusy(`test-${profile.id}`);
    toast.loading(`正在检测 ${profile.name}`, { id: toastId });
    try {
      if (profile.hasNativeConfig || profile.metadata?.authBinding) {
        const pair = await consoleAPI.integrationSync(profile.id, client);
        const response = await fetch("http://127.0.0.1:17372/proxy/probe", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({
            client,
            pairUrls: [
              `${window.location.origin}/client-connect/redeem?token=${pair.token}`,
            ],
          }),
          signal: AbortSignal.timeout(45000),
        });
        const result = await response.json();
        if (!result.success) throw Error(result.error || "检测失败");
        const check = result.results?.[0];
        if (check?.outcome === "unsupported") {
          toast.info("当前配置暂不支持直接检测", {
            id: toastId,
            description: check.error,
          });
          return;
        }
        if (!check?.success)
          throw Error(
            check?.error ||
              (check?.status
                ? `上游返回 HTTP ${check.status}`
                : "未收到检测结果"),
          );
        toast.success("连接正常", {
          id: toastId,
          description: `${check.modelCount != null ? `已读取 ${check.modelCount} 个模型，` : `HTTP ${check.status}，`}耗时 ${check.latencyMs} ms`,
        });
        return;
      }
      const result = await consoleAPI.integrationTest(profile.id, client);
      if (result.ok)
        toast.success("连接正常", { id: toastId, description: result.message });
      else if (result.outcome === "warning")
        toast.info("当前配置暂不支持直接检测", {
          id: toastId,
          description: result.message,
        });
      else
        toast.error("连接失败", { id: toastId, description: result.message });
    } catch (error) {
      toast.error("检测未完成", {
        id: toastId,
        description:
          error instanceof TypeError
            ? "无法连接本机组件，请确认组件正在运行。"
            : error instanceof Error
              ? error.message
              : "连接检测失败",
      });
    } finally {
      testingProfile.current = false;
      setBusy("");
    }
  };
  const reorderProfile = async (targetId: string) => {
    if (!draggedProfileId || draggedProfileId === targetId) return;
    const previous = profiles;
    const from = previous.findIndex((item) => item.id === draggedProfileId);
    const to = previous.findIndex((item) => item.id === targetId);
    if (from < 0 || to < 0) return;
    const next = [...previous];
    const [moved] = next.splice(from, 1);
    next.splice(to, 0, moved);
    setProfiles(next);
    setDraggedProfileId(null);
    try {
      await consoleAPI.integrationReorder(next.map((item) => item.id));
    } catch (error) {
      setProfiles(previous);
      toast.error(error instanceof Error ? error.message : "保存排序失败");
    }
  };
  const deleteProfile = async () => {
    if (!deleteConfirm) return;
    if (
      Object.values(activeProviders).some(
        (active) => active.id === deleteConfirm.id,
      )
    ) {
      toast.error("请先在使用此供应商的客户端中切换或移除配置");
      setDeleteConfirm(null);
      return;
    }
    try {
      await consoleAPI.integrationDelete(deleteConfirm.id);
      setProfiles((current) =>
        current.filter((item) => item.id !== deleteConfirm.id),
      );
      toast.success("供应商已删除");
    } catch (error) {
      toast.error(error instanceof Error ? error.message : "删除供应商失败");
    } finally {
      setDeleteConfirm(null);
    }
  };
  const copyBootstrap = async () => {
    try {
      await navigator.clipboard.writeText("./deploy.sh client");
      toast.success("安装命令已复制");
    } catch {
      toast.error("无法复制安装命令");
    }
  };

  if (loading) return <LoadingState />;
  if (loadError) return <ErrorState message={loadError} onRetry={load} />;
  const statuses = toolItems.map((item) => ({
    item,
    status: toolStatuses[item.value],
  }));
  const missing = statuses
    .filter(
      ({ status }) =>
        status && !status.current_version && !status.installed_but_broken,
    )
    .map(({ item }) => item.value);
  const updates = statuses
    .filter(({ status }) => status && hasUpdate(status))
    .map(({ item }) => item.value);
  const visibleProfiles = profiles.filter(
    (profile) => profile.apps[selectedClient],
  );
  const createForSelectedClient = () => {
    setForm(createNativeForm(selectedClient));
  };

  return (
    <div className="space-y-6">
      {section === "providers" ? (
        <>
          <PageHead
            title="客户端配置"
            description="为客户端添加、编辑和切换供应商配置。"
            action={
              <div className="grid grid-flow-col auto-cols-fr gap-1 [&>button]:w-full sm:flex sm:items-center sm:gap-2 sm:[&>button]:w-10">
                <Button
                  variant="outline"
                  size="icon"
                  aria-label="代理设置"
                  title="代理设置"
                  onClick={() => setProxyOpen("settings")}
                >
                  <Network />
                </Button>
                <Button
                  variant="outline"
                  size="icon"
                  aria-label="本地用量"
                  title="本地用量"
                  onClick={() => setUsageOpen(true)}
                >
                  <ChartNoAxesColumn />
                </Button>
                <Button
                  size="icon"
                  onClick={createForSelectedClient}
                  disabled={Boolean(busy)}
                  aria-label="添加供应商"
                  title="添加供应商"
                >
                  <Plus />
                </Button>
              </div>
            }
          />
          <section className="space-y-5" aria-labelledby="providers-title">
            <div className="border-b border-border-default pb-5">
              <ClientSwitcher
                value={selectedClient}
                onChange={setSelectedClient}
              />
            </div>
            <div className="flex flex-wrap items-center justify-between gap-2">
              <div className="flex items-center gap-2">
                <h2
                  id="providers-title"
                  className="text-sm font-medium text-muted-foreground"
                >
                  {
                    clientItems.find((item) => item.value === selectedClient)
                      ?.label
                  }
                </h2>
                <span className="rounded-full bg-bg-sunken px-2 py-0.5 text-xs tabular-nums text-muted-foreground">
                  {visibleProfiles.length}
                </span>
              </div>
              <ProxyControls
                key={selectedClient}
                client={selectedClient}
                activeId={
                  activeProviders[selectedClient]?.id ??
                  (profiles.filter(
                    (profile) =>
                      activeProviders[`${selectedClient}:${profile.id}`],
                  ).length === 1
                    ? profiles.find(
                        (profile) =>
                          activeProviders[`${selectedClient}:${profile.id}`],
                      )?.id
                    : undefined)
                }
                onQueue={() => setProxyOpen("queue")}
                onChange={() => {
                  void fetch(
                    `http://127.0.0.1:17372/configuration-status?client=${selectedClient}`,
                  )
                    .then((response) => response.json())
                    .then((data) => {
                      if (data.active_providers)
                        setActiveProviders(data.active_providers);
                    })
                    .catch(() => {});
                }}
              />
            </div>
            {(inspection?.client !== selectedClient || inspection.status === "loading") ? <LoadingState /> : !visibleProfiles.length ? (
              <div className="flex flex-col items-center justify-center rounded-xl border border-dashed border-border-default py-14 text-center">
                <Layers className="mb-3 size-9 text-muted-foreground/50" />
                <p className="text-sm font-medium">该客户端还没有供应商</p>
                <p className="mt-1 text-xs text-muted-foreground">
                  添加后即可切换当前配置
                </p>
                <Button className="mt-5" onClick={createForSelectedClient}>
                  添加供应商
                </Button>
              </div>
            ) : (
              <div className="space-y-3">
                {visibleProfiles.map((profile) => (
                  <ProviderCard
                    key={profile.id}
                    profile={profile}
                    client={selectedClient}
                    active={
                      activeProviders[selectedClient]?.id === profile.id ||
                      Boolean(
                        activeProviders[`${selectedClient}:${profile.id}`],
                      )
                    }
                    inspectionStatus={inspection?.client === selectedClient ? inspection.status : "loading"}
                    onRetryInspection={() => setInspectionRetry((value) => value + 1)}
                    dragging={draggedProfileId === profile.id}
                    usageKey={keys.find((key) => key.id === profile.keyId)}
                    busy={busy}
                    onSync={(value) =>
                      void syncProfile(value.id, selectedClient)
                    }
                    onTest={(value) => void testProfile(value, selectedClient)}
                    onDragStart={() => setDraggedProfileId(profile.id)}
                    onDragEnd={() => setDraggedProfileId(null)}
                    onDrop={() => void reorderProfile(profile.id)}
                    onDuplicate={duplicateProfile}
                    onEdit={(value) => void editProfile(value)}
                    onRemove={(value) => setRemoveConfirm(value)}
                    onDelete={(value) => setDeleteConfirm(value)}
                  />
                ))}
              </div>
            )}
          </section>
        </>
      ) : (
        <>
          <PageHead
            title="客户端安装"
            description="检测本机客户端版本，并安装缺失项或升级现有工具。"
          />
          <Card
            role="region"
            className="gap-0 py-0"
            aria-labelledby="tools-title"
          >
            <div className="flex flex-col gap-3 border-b border-border-default px-5 py-4 sm:flex-row sm:items-center sm:justify-between sm:px-6">
              <div>
                <h2 id="tools-title" className="text-base font-semibold">
                  客户端安装
                </h2>
                <p className="mt-1 text-sm text-muted-foreground">
                  检测本机安装及版本，按客户端安装或升级。
                </p>
              </div>
              <div className="flex flex-wrap gap-2">
                {missing.length > 0 && (
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={Boolean(busy)}
                    onClick={() => runToolAction(missing.join(","), "install")}
                  >
                    <Download />
                    安装缺失项
                  </Button>
                )}
                {updates.length > 0 && (
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={Boolean(busy)}
                    onClick={() => runToolAction(updates.join(","), "update")}
                  >
                    <ArrowUpCircle />
                    更新全部
                  </Button>
                )}
                <Button
                  variant="outline"
                  size="sm"
                  disabled={Boolean(busy)}
                  onClick={() => void inspectTools()}
                >
                  {busy === "inspect" ? (
                    <Loader2 className="animate-spin" />
                  ) : (
                    <RefreshCw />
                  )}
                  检测
                </Button>
              </div>
            </div>
            {inspectionError && <div role="alert" className="mx-5 mt-4 rounded-lg bg-danger-subtle px-3 py-2.5 text-sm text-danger-text sm:mx-6">{inspectionError}</div>}
            <div className="grid gap-3 p-5 sm:grid-cols-2 xl:grid-cols-4 sm:p-6">
              {statuses.map(({ item, status }) => (
                <ToolCard
                  key={item.value}
                  item={item}
                  status={status}
                  busy={busy}
                  onAction={runToolAction}
                />
              ))}
            </div>
            <div className="flex flex-col gap-3 border-t border-border-default px-5 py-4 sm:flex-row sm:items-center sm:justify-between sm:px-6">
              <Button variant="outline" size="sm" onClick={copyBootstrap}>
                <Download />
                复制组件安装命令
              </Button>
            </div>
          </Card>
        </>
      )}
      {usageOpen && (
        <LocalUsageDialog
          initialClient={selectedClient}
          onClose={() => setUsageOpen(false)}
        />
      )}
      {proxyOpen && (
        <LocalProxyPanel
          mode={proxyOpen}
          client={selectedClient}
          profiles={profiles}
          onClose={() => setProxyOpen(false)}
        />
      )}
      {form?.settingsConfig && form.metadata?.scope === "client" ? (
        <NativeProviderPanel
          key={
            form.id ||
            `new-${selectedClient}-${form.metadata?.presetId === "junohub" ? "junohub" : "native"}`
          }
          form={form}
          keys={keys}
          busy={busy === "save"}
          onChange={setForm}
          onClose={() => setForm(null)}
          onSave={() => void saveProfile(false)}
          onJunoHub={() => {
            setBusy("junohub-preset");
            void junoHubForm()
              .then(setForm)
              .catch((error) =>
                toast.error(
                  error instanceof Error ? error.message : "读取访问密钥失败",
                ),
              )
              .finally(() => setBusy(""));
          }}
        />
      ) : (
        <IntegrationDialog
          form={form}
          client={selectedClient}
          keys={keys}
          models={models}
          busy={busy === "save"}
          onChange={setForm}
          onClose={() => setForm(null)}
          onSave={(sync) => void saveProfile(sync)}
        />
      )}
      <ConfirmDialog
        open={Boolean(removeConfirm)}
        onOpenChange={(open) => !open && setRemoveConfirm(null)}
        title="从客户端配置中移除供应商"
        description={`从 ${clientItems.find((item) => item.value === selectedClient)?.label} 的配置中移除“${removeConfirm?.name || ""}”，保留已保存的供应商。`}
        confirmText="移除"
        onConfirm={() => {
          if (!removeConfirm) return;
          setBusy(`remove-${removeConfirm.id}`);
          void callLocalHelper("/remove-provider", {
            client: selectedClient,
            id: removeConfirm.id,
            provider_key:
              removeConfirm.metadata?.providerKey ||
              `junohub-${removeConfirm.id}`,
          })
            .then((result) => {
              setActiveProviders(result.active_providers || {});
              setRemoveConfirm(null);
              toast.success("已从客户端配置中移除");
            })
            .catch((error) =>
              toast.error(error instanceof Error ? error.message : "移除失败"),
            )
            .finally(() => setBusy(""));
        }}
      />
      <ConfirmDialog
        open={Boolean(deleteConfirm)}
        onOpenChange={(open) => {
          if (!open) setDeleteConfirm(null);
        }}
        title="删除供应商"
        description={`确定要删除“${deleteConfirm?.name || ""}”吗？相关客户端将不再能切换到该供应商。`}
        confirmText="删除"
        destructive
        onConfirm={() => void deleteProfile()}
      />
    </div>
  );
}

function ClientSwitcher({
  value,
  onChange,
}: {
  value: ClientKind;
  onChange: (value: ClientKind) => void;
}) {
  return (
    <div className="max-w-full min-w-0 overflow-x-auto p-0.5">
    <RadioGroupPrimitive value={value} onValueChange={next => onChange(next as ClientKind)} className={segmentedControlRootClassName} aria-label="选择客户端">
      {clientItems.map((item) => {
        return (
          <RadioPrimitive.Root
            key={item.value}
            value={item.value}
            aria-label={item.label}
            className={segmentedControlItemVariants({ className: "grow", state: "checked" })}
          >
            <ProviderBrand
              icon={clientBrands[item.value]}
              name={item.label}
              size={16}
            />
            {item.label}
          </RadioPrimitive.Root>
        );
      })}
    </RadioGroupPrimitive>
    </div>
  );
}

function ProviderCard({
  profile,
  client,
  active,
  dragging,
  inspectionStatus,
  onRetryInspection,
  usageKey,
  busy,
  onSync,
  onTest,
  onDragStart,
  onDragEnd,
  onDrop,
  onDuplicate,
  onEdit,
  onDelete,
  onRemove,
}: {
  profile: IntegrationProfile;
  client: ClientKind;
  active: boolean;
  dragging: boolean;
  inspectionStatus: "loading" | "ready" | "error";
  onRetryInspection: () => void;
  usageKey?: ConsoleKey;
  busy: string;
  onSync: (value: IntegrationProfile) => void;
  onTest: (value: IntegrationProfile) => void;
  onDragStart: () => void;
  onDragEnd: () => void;
  onDrop: () => void;
  onDuplicate: (value: IntegrationProfile) => void;
  onEdit: (value: IntegrationProfile) => void;
  onDelete: (value: IntegrationProfile) => void;
  onRemove: (value: IntegrationProfile) => void;
}) {
  const additive = additiveClients.includes(client);
  const endpoint =
    profile.providerType === "junohub"
      ? window.location.origin
      : profile.baseUrl;
  const description =
    profile.notes || profile.websiteUrl || endpoint || "使用客户端官方登录";
  return (
    <article
      draggable={!busy}
      onDragStart={onDragStart}
      onDragEnd={onDragEnd}
      onDragOver={(event) => event.preventDefault()}
      onDrop={onDrop}
      className={cn(
        "group rounded-lg border bg-bg-raised px-4 py-3 transition-[border-color,opacity] duration-150",
        dragging && "opacity-50",
        active
          ? "border-accent-solid/60"
          : "border-border-default hover:border-border-strong",
      )}
    >
      <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
        <div className="flex min-w-0 flex-1 items-center gap-2">
          <span
            className="-ms-1 hidden cursor-grab p-1 text-muted-foreground/50 sm:block"
            aria-hidden="true"
          >
            <GripVertical className="size-4" />
          </span>
          <span className="flex size-8 shrink-0 items-center justify-center rounded-lg border border-border-default bg-bg-sunken">
            <ProviderBrand icon={profile.metadata?.presetId === "junohub" || profile.providerType === "junohub" ? "junohub" : profile.metadata?.icon} name={profile.name} />
          </span>
          <div className="min-w-0 flex-1 space-y-1">
            <div className="flex min-h-7 flex-wrap items-center gap-2">
              <h3 className="truncate text-base font-semibold leading-none">
                {profile.name}
              </h3>
              {active && (
                <span className="flex items-center gap-1 text-xs text-success-text">
                  <Check className="size-3" />
                  {additive ? "已添加" : "当前使用"}
                </span>
              )}
            </div>
            <p
              className="truncate text-sm text-muted-foreground"
              title={description}
            >
              {description}
            </p>
          </div>
        </div>
        <div className="ms-auto flex shrink-0 items-center gap-1.5">
          <div className="flex items-center gap-1 sm:opacity-0 sm:group-hover:opacity-100 sm:group-focus-within:opacity-100">
            <Button
              variant="ghost"
              size="icon-sm"
              title="检测连通性"
              aria-label={`检测 ${profile.name} 连通性`}
              onClick={() => onTest(profile)}
              disabled={Boolean(busy)}
            >
              {busy === `test-${profile.id}` ? (
                <Loader2 className="animate-spin" />
              ) : (
                <Activity />
              )}
            </Button>
            <Button
              variant="ghost"
              size="icon-sm"
              title="复制"
              aria-label={`复制 ${profile.name}`}
              onClick={() => onDuplicate(profile)}
              disabled={Boolean(busy)}
            >
              <Copy />
            </Button>
            <Button
              variant="ghost"
              size="icon-sm"
              title="编辑"
              aria-label={`编辑 ${profile.name}`}
              onClick={() => onEdit(profile)}
              disabled={Boolean(busy)}
            >
              <Edit2 />
            </Button>
            {additive && active && (
              <Button
                variant="ghost"
                size="icon-sm"
                title="从配置移除"
                aria-label={`从配置移除 ${profile.name}`}
                onClick={() => onRemove(profile)}
                disabled={Boolean(busy)}
              >
                <X />
              </Button>
            )}
            <Button
              variant="ghost"
              size="icon-sm"
              title={active ? "请先从客户端配置移除或切换供应商" : "删除"}
              aria-label={`删除 ${profile.name}`}
              onClick={() => onDelete(profile)}
              disabled={active || Boolean(busy)}
            >
              <Trash2 />
            </Button>
          </div>
          <Button
            size="sm"
            variant={active ? "secondary" : "default"}
            disabled={inspectionStatus === "loading" || (inspectionStatus === "ready" && active) || Boolean(busy)}
            onClick={() => inspectionStatus === "error" ? onRetryInspection() : onSync(profile)}
          >
            {inspectionStatus === "loading" || busy === `sync-${profile.id}` ? (
              <Loader2 className="animate-spin" />
            ) : active ? (
              <Check />
            ) : (
              <Plus />
            )}
            {inspectionStatus === "loading" ? "加载中" : inspectionStatus === "error" ? "重新读取" : active
              ? additive
                ? "已添加"
                : "使用中"
              : additive
                ? "添加"
                : "启用"}
          </Button>
        </div>
      </div>
      {usageKey && (
        <p className="mt-2 ps-10 text-xs tabular-nums text-muted-foreground">
          {(usageKey.total_requests || 0).toLocaleString()} 次请求 ·{" "}
          {(
            (usageKey.input_tokens || 0) + (usageKey.output_tokens || 0)
          ).toLocaleString()}{" "}
          Token
        </p>
      )}
    </article>
  );
}

function ToolCard({
  item,
  status,
  busy,
  onAction,
}: {
  item: { value: ToolKind; label: string };
  status?: ToolStatus;
  busy: string;
  onAction: (target: string, action: "install" | "update") => void;
}) {
  const action =
    status && !status.installed_but_broken
      ? !status.current_version
        ? "install"
        : hasUpdate(status)
          ? "update"
          : null
      : null;
  return (
    <div className="flex min-h-36 flex-col rounded-xl border border-border-default bg-bg-raised p-4">
      <div className="flex justify-between gap-3">
        <div>
          <span className="text-sm font-semibold">{item.label}</span>
          {status?.env_type && (
            <span className="ms-2 rounded-full bg-bg-sunken px-1.5 py-0.5 text-[10px] text-muted-foreground">
              {status.env_type}
              {status.wsl_distro ? ` · ${status.wsl_distro}` : ""}
            </span>
          )}
        </div>
        {!status ? (
          <span className="text-xs text-muted-foreground">未检测</span>
        ) : status.current_version ? (
          <CheckCircle2 className="size-4 text-success-text" />
        ) : (
          <AlertCircle className="size-4 text-warning-text" />
        )}
      </div>
      <dl className="mt-4 space-y-2 text-xs">
        <div className="flex justify-between gap-3">
          <dt className="text-muted-foreground">当前版本</dt>
          <dd className="font-mono">
            {status?.current_version ||
              (status
                ? status.installed_but_broken
                  ? "无法运行"
                  : "未安装"
                : "—")}
          </dd>
        </div>
        <div className="flex justify-between gap-3">
          <dt className="text-muted-foreground">最新版本</dt>
          <dd className={status?.latest_version ? "font-mono" : "text-muted-foreground"}>{status?.latest_version || (status ? "暂不可用" : "—")}</dd>
        </div>
      </dl>
      {status?.installed_but_broken && status.error && <p className="mt-3 break-words text-xs leading-5 text-warning-text">{status.error}。请检查或重新安装该工具后再检测。</p>}
      {status && status.installations.length > 1 && (
        <details className="mt-2 text-[11px]">
          <summary className="cursor-pointer text-warning-text">
            检测到 {status.installations.length} 个安装位置
          </summary>
          <div className="mt-2 space-y-1.5">
            {status.installations.map((installation) => (
              <div
                key={installation.path}
                className="rounded-md bg-bg-sunken px-2 py-1.5"
              >
                <div className="flex justify-between gap-2">
                  <span>
                    {installation.source}
                    {installation.is_path_default ? " · 默认" : ""}
                  </span>
                  <span className="font-mono">
                    {installation.version || "无法运行"}
                  </span>
                </div>
                <p
                  className="mt-1 truncate font-mono text-muted-foreground"
                  title={installation.path}
                >
                  {installation.path}
                </p>
              </div>
            ))}
          </div>
        </details>
      )}
      <div className="mt-auto flex justify-end pt-4">
        {action ? (
          <Button
            variant="outline"
            size="sm"
            onClick={() => onAction(item.value, action)}
            disabled={Boolean(busy)}
          >
            {busy.includes(item.value) ? (
              <Loader2 className="animate-spin" />
            ) : action === "install" ? (
              <Download />
            ) : (
              <ArrowUpCircle />
            )}
            {busy.includes(item.value)
              ? action === "install"
                ? "安装中"
                : "更新中"
              : action === "install"
                ? "安装"
                : "更新"}
          </Button>
        ) : status?.current_version ? (
          <span className="flex items-center gap-1.5 text-xs text-muted-foreground">
            <Check className="size-3.5" />
            {status.latest_version ? "已是最新" : "已安装"}
          </span>
        ) : null}
      </div>
    </div>
  );
}

function IntegrationDialog({
  form,
  keys,
  models,
  busy,
  onChange,
  onClose,
  onSave,
}: {
  form: FormState | null;
  client: ClientKind;
  keys: ConsoleKey[];
  models: AvailableModel[];
  busy: boolean;
  onChange: (value: FormState | null) => void;
  onClose: () => void;
  onSave: (sync: boolean) => void;
}) {
  const [showApiKey, setShowApiKey] = useState(false);
  const [saveSyncConfirm, setSaveSyncConfirm] = useState(false);
  const selectedKey = keys.find((key) => key.id === form?.keyId);
  const isJunoHub = form?.providerType === "junohub";
  const scopedClient =
    form?.metadata?.nativeClient ||
    clientItems.find((item) => form?.apps[item.value])?.value;
  const allowed = selectedKey?.allowed_models || [];
  const modelOptions = models
    .filter((model) => !allowed.length || allowed.includes(model.alias))
    .map((model) => ({ value: model.alias, label: model.alias }));
  const updateKey = (value: string) => {
    if (!form) return;
    const key = keys.find((item) => String(item.id) === value);
    const keyAllowed = key?.allowed_models || [];
    const model =
      models.find(
        (item) => !keyAllowed.length || keyAllowed.includes(item.alias),
      )?.alias || "";
    update({
      keyId: Number(value),
      models: {
        claude: {
          model,
          haikuModel: model,
          sonnetModel: model,
          opusModel: model,
        },
        codex: { model, reasoningEffort: "high" },
        gemini: { model },
        grok: { model, protocol: defaultProtocol("grok") },
        opencode: { model, protocol: defaultProtocol("opencode") },
        openclaw: { model, protocol: defaultProtocol("openclaw") },
        hermes: { model, protocol: defaultProtocol("hermes") },
        pi: { model, protocol: defaultProtocol("pi") },
      },
    });
  };
  const update = (patch: Partial<FormState>) =>
    form && onChange({ ...form, ...patch });
  const updateModels = (
    app: keyof IntegrationModels,
    patch: Record<string, string>,
  ) =>
    form &&
    update({
      models: {
        ...form.models,
        [app]: { ...(form.models[app] || {}), ...patch },
      } as IntegrationModels,
    });
  const complete = Boolean(
    form?.name.trim() &&
    (isJunoHub
      ? Boolean(form.keyId)
      : Boolean(
          form.baseUrl?.trim() &&
          (form.id || form.copyFromId || form.apiKey?.trim()),
        )) &&
    clientItems.some((item) => form.apps[item.value]) &&
    clientItems
      .filter((item) => form.apps[item.value])
      .every((item) => form.models[item.value]?.model),
  );
  const previewKey = form?.apiKey || (form?.id ? "••••••••" : "");
  const previewBaseUrl = isJunoHub
    ? window.location.origin
    : form?.baseUrl || "";
  const codexBaseUrl = previewBaseUrl.endsWith("/v1")
    ? previewBaseUrl
    : `${previewBaseUrl.replace(/\/+$/, "")}/v1`;
  return (
    <>
      <Dialog
        open={Boolean(form)}
        onOpenChange={(open) => {
          if (!open && !busy) onClose();
        }}
      >
        <DialogContent
          showCloseButton={false}
          className="h-[min(88dvh,54rem)] w-[calc(100vw-1.5rem)] min-w-0 max-w-[52rem] grid-cols-[minmax(0,1fr)] grid-rows-[auto_minmax(0,1fr)_auto] gap-0 overflow-hidden rounded-2xl bg-bg-surface p-0 sm:max-w-[52rem]"
        >
          <DialogHeader className="flex min-h-16 flex-row items-center justify-between gap-3 border-b border-border-default px-4 py-3 sm:px-6">
            <DialogTitle className="text-lg font-semibold">
              {form?.id ? "编辑供应商" : "添加供应商"}
            </DialogTitle>
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              onClick={onClose}
              disabled={busy}
              aria-label="关闭"
            >
              <X />
            </Button>
          </DialogHeader>
          {form && (
            <div className="min-h-0 min-w-0 overflow-y-auto px-4 py-6 sm:px-6">
              <div className="w-full space-y-6">
                {scopedClient && (
                  <div className="flex items-center justify-between gap-3 rounded-lg border border-border-default p-3 text-sm">
                    <span>
                      {
                        clientItems.find((item) => item.value === scopedClient)
                          ?.label
                      }{" "}
                      · JunoHub
                    </span>
                    {!form.id && (
                      <Button
                        variant="ghost"
                        size="sm"
                        onClick={() => onChange(createNativeForm(scopedClient))}
                      >
                        更换预设
                      </Button>
                    )}
                  </div>
                )}
                <div className="space-y-4">
                  <div className="space-y-2">
                    <Label htmlFor="integration-name" className="mb-2">
                      名称
                    </Label>
                    <Input
                      id="integration-name"
                      value={form.name}
                      onChange={(event) => update({ name: event.target.value })}
                    />
                  </div>
                  {isJunoHub ? (
                    <div className="space-y-2">
                      <Label htmlFor="integration-key" className="mb-2">
                        JunoHub 访问密钥
                      </Label>
                      <SelectField
                        id="integration-key"
                        value={String(form.keyId)}
                        onValueChange={updateKey}
                        options={keys.map((key) => ({
                          value: String(key.id),
                          label: `${key.name} · ${key.key_prefix}`,
                        }))}
                        placeholder="选择访问密钥"
                      />
                    </div>
                  ) : (
                    <div className="space-y-2">
                      <Label htmlFor="integration-base-url">API 地址</Label>
                      <Input
                        id="integration-base-url"
                        value={form.baseUrl || ""}
                        onChange={(event) =>
                          update({ baseUrl: event.target.value })
                        }
                        placeholder="https://api.example.com/v1"
                      />
                    </div>
                  )}
                </div>
                {!isJunoHub && (
                  <>
                    <div className="space-y-2">
                      <Label htmlFor="integration-api-key">API Key</Label>
                      <div className="relative">
                        <Input
                          id="integration-api-key"
                          type={showApiKey ? "text" : "password"}
                          autoComplete="new-password"
                          value={form.apiKey || ""}
                          onChange={(event) =>
                            update({ apiKey: event.target.value })
                          }
                          placeholder={
                            form.id ? "留空则保持当前 API Key" : "sk-..."
                          }
                          className="pe-10"
                        />
                        <Button
                          type="button"
                          variant="ghost"
                          size="icon-sm"
                          className="absolute end-1 top-1/2 -translate-y-1/2"
                          onClick={() => setShowApiKey((value) => !value)}
                          aria-label={
                            showApiKey ? "隐藏 API Key" : "显示 API Key"
                          }
                        >
                          {showApiKey ? <EyeOff /> : <Eye />}
                        </Button>
                      </div>
                    </div>
                    <div className="space-y-2">
                      <Label htmlFor="integration-website">官网地址</Label>
                      <Input
                        id="integration-website"
                        value={form.websiteUrl || ""}
                        onChange={(event) =>
                          update({ websiteUrl: event.target.value })
                        }
                        placeholder="https://example.com（可选，用于在列表中显示）"
                      />
                    </div>
                  </>
                )}
                <div className="space-y-2">
                  <Label htmlFor="integration-notes">备注</Label>
                  <Input
                    id="integration-notes"
                    value={form.notes || ""}
                    onChange={(event) => update({ notes: event.target.value })}
                    placeholder="可选：添加备注信息"
                  />
                </div>
                <div className="space-y-4">
                  <Label>模型配置</Label>
                  {form.apps.claude && (
                    <ClientModels title="Claude">
                      <div className="grid gap-3 sm:grid-cols-2">
                        <ModelField
                          label="主模型"
                          value={form.models.claude?.model || ""}
                          options={modelOptions}
                          freeform
                          onChange={(value) =>
                            updateModels("claude", { model: value })
                          }
                        />
                        <ModelField
                          label="Haiku"
                          value={form.models.claude?.haikuModel || ""}
                          options={modelOptions}
                          freeform
                          onChange={(value) =>
                            updateModels("claude", { haikuModel: value })
                          }
                        />
                        <ModelField
                          label="Sonnet"
                          value={form.models.claude?.sonnetModel || ""}
                          options={modelOptions}
                          freeform
                          onChange={(value) =>
                            updateModels("claude", { sonnetModel: value })
                          }
                        />
                        <ModelField
                          label="Opus"
                          value={form.models.claude?.opusModel || ""}
                          options={modelOptions}
                          freeform
                          onChange={(value) =>
                            updateModels("claude", { opusModel: value })
                          }
                        />
                      </div>
                    </ClientModels>
                  )}
                  {form.apps.codex && (
                    <ClientModels title="Codex">
                      <div className="grid gap-3 sm:grid-cols-2">
                        <ModelField
                          label="默认模型"
                          value={form.models.codex?.model || ""}
                          options={modelOptions}
                          freeform
                          onChange={(value) =>
                            updateModels("codex", { model: value })
                          }
                        />
                        <div className="space-y-1">
                          <Label htmlFor="codex-reasoning" className="text-xs">
                            Reasoning Effort
                          </Label>
                          <Input
                            id="codex-reasoning"
                            value={form.models.codex?.reasoningEffort || "high"}
                            onChange={(event) =>
                              updateModels("codex", {
                                reasoningEffort: event.target.value,
                              })
                            }
                            placeholder="high"
                          />
                        </div>
                      </div>
                    </ClientModels>
                  )}
                  {form.apps.gemini && (
                    <ClientModels title="Gemini CLI">
                      <ModelField
                        label="默认模型"
                        value={form.models.gemini?.model || ""}
                        options={modelOptions}
                        freeform
                        onChange={(value) =>
                          updateModels("gemini", { model: value })
                        }
                      />
                    </ClientModels>
                  )}
                  {clientItems
                    .filter(
                      (item) =>
                        !["claude", "codex", "gemini"].includes(item.value),
                    )
                    .map((item) =>
                      form.apps[item.value] ? (
                        <ClientModels key={item.value} title={item.label}>
                          <div className="grid gap-3 sm:grid-cols-2">
                            <ModelField
                              label="默认模型"
                              value={form.models[item.value]?.model || ""}
                              options={modelOptions}
                              freeform
                              onChange={(value) =>
                                updateModels(item.value, { model: value })
                              }
                            />
                            <div className="space-y-1">
                              <Label
                                htmlFor={`${item.value}-protocol`}
                                className="text-xs"
                              >
                                {item.value === "opencode"
                                  ? "SDK 类型"
                                  : "API 协议"}
                              </Label>
                              <SelectField
                                id={`${item.value}-protocol`}
                                value={
                                  configuredProtocol(form.models, item.value) ||
                                  defaultProtocol(item.value) ||
                                  ""
                                }
                                onValueChange={(value) =>
                                  updateModels(item.value, { protocol: value })
                                }
                                options={protocolOptions[item.value] || []}
                              />
                            </div>
                          </div>
                        </ClientModels>
                      ) : null,
                    )}
                </div>
                {form.id && (
                  <div className="space-y-4">
                    <Label>配置 JSON 预览</Label>
                    <p className="text-xs text-muted-foreground">
                      以下是将要同步到各应用的配置内容（仅覆盖显示的字段，保留其他自定义配置）
                    </p>
                    {form.apps.claude && (
                      <ConfigPreview
                        title="Claude"
                        value={{
                          env: {
                            ANTHROPIC_BASE_URL: previewBaseUrl,
                            ANTHROPIC_AUTH_TOKEN: previewKey,
                            ANTHROPIC_MODEL: form.models.claude?.model,
                            ANTHROPIC_DEFAULT_HAIKU_MODEL:
                              form.models.claude?.haikuModel,
                            ANTHROPIC_DEFAULT_SONNET_MODEL:
                              form.models.claude?.sonnetModel,
                            ANTHROPIC_DEFAULT_OPUS_MODEL:
                              form.models.claude?.opusModel,
                          },
                        }}
                      />
                    )}
                    {form.apps.codex && (
                      <ConfigPreview
                        title="Codex"
                        value={{
                          auth: { OPENAI_API_KEY: previewKey },
                          config: `model_provider = "custom"\nmodel = "${form.models.codex?.model || ""}"\nmodel_reasoning_effort = "${form.models.codex?.reasoningEffort || "high"}"\ndisable_response_storage = true\n\n[model_providers.custom]\nname = "${form.name}"\nbase_url = "${codexBaseUrl}"\nwire_api = "responses"\nrequires_openai_auth = false`,
                        }}
                      />
                    )}
                    {form.apps.gemini && (
                      <ConfigPreview
                        title="Gemini"
                        value={{
                          env: {
                            GOOGLE_GEMINI_BASE_URL: previewBaseUrl,
                            GEMINI_API_KEY: previewKey,
                            GEMINI_MODEL: form.models.gemini?.model,
                          },
                        }}
                      />
                    )}
                  </div>
                )}
              </div>
            </div>
          )}
          <DialogFooter className="m-0 flex-row justify-end rounded-none bg-bg-surface px-4 py-3 sm:px-6">
            <span className="me-auto hidden min-w-0 text-xs text-muted-foreground sm:block">
              配置会保存到供应商列表，启用后写入当前客户端。
            </span>
            <Button variant="outline" onClick={onClose} disabled={busy}>
              取消
            </Button>
            <Button
              onClick={() =>
                form?.id ? setSaveSyncConfirm(true) : onSave(false)
              }
              disabled={!complete || busy}
            >
              {busy && <Loader2 className="animate-spin" />}
              {form?.id ? <RefreshCw /> : <Plus />}
              {form?.id ? "保存并同步" : "添加"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      <ConfirmDialog
        open={saveSyncConfirm}
        onOpenChange={setSaveSyncConfirm}
        title="保存并同步供应商"
        description={`保存“${form?.name || ""}”后，将更新所有已启用客户端中的对应配置。`}
        confirmText="保存并同步"
        onConfirm={() => {
          onSave(true);
        }}
      />
    </>
  );
}

function ConfigPreview({ title, value }: { title: string; value: unknown }) {
  return (
    <div className="space-y-2">
      <p className="text-sm font-medium">{title}</p>
      <pre className="max-h-72 overflow-auto rounded-lg border border-border-default bg-bg-sunken p-3 text-xs leading-5">
        {JSON.stringify(value, null, 2)}
      </pre>
    </div>
  );
}

function ClientModels({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <div className="rounded-xl border border-border-default p-4">
      <h3 className="mb-3 text-sm font-semibold">{title}</h3>
      {children}
    </div>
  );
}
function ModelField({
  label,
  value,
  options,
  freeform = false,
  onChange,
}: {
  label: string;
  value: string;
  options: Array<{ value: string; label: string }>;
  freeform?: boolean;
  onChange: (value: string) => void;
}) {
  const id = useId();
  return (
    <div>
      <Label htmlFor={id} className="mb-2">
        {label}
      </Label>
      {freeform ? (
        <Input
          id={id}
          value={value}
          onChange={(event) => onChange(event.target.value)}
          placeholder="输入模型名称"
        />
      ) : (
        <SelectField
          id={id}
          value={value}
          onValueChange={onChange}
          options={options}
          placeholder="选择模型"
        />
      )}
    </div>
  );
}
