"use client";
import { useCallback, useEffect, useState } from "react";
import {
  ArrowDown,
  ArrowUp,
  Loader2,
  Plus,
  RefreshCw,
  Trash2,
  X,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { SelectField } from "@/components/ui/select-field";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogFooter,
} from "@/components/ui/dialog";
import {
  consoleAPI,
  type IntegrationClient,
  type IntegrationProfile,
} from "@/lib/api";
import { clientNames } from "@/lib/cc-switch/native-config";
import { toast } from "sonner";
type Probe = {
  id: string;
  success: boolean;
  status?: number;
  latencyMs?: number;
  error?: string;
};
type Route = {
  client: IntegrationClient;
  sources: { id: string; name: string }[];
  autoFailoverEnabled: boolean;
  timeoutSeconds: number;
  failureThreshold: number;
  cooldownSeconds: number;
  hasHeaderOverrides: boolean;
  hasBodyOverrides: boolean;
  stats: {
    requests: number;
    failovers: number;
    errors: number;
    lastProvider?: string;
    lastError?: string;
    lastLatencyMs: number;
  };
};
export async function localProxy(path: string, payload?: unknown) {
  const result = await fetch(`http://127.0.0.1:17372/proxy/${path}`, {
    method: payload === undefined ? "GET" : "POST",
    headers: { "Content-Type": "application/json" },
    ...(payload === undefined ? {} : { body: JSON.stringify(payload) }),
    signal: AbortSignal.timeout(60000),
  });
  const data = await result.json();
  if (!data.success) throw Error(data.error || "本地代理操作失败");
  return data as { routes: Route[]; results: Probe[] };
}
export function LocalProxyPanel({
  client,
  profiles,
  onClose,
  mode,
}: {
  client: IntegrationClient;
  profiles: IntegrationProfile[];
  onClose: () => void;
  mode: "settings" | "queue";
}) {
  const [route, setRoute] = useState<Route | null>(null),
    [ids, setIds] = useState<string[]>([]),
    [addId, setAddId] = useState(""),
    [busy, setBusy] = useState(false),
    [loading, setLoading] = useState(true),
    [error, setError] = useState("");
  const [timeout, setTimeoutValue] = useState(30),
    [threshold, setThreshold] = useState(3),
    [cooldown, setCooldown] = useState(30);
  const [probes, setProbes] = useState<Probe[]>([]);
  const available = profiles.filter((profile) => profile.apps[client]);
  const load = useCallback(
    async (initial = false) => {
      try {
        const data = await localProxy("status");
        const current =
          data.routes.find((route) => route.client === client) || null;
        setRoute(current);
        setError("");
        if (initial && current) {
          setIds(current.sources.map((s) => s.id));
          setTimeoutValue(current.timeoutSeconds);
          setThreshold(current.failureThreshold);
          setCooldown(current.cooldownSeconds);
        }
      } catch (e) {
        setError(
          e instanceof TypeError
            ? "无法连接本地组件，请确认组件已安装并运行。"
            : e instanceof Error
              ? e.message
              : "读取代理状态失败",
        );
      } finally {
        setLoading(false);
      }
    },
    [client],
  );
  useEffect(() => {
    void load(true);
  }, [load]);
  const pairUrls = async () => {
    const urls: string[] = [];
    for (const id of ids) {
      const pair = await consoleAPI.integrationSync(id, client);
      urls.push(
        `${window.location.origin}/client-connect/redeem?token=${pair.token}`,
      );
    }
    return urls;
  };
  const probe = async () => {
    setBusy(true);
    setError("");
    try {
      setProbes(
        (await localProxy("probe", { client, pairUrls: await pairUrls() }))
          .results,
      );
    } catch (e) {
      setError(e instanceof Error ? e.message : "检测失败");
    } finally {
      setBusy(false);
    }
  };
  const save = async () => {
    setBusy(true);
    setError("");
    try {
      const data =
        mode === "settings"
          ? await localProxy("settings", {
              client,
              timeoutSeconds: timeout,
              failureThreshold: threshold,
              cooldownSeconds: cooldown,
            })
          : await localProxy("queue", {
              client,
              pairUrls: await pairUrls(),
            });
      setRoute(data.routes.find((route) => route.client === client) || null);
      window.dispatchEvent(new Event("junohub-proxy-change"));
      toast.success(
        mode === "settings" ? "代理设置已保存" : "故障转移队列已保存",
      );
    } catch (e) {
      setError(e instanceof Error ? e.message : "保存失败");
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog open onOpenChange={(open) => !open && !busy && onClose()}>
      <DialogContent
        showCloseButton={false}
        className="w-[calc(100vw-1.5rem)] max-w-[46rem] grid-cols-[minmax(0,1fr)] grid-rows-[auto_minmax(0,1fr)_auto] gap-0 overflow-hidden rounded-2xl p-0 sm:max-w-[46rem]"
      >
        <DialogHeader className="flex flex-row items-center justify-between border-b border-border-default px-5 py-4">
          <DialogTitle>
            {clientNames[client]} ·{" "}
            {mode === "settings" ? "代理设置" : "故障转移队列"}
          </DialogTitle>
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label="关闭"
            onClick={onClose}
            disabled={busy}
          >
            <X />
          </Button>
        </DialogHeader>
        <div className="min-h-0 space-y-5 overflow-y-auto overscroll-contain p-5">
          {loading ? (
            <p role="status">正在读取本地状态…</p>
          ) : (
            <>
              {mode === "settings" && (
                <section className="space-y-3">
                  <div className="flex items-center justify-between">
                    <p className="font-medium">
                      {route ? "代理已启用" : "代理未启用"}
                    </p>
                    <Button
                      size="icon-sm"
                      variant="ghost"
                      aria-label="刷新代理状态"
                      onClick={() => void load()}
                      disabled={busy}
                    >
                      <RefreshCw />
                    </Button>
                  </div>
                  {route && (
                    <>
                      <div className="grid grid-cols-3 gap-3 text-sm">
                        {[
                          ["请求", route.stats.requests],
                          ["失败切换", route.stats.failovers],
                          ["错误", route.stats.errors],
                        ].map(([label, value]) => (
                          <div
                            key={label}
                            className="rounded-lg bg-bg-sunken p-3"
                          >
                            <p className="text-muted-foreground">{label}</p>
                            <p className="mt-1 text-lg tabular-nums">{value}</p>
                          </div>
                        ))}
                      </div>
                      {route.stats.lastProvider && (
                        <p className="text-sm text-muted-foreground">
                          最近使用：{route.stats.lastProvider} ·{" "}
                          {route.stats.lastLatencyMs} ms
                        </p>
                      )}
                      {route.stats.lastError && (
                        <p className="text-sm text-danger-text">
                          {route.stats.lastError}
                        </p>
                      )}
                    </>
                  )}
                  <p className="text-sm text-muted-foreground">
                    启用后，主用供应商的客户端配置会指向本机组件；停用会恢复接管前的配置。
                  </p>
                </section>
              )}
              {mode === "queue" && (
                <fieldset
                  className="min-w-0 space-y-3"
                  disabled={busy || !route}
                >
                  <legend className="mb-2 font-medium">故障转移队列</legend>
                  <p className="text-sm text-muted-foreground">
                    按 P1、P2 的顺序选择供应商；开启自动故障转移后生效。
                  </p>
                  <div className="divide-y divide-border-default rounded-lg border border-border-default">
                    {ids.map((id, index) => (
                      <div
                        key={id}
                        className="grid min-w-0 grid-cols-[minmax(0,1fr)_repeat(3,2.25rem)] items-center gap-1 p-3"
                      >
                        <span className="min-w-0 flex-1 truncate text-sm">
                          P{index + 1} ·{" "}
                          {profiles.find((p) => p.id === id)?.name ||
                            route?.sources.find((p) => p.id === id)?.name ||
                            "供应商已删除"}
                        </span>
                        {probes.find((p) => p.id === id) && (
                          <span className="order-5 col-span-full min-w-0 truncate text-xs tabular-nums text-muted-foreground">
                            {(() => {
                              const p = probes.find((p) => p.id === id)!;
                              return p.status
                                ? `HTTP ${p.status} · ${p.latencyMs} ms`
                                : p.error;
                            })()}
                          </span>
                        )}
                        <Button
                          size="icon-sm"
                          variant="ghost"
                          aria-label={`上移供应商 ${index + 1}`}
                          disabled={index === 0}
                          onClick={() =>
                            setIds((items) => {
                              const next = [...items];
                              [next[index - 1], next[index]] = [
                                next[index],
                                next[index - 1],
                              ];
                              return next;
                            })
                          }
                        >
                          <ArrowUp />
                        </Button>
                        <Button
                          size="icon-sm"
                          variant="ghost"
                          aria-label={`下移供应商 ${index + 1}`}
                          disabled={index === ids.length - 1}
                          onClick={() =>
                            setIds((items) => {
                              const next = [...items];
                              [next[index + 1], next[index]] = [
                                next[index],
                                next[index + 1],
                              ];
                              return next;
                            })
                          }
                        >
                          <ArrowDown />
                        </Button>
                        <Button
                          size="icon-sm"
                          variant="ghost"
                          aria-label={`移除供应商 ${index + 1}`}
                          onClick={() =>
                            setIds((items) => items.filter((v) => v !== id))
                          }
                        >
                          <Trash2 />
                        </Button>
                      </div>
                    ))}
                    {ids.length === 0 && (
                      <p className="p-3 text-sm text-muted-foreground">
                        队列为空，添加供应商后设置优先级。
                      </p>
                    )}
                  </div>
                  <div className="flex items-center gap-2">
                    <div className="min-w-0 flex-1">
                      <label htmlFor="proxy-source" className="sr-only">
                        添加队列供应商
                      </label>
                      <SelectField
                        id="proxy-source"
                        value={addId}
                        placeholder="选择供应商"
                        options={available
                          .filter((p) => !ids.includes(p.id))
                          .map((p) => ({ value: p.id, label: p.name }))}
                        onValueChange={setAddId}
                      />
                    </div>
                    <Button
                      variant="outline"
                      size="icon"
                      aria-label="添加到故障转移队列"
                      disabled={!addId}
                      onClick={() => {
                        setIds([...ids, addId]);
                        setAddId("");
                      }}
                    >
                      <Plus />
                    </Button>
                  </div>
                  <div className="flex flex-wrap items-center gap-2">
                    <Button
                      variant="outline"
                      disabled={busy || !ids.length}
                      onClick={() => void probe()}
                    >
                      检测上游
                    </Button>
                    {!!probes.length && (
                      <Button
                        variant="ghost"
                        disabled={busy}
                        onClick={() =>
                          setIds(
                            [...ids].sort((a, b) => {
                              const score = (id: string) => {
                                const p = probes.find((p) => p.id === id);
                                return p?.success
                                  ? (p.latencyMs ?? Infinity)
                                  : Infinity;
                              };
                              return score(a) - score(b);
                            }),
                          )
                        }
                      >
                        按延迟排序
                      </Button>
                    )}
                    <span className="text-xs text-muted-foreground">
                      检测模型接口连通性，不产生生成用量。
                    </span>
                  </div>
                </fieldset>
              )}
              {mode === "settings" && (
                <fieldset
                  className="grid gap-3 sm:grid-cols-3"
                  disabled={busy || !route}
                >
                  <label className="space-y-1.5 text-sm">
                    响应超时（秒）
                    <Input
                      type="number"
                      min={1}
                      max={120}
                      value={timeout}
                      onChange={(e) => setTimeoutValue(Number(e.target.value))}
                    />
                  </label>
                  <label className="space-y-1.5 text-sm">
                    连续失败阈值
                    <Input
                      type="number"
                      min={1}
                      max={10}
                      value={threshold}
                      onChange={(e) => setThreshold(Number(e.target.value))}
                    />
                  </label>
                  <label className="space-y-1.5 text-sm">
                    冷却时间（秒）
                    <Input
                      type="number"
                      min={1}
                      max={600}
                      value={cooldown}
                      onChange={(e) => setCooldown(Number(e.target.value))}
                    />
                  </label>
                </fieldset>
              )}
              {!route && (
                <p className="text-sm text-muted-foreground">
                  请先在供应商列表开启客户端接管。
                </p>
              )}
            </>
          )}
          {error && (
            <p role="alert" className="break-words text-sm text-danger-text">
              {error}
            </p>
          )}
        </div>
        <DialogFooter className="m-0 flex-row justify-end rounded-none px-5 py-3">
          <Button
            disabled={busy || loading || !route || !ids.length}
            onClick={() => void save()}
          >
            {busy && <Loader2 className="animate-spin" />}
            保存
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
