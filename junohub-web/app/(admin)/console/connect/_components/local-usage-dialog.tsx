"use client";
import { useCallback, useEffect, useState } from "react";
import {
  ChevronLeft,
  ChevronRight,
  Info,
  Loader2,
  RefreshCw,
  SearchIcon,
  X,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/coss-ui/input-group";
import { SelectField } from "@/components/ui/select-field";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { DateRangeFilter } from "@/components/date-range-filter";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { clientNames } from "@/lib/cc-switch/native-config";
type Row = {
  id: string;
  at: number;
  client: string;
  provider: string;
  requestModel: string;
  model: string;
  status: number;
  durationMs: number;
  firstByteMs?: number;
  attempts: number;
  streaming: boolean;
  completed: boolean;
  error?: string;
  usage?: {
    input_tokens: number;
    output_tokens: number;
    cache_read_tokens: number;
    cache_creation_tokens: number;
  };
  cost?: string;
};
type Result = {
  success: boolean;
  error?: string;
  list: Row[];
  total: number;
  summary: {
    input: number;
    output: number;
    cost: number | null;
    unpriced: number;
  };
};
const money = (cost: string | number | null | undefined) =>
  cost == null
    ? "未计价"
    : `$${Number(cost).toLocaleString("en-US", { maximumFractionDigits: 8 })}`;
export function LocalUsageDialog({
  initialClient,
  onClose,
}: {
  initialClient: string;
  onClose: () => void;
}) {
  const [client, setClient] = useState(initialClient),
    [provider, setProvider] = useState(""),
    [model, setModel] = useState(""),
    [status, setStatus] = useState("all"),
    [from, setFrom] = useState(""),
    [to, setTo] = useState(""),
    [page, setPage] = useState(1),
    [data, setData] = useState<Result | null>(null),
    [error, setError] = useState(""),
    [busy, setBusy] = useState(true),
    [detail, setDetail] = useState<Row | null>(null);
  const load = useCallback(
    async (signal?: AbortSignal) => {
      setBusy(true);
      try {
        const params = new URLSearchParams({
          client: client === "all" ? "" : client,
          provider,
          model,
          status: status === "all" ? "" : status,
          page: String(page),
        });
        if (from)
          params.set("from", String(new Date(`${from}T00:00:00`).getTime()));
        if (to)
          params.set("to", String(new Date(`${to}T23:59:59.999`).getTime()));
        const response = await fetch(`http://127.0.0.1:17372/usage?${params}`, {
          signal: signal
            ? AbortSignal.any([signal, AbortSignal.timeout(10000)])
            : AbortSignal.timeout(10000),
        });
        const result: Result = await response.json();
        if (!result.success) throw Error(result.error);
        setData(result);
        setError("");
      } catch (e) {
        if (!signal?.aborted)
          setError(
            e instanceof TypeError
              ? "无法连接本地组件"
              : e instanceof Error
                ? e.message
                : "读取失败",
          );
      } finally {
        if (!signal?.aborted) setBusy(false);
      }
    },
    [client, provider, model, status, from, to, page],
  );
  useEffect(() => {
    const controller = new AbortController();
    const timer = setTimeout(() => void load(controller.signal), 200);
    return () => {
      controller.abort();
      clearTimeout(timer);
    };
  }, [load]);
  const change = (setter: (v: string) => void, value: string) => {
    setter(value);
    setPage(1);
  };
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent
        showCloseButton={false}
        className="max-h-[min(88dvh,52rem)] w-[calc(100vw-1.5rem)] max-w-[74rem] grid-cols-[minmax(0,1fr)] grid-rows-[auto_minmax(0,1fr)] gap-0 overflow-hidden p-0 sm:max-w-[74rem]"
      >
        <DialogHeader className="flex flex-row items-center justify-between border-b border-border-default px-5 py-4">
          <DialogTitle>本地用量</DialogTitle>
          <div className="flex gap-1">
            <Button
              variant="ghost"
              size="icon-sm"
              aria-label="刷新用量"
              disabled={busy}
              onClick={() => void load()}
            >
              {busy ? <Loader2 className="animate-spin" /> : <RefreshCw />}
            </Button>
            <Button
              variant="ghost"
              size="icon-sm"
              aria-label="关闭本地用量"
              onClick={onClose}
            >
              <X />
            </Button>
          </div>
        </DialogHeader>
        <div className="min-h-0 space-y-4 overflow-y-auto p-4 sm:p-5">
          <p className="text-sm text-muted-foreground">
            通过本地路由的请求会记录在这里，费用按配置价格估算。
          </p>
          <div className="grid gap-2 sm:grid-cols-2 lg:grid-cols-5">
            <SelectField
              id="usage-client"
              value={client}
              options={[
                { value: "all", label: "全部客户端" },
                ...Object.entries(clientNames).map(([value, label]) => ({
                  value,
                  label,
                })),
              ]}
              onValueChange={(v) => change(setClient, v)}
            />
            <InputGroup><InputGroupInput
              type="search"
              aria-label="搜索用量供应商"
              placeholder="搜索供应商"
              value={provider}
              onChange={(e) => change(setProvider, e.target.value)}
            /><InputGroupAddon><SearchIcon aria-hidden="true" /></InputGroupAddon></InputGroup>
            <InputGroup><InputGroupInput
              type="search"
              aria-label="搜索用量模型"
              placeholder="搜索模型"
              value={model}
              onChange={(e) => change(setModel, e.target.value)}
            /><InputGroupAddon><SearchIcon aria-hidden="true" /></InputGroupAddon></InputGroup>
            <SelectField
              id="usage-status"
              value={status}
              options={[
                { value: "all", label: "全部状态" },
                { value: "success", label: "成功" },
                { value: "error", label: "失败或中断" },
              ]}
              onValueChange={(v) => change(setStatus, v)}
            />
            <DateRangeFilter
              startDate={from}
              endDate={to}
              onApply={(f, t) => {
                setFrom(f);
                setTo(t);
                setPage(1);
              }}
            />
          </div>
          <div className="flex flex-wrap items-center gap-x-6 gap-y-2 rounded-lg bg-bg-sunken px-4 py-3 text-sm tabular-nums">
            <span>{data?.total || 0} 次请求</span>
            <span>输入 {(data?.summary.input || 0).toLocaleString()}</span>
            <span>输出 {(data?.summary.output || 0).toLocaleString()}</span>
            <span>估算费用 {money(data?.summary.cost)}</span>
            {!!data?.summary.unpriced && (
              <span className="text-xs text-muted-foreground">
                {data.summary.unpriced} 条未计价
              </span>
            )}
          </div>
          {error && (
            <p role="alert" className="text-sm text-danger-text">
              {error}
            </p>
          )}
          <Table className="min-w-[58rem] text-left text-xs" containerProps={{ className: "rounded-xl border border-border-default", tabIndex: 0, role: "region", "aria-label": "本机请求用量表格" }}>
            <TableHeader className="bg-bg-sunken text-muted-foreground">
              <TableRow>
                {[
                  "时间",
                  "供应商",
                  "请求模型",
                  "状态",
                  "耗时",
                  "Token（入 / 出）",
                  "估算费用",
                  "详情",
                ].map((t) => (
                  <TableHead key={t} className="px-3 py-3 font-medium">
                    {t}
                  </TableHead>
                ))}
              </TableRow>
            </TableHeader>
            <TableBody className="divide-y divide-border-default">
              {data?.list.map((row) => (
                <TableRow key={row.id}>
                  <TableCell className="whitespace-nowrap px-3 py-3 tabular-nums">
                    {new Date(row.at).toLocaleString("zh-CN", {
                      hour12: false,
                    })}
                  </TableCell>
                  <TableCell
                    className="max-w-40 truncate px-3 py-3"
                    title={row.provider}
                  >
                    {row.provider}
                  </TableCell>
                  <TableCell
                    className="max-w-48 truncate px-3 py-3 font-mono"
                    title={row.requestModel}
                  >
                    {row.requestModel}
                  </TableCell>
                  <TableCell
                    className={`whitespace-nowrap px-3 py-3 ${row.error || row.status >= 400 ? "text-danger-text" : "text-success-text"}`}
                  >
                    {row.error && row.status < 400 ? "中断" : row.status}
                  </TableCell>
                  <TableCell className="whitespace-nowrap px-3 py-3 tabular-nums">
                    {(row.durationMs / 1000).toFixed(2)} s
                  </TableCell>
                  <TableCell className="whitespace-nowrap px-3 py-3 tabular-nums">
                    {row.usage
                      ? `${row.usage.input_tokens.toLocaleString()} / ${row.usage.output_tokens.toLocaleString()}`
                      : "未返回"}
                  </TableCell>
                  <TableCell className="whitespace-nowrap px-3 py-3 tabular-nums">
                    {money(row.cost)}
                  </TableCell>
                  <TableCell className="px-3 py-2">
                    <Button
                      variant="ghost"
                      size="icon-sm"
                      aria-label={`查看请求 ${row.id}`}
                      onClick={() => setDetail(row)}
                    >
                      <Info />
                    </Button>
                  </TableCell>
                </TableRow>
              ))}
              {!data?.list.length && (
                <TableRow>
                  <TableCell
                    colSpan={8}
                    className="py-16 text-center text-sm text-muted-foreground"
                  >
                    {busy ? "正在读取…" : "暂无请求记录"}
                  </TableCell>
                </TableRow>
              )}
            </TableBody>
          </Table>
          <p className="text-xs text-muted-foreground lg:hidden">
            左右滑动查看完整记录
          </p>
          <div className="flex items-center justify-between">
            <span className="text-xs text-muted-foreground">
              第 {page} / {Math.max(1, Math.ceil((data?.total || 0) / 50))} 页
            </span>
            <div className="flex gap-1">
              <Button
                variant="outline"
                size="icon-sm"
                aria-label="上一页用量"
                disabled={page === 1 || busy}
                onClick={() => setPage((p) => p - 1)}
              >
                <ChevronLeft />
              </Button>
              <Button
                variant="outline"
                size="icon-sm"
                aria-label="下一页用量"
                disabled={page * 50 >= (data?.total || 0) || busy}
                onClick={() => setPage((p) => p + 1)}
              >
                <ChevronRight />
              </Button>
            </div>
          </div>
        </div>
      </DialogContent>
      {detail && (
        <Dialog open onOpenChange={(open) => !open && setDetail(null)}>
          <DialogContent className="sm:max-w-lg">
            <DialogHeader>
              <DialogTitle>请求详情</DialogTitle>
            </DialogHeader>
            <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-5 gap-y-3 text-sm">
              {[
                ["请求 ID", detail.id],
                ["供应商", detail.provider],
                ["请求模型", detail.requestModel],
                ["实际模型", detail.model],
                ["HTTP 状态", detail.status],
                [
                  "首次响应",
                  detail.firstByteMs == null
                    ? "未返回"
                    : `${detail.firstByteMs} ms`,
                ],
                ["尝试次数", detail.attempts],
                ["响应方式", detail.streaming ? "流式" : "普通"],
                ["缓存读取", detail.usage?.cache_read_tokens ?? "未返回"],
                ["缓存写入", detail.usage?.cache_creation_tokens ?? "未返回"],
                ["估算费用", money(detail.cost)],
              ].map(([label, value]) => (
                <div key={String(label)} className="contents">
                  <dt className="text-muted-foreground">{label}</dt>
                  <dd className="break-all">{value}</dd>
                </div>
              ))}
            </dl>
            {detail.error && (
              <p className="text-sm text-danger-text">{detail.error}</p>
            )}
          </DialogContent>
        </Dialog>
      )}
    </Dialog>
  );
}
