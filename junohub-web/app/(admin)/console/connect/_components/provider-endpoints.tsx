"use client";
import { useState } from "react";
import { Check, Loader2, Plus, RefreshCw, Trash2, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
export type ProviderEndpoint = {
  url: string;
  addedAt: number;
  lastUsed?: number;
};
export function ProviderEndpoints({
  current,
  candidates,
  endpoints,
  apiKey,
  protocol,
  headers,
  onChange,
  onSelect,
  onClose,
}: {
  current: string;
  candidates: string[];
  endpoints: Record<string, ProviderEndpoint>;
  apiKey: string;
  protocol: string;
  headers: Record<string, unknown>;
  onChange: (endpoints: Record<string, ProviderEndpoint>) => void;
  onSelect: (url: string, endpoints: Record<string, ProviderEndpoint>) => void;
  onClose: () => void;
}) {
  const [draft, setDraft] = useState(""),
    [error, setError] = useState(""),
    [busy, setBusy] = useState<string[]>([]),
    [checks, setChecks] = useState<
      Record<string, { ms: number; ok: boolean; error?: string }>
    >({}),
    [sorted, setSorted] = useState(false);
  const normalize = (url: string) => url.trim().replace(/\/+$/, "");
  const urls = Array.from(
    new Set(
      [current, ...candidates, ...Object.keys(endpoints)]
        .map(normalize)
        .filter(Boolean),
    ),
  );
  if (sorted)
    urls.sort(
      (a, b) =>
        (checks[a]?.ok ? checks[a].ms : Infinity) -
        (checks[b]?.ok ? checks[b].ms : Infinity),
    );
  const add = () => {
    try {
      const url = normalize(draft),
        parsed = new URL(url);
      if (
        !["http:", "https:"].includes(parsed.protocol) ||
        parsed.username ||
        parsed.password ||
        parsed.search ||
        parsed.hash
      )
        throw Error();
      onChange({ ...endpoints, [url]: { url, addedAt: Date.now() } });
      setDraft("");
      setError("");
    } catch {
      setError("请输入完整的 HTTP 或 HTTPS API 地址");
    }
  };
  const test = async (url: string) => {
    setBusy((b) => [...b, url]);
    const start = performance.now();
    try {
      const response = await fetch("http://127.0.0.1:17372/models", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          baseUrl: url,
          apiKey,
          apiFormat: protocol,
          requestHeaders: headers,
        }),
        signal: AbortSignal.timeout(55000),
      });
      const result = await response.json();
      setChecks((c) => ({
        ...c,
        [url]: {
          ok: result.success,
          ms: Math.round(performance.now() - start),
          error: result.error,
        },
      }));
    } catch {
      setChecks((c) => ({
        ...c,
        [url]: {
          ok: false,
          ms: Math.round(performance.now() - start),
          error: "连接失败或超时",
        },
      }));
    } finally {
      setBusy((b) => b.filter((v) => v !== url));
    }
  };
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent
        showCloseButton={false}
        className="w-[calc(100vw-1.5rem)] max-w-[46rem] grid-cols-[minmax(0,1fr)] grid-rows-[auto_minmax(0,1fr)] gap-0 overflow-hidden p-0 sm:max-w-[46rem]"
      >
        <DialogHeader className="flex flex-row items-center justify-between border-b border-border-default px-5 py-4">
          <DialogTitle>API 端点</DialogTitle>
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label="关闭端点管理"
            onClick={onClose}
          >
            <X />
          </Button>
        </DialogHeader>
        <div className="min-h-0 space-y-4 overflow-y-auto p-5">
          <div className="flex items-center gap-2">
            <Input
              aria-label="添加 API 端点"
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              placeholder="https://api.example.com/v1"
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  add();
                }
              }}
            />
            <Button
              variant="outline"
              size="icon"
              aria-label="添加端点"
              onClick={add}
            >
              <Plus />
            </Button>
          </div>
          <div className="flex flex-wrap items-center justify-between gap-2">
            <p className="text-xs text-muted-foreground">
              检测模型接口的响应时间，不发送生成请求。
            </p>
            <div className="flex gap-2">
              <Button
                size="sm"
                variant="ghost"
                onClick={() => setSorted(!sorted)}
              >
                按延迟排序
              </Button>
              <Button
                size="sm"
                variant="outline"
                disabled={busy.length > 0 || !urls.length}
                onClick={() =>
                  void (async () => {
                    for (const url of urls) await test(url);
                  })()
                }
              >
                <RefreshCw />
                全部检测
              </Button>
            </div>
          </div>
          <div className="divide-y divide-border-default rounded-xl border border-border-default">
            {urls.map((url) => (
              <div key={url} className="px-3 py-3">
                <div className="flex flex-wrap items-center justify-end gap-2 sm:flex-nowrap">
                  <div className="min-w-0 basis-full sm:flex-1 sm:basis-auto">
                    <p className="break-all font-mono text-xs">{url}</p>
                    <p className="mt-1 text-xs text-muted-foreground">
                      {url === normalize(current)
                        ? "当前端点"
                        : endpoints[url]
                          ? "自定义端点"
                          : "预设端点"}
                      {checks[url]?.ok ? ` · ${checks[url].ms} ms` : ""}
                    </p>
                  </div>
                  <Button
                    size="icon-sm"
                    variant="ghost"
                    disabled={busy.includes(url)}
                    aria-label={`检测端点 ${url}`}
                    onClick={() => void test(url)}
                  >
                    {busy.includes(url) ? (
                      <Loader2 className="animate-spin" />
                    ) : (
                      <RefreshCw />
                    )}
                  </Button>
                  <Button
                    size="icon-sm"
                    variant="ghost"
                    aria-label={`使用端点 ${url}`}
                    disabled={url === normalize(current)}
                    onClick={() => {
                      onSelect(url, {
                        ...endpoints,
                        [url]: {
                          url,
                          addedAt: endpoints[url]?.addedAt || Date.now(),
                          lastUsed: Date.now(),
                        },
                      });
                    }}
                  >
                    <Check />
                  </Button>
                  {endpoints[url] && (
                    <Button
                      size="icon-sm"
                      variant="ghost"
                      aria-label={`删除端点 ${url}`}
                      onClick={() => {
                        const next = { ...endpoints };
                        delete next[url];
                        onChange(next);
                      }}
                    >
                      <Trash2 />
                    </Button>
                  )}
                </div>
                {checks[url] && !checks[url].ok && (
                  <p className="mt-2 break-words text-xs text-danger-text">
                    {checks[url].error || "端点不可用"}
                  </p>
                )}
              </div>
            ))}
            {!urls.length && (
              <p className="p-6 text-center text-sm text-muted-foreground">
                添加端点后可检测并切换。
              </p>
            )}
          </div>
          {error && (
            <p role="alert" className="text-sm text-danger-text">
              {error}
            </p>
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}
