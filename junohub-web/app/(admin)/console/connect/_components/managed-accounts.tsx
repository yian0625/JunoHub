"use client";
import { useCallback, useEffect, useRef, useState } from "react";
import {
  Check,
  Copy,
  ExternalLink,
  Loader2,
  Plus,
  RefreshCw,
  Star,
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
} from "@/components/ui/dialog";
import { toast } from "sonner";
export const authNames: Record<string, string> = {
  github_copilot: "GitHub Copilot",
  codex_oauth: "ChatGPT / Codex",
  xai_oauth: "xAI / Grok",
};
export type AuthBinding = {
  authProvider: string;
  accountId?: string;
  accountName?: string;
};
type Account = {
  id: string;
  login: string;
  reauth_required?: boolean;
  requires_reauth?: boolean;
};
type Group = {
  accounts: Account[];
  default_account_id?: string;
  migration_error?: string;
};
type Login = {
  sessionId: string;
  userCode: string;
  verificationUri: string;
  expiresIn: number;
  interval: number;
};
type AccountResponse = {
  success: boolean;
  error?: string;
  providers: Record<string, Group>;
  pending?: boolean;
  account?: Account;
  interval?: number;
} & Login;
export async function accountRequest(
  payload?: Record<string, unknown>,
): Promise<AccountResponse> {
  const response = await fetch("http://127.0.0.1:17372/accounts", {
    method: payload ? "POST" : "GET",
    headers: { "Content-Type": "application/json" },
    ...(payload ? { body: JSON.stringify(payload) } : {}),
    signal: AbortSignal.timeout(45000),
  });
  const data = await response.json();
  if (!data.success) throw Error(data.error || "账号操作失败");
  return data;
}
export function ManagedAccounts({
  initialProvider = "github_copilot",
  onSelect,
  onClose,
}: {
  initialProvider?: string;
  onSelect?: (binding: AuthBinding) => void;
  onClose: () => void;
}) {
  const [provider, setProvider] = useState(initialProvider),
    [groups, setGroups] = useState<Record<string, Group>>({}),
    [login, setLogin] = useState<Login | null>(null),
    [busy, setBusy] = useState(false),
    [error, setError] = useState(""),
    [domain, setDomain] = useState(""),
    [confirm, setConfirm] = useState<string | null>(null);
  const session = useRef<string | null>(null),
    mounted = useRef(true);
  const load = useCallback(async () => {
    try {
      setGroups((await accountRequest()).providers);
      setError("");
    } catch (e) {
      setError(
        e instanceof TypeError
          ? "无法连接本地组件，请确认组件已运行。"
          : e instanceof Error
            ? e.message
            : "读取账号失败",
      );
    }
  }, []);
  useEffect(() => {
    mounted.current = true;
    void load();
    return () => {
      mounted.current = false;
      if (session.current)
        void accountRequest({
          action: "cancel",
          sessionId: session.current,
        }).catch(() => {});
    };
  }, [load]);
  useEffect(() => {
    if (!login) return;
    let active = true,
      timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try {
        const result = await accountRequest({
          action: "poll",
          sessionId: login.sessionId,
        });
        if (!active) return;
        if (!result.pending) {
          session.current = null;
          setLogin(null);
          await load();
          if (result.account)
            onSelect?.({
              authProvider: provider,
              accountId: result.account.id,
              accountName: result.account.login,
            });
        } else
          timer = setTimeout(poll, (result.interval || login.interval) * 1000);
      } catch (e) {
        if (active) {
          session.current = null;
          setLogin(null);
          setError(e instanceof Error ? e.message : "登录失败");
        }
      }
    };
    timer = setTimeout(poll, login.interval * 1000);
    return () => {
      active = false;
      clearTimeout(timer);
    };
  }, [login, load, provider, onSelect]);
  const start = async (accountId?: string) => {
    setBusy(true);
    setError("");
    try {
      const result = await accountRequest({
        action: "start",
        provider,
        accountId,
        domain: domain || undefined,
      });
      if (!mounted.current) {
        void accountRequest({ action: "cancel", sessionId: result.sessionId });
        return;
      }
      session.current = result.sessionId;
      setLogin(result);
    } catch (e) {
      setError(e instanceof Error ? e.message : "无法启动登录");
    } finally {
      setBusy(false);
    }
  };
  const manage = async (action: string, accountId: string) => {
    setBusy(true);
    setError("");
    try {
      await accountRequest({ action, provider, accountId });
      setConfirm(null);
      await load();
      if (action === "check") toast.success("账号凭证有效");
    } catch (e) {
      setError(e instanceof Error ? e.message : "账号操作失败");
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog open onOpenChange={(open) => !open && !busy && onClose()}>
      <DialogContent
        showCloseButton={false}
        className="w-[calc(100vw-1.5rem)] max-w-[42rem] max-h-[88dvh] grid-rows-[auto_minmax(0,1fr)] grid-cols-[minmax(0,1fr)] gap-0 overflow-hidden p-0 sm:max-w-[42rem]"
      >
        <DialogHeader className="flex flex-row items-center justify-between border-b border-border-default px-5 py-4">
          <DialogTitle>托管账号</DialogTitle>
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label="关闭托管账号"
            disabled={busy}
            onClick={onClose}
          >
            <X />
          </Button>
        </DialogHeader>
        <div className="min-h-0 space-y-4 overflow-y-auto p-5">
          <label htmlFor="managed-auth-provider" className="sr-only">
            账号类型
          </label>
          <SelectField
            id="managed-auth-provider"
            value={provider}
            disabled={busy || !!login}
            onValueChange={setProvider}
            options={Object.entries(authNames).map(([value, label]) => ({
              value,
              label,
            }))}
          />
          {login ? (
            <section
              className="space-y-4 rounded-xl border border-border-default p-4"
              aria-label="设备授权"
            >
              <p className="text-sm text-muted-foreground">
                打开授权页，输入以下代码并完成登录。
              </p>
              <div className="flex items-center justify-between gap-2 rounded-lg bg-bg-sunken px-4 py-3">
                <code className="min-w-0 break-all text-lg font-semibold tracking-wider">
                  {login.userCode}
                </code>
                <Button
                  variant="ghost"
                  size="icon-sm"
                  aria-label="复制授权代码"
                  onClick={() =>
                    void navigator.clipboard
                      .writeText(login.userCode)
                      .then(() => toast.success("已复制授权代码"))
                      .catch(() => setError("复制失败，请手动选择代码"))
                  }
                >
                  <Copy />
                </Button>
              </div>
              <div className="flex flex-wrap items-center gap-3">
                <a
                  href={login.verificationUri}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="inline-flex min-h-10 items-center gap-2 rounded-lg bg-accent-solid px-4 text-sm font-semibold text-primary-foreground"
                >
                  打开授权页
                  <ExternalLink className="size-4" />
                </a>
                <Button
                  variant="ghost"
                  onClick={() => {
                    void accountRequest({
                      action: "cancel",
                      sessionId: login.sessionId,
                    });
                    session.current = null;
                    setLogin(null);
                  }}
                >
                  取消登录
                </Button>
              </div>
              <p
                role="status"
                className="flex items-center gap-2 text-sm text-muted-foreground"
              >
                <Loader2 className="size-4 animate-spin" />
                等待授权完成，页面会自动更新。
              </p>
            </section>
          ) : (
            <>
              <div className="flex flex-wrap items-center justify-between gap-2">
                <p className="text-sm text-muted-foreground">
                  凭证保存在本机，可绑定到多个供应商。
                </p>
                <div className="flex gap-1">
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    aria-label="刷新账号"
                    onClick={() => void load()}
                    disabled={busy}
                  >
                    <RefreshCw />
                  </Button>
                  <Button
                    size="sm"
                    onClick={() => void start()}
                    disabled={busy}
                  >
                    <Plus />
                    添加账号
                  </Button>
                </div>
              </div>
              {provider === "github_copilot" && (
                <details className="text-sm">
                  <summary className="cursor-pointer text-muted-foreground">
                    GitHub Enterprise
                  </summary>
                  <label htmlFor="github-domain" className="mt-3 block text-sm">
                    企业域名
                  </label>
                  <Input
                    id="github-domain"
                    value={domain}
                    onChange={(e) => setDomain(e.target.value)}
                    placeholder="留空使用 github.com"
                  />
                </details>
              )}
              <div className="divide-y divide-border-default rounded-xl border border-border-default">
                {(groups[provider]?.accounts || []).map((account) => (
                  <div key={account.id} className="space-y-2 p-3">
                    <div className="flex flex-wrap items-center gap-2">
                      <div className="min-w-0 flex-1">
                        <p className="truncate text-sm font-medium">
                          {account.login}
                        </p>
                        <p className="text-xs text-muted-foreground">
                          {account.reauth_required || account.requires_reauth
                            ? "需要重新登录"
                            : groups[provider]?.default_account_id ===
                                account.id
                              ? "默认账号"
                              : "已登录"}
                        </p>
                      </div>
                      {onSelect && (
                        <Button
                          variant="outline"
                          size="sm"
                          disabled={busy}
                          onClick={() =>
                            onSelect({
                              authProvider: provider,
                              accountId: account.id,
                              accountName: account.login,
                            })
                          }
                        >
                          选择
                        </Button>
                      )}
                      <Button
                        variant="ghost"
                        size="icon-sm"
                        aria-label={`检测账号 ${account.login}`}
                        disabled={busy}
                        onClick={() => void manage("check", account.id)}
                      >
                        <Check />
                      </Button>
                      <Button
                        variant="ghost"
                        size="icon-sm"
                        aria-label={`设为默认 ${account.login}`}
                        disabled={
                          busy ||
                          groups[provider]?.default_account_id === account.id
                        }
                        onClick={() => void manage("default", account.id)}
                      >
                        <Star />
                      </Button>
                      <Button
                        variant="ghost"
                        size="icon-sm"
                        aria-label={`重新登录 ${account.login}`}
                        disabled={busy}
                        onClick={() => void start(account.id)}
                      >
                        <RefreshCw />
                      </Button>
                      <Button
                        variant="ghost"
                        size="icon-sm"
                        aria-label={`删除账号 ${account.login}`}
                        disabled={busy}
                        onClick={() => setConfirm(account.id)}
                      >
                        <Trash2 />
                      </Button>
                    </div>
                    {confirm === account.id && (
                      <div
                        role="alert"
                        className="flex flex-wrap items-center gap-2 rounded-lg bg-bg-sunken p-3 text-sm"
                      >
                        <span className="min-w-0 flex-1">
                          删除后，绑定此账号的供应商将无法调用。
                        </span>
                        <Button
                          variant="ghost"
                          size="sm"
                          onClick={() => setConfirm(null)}
                        >
                          取消
                        </Button>
                        <Button
                          variant="destructive"
                          size="sm"
                          disabled={busy}
                          onClick={() => void manage("remove", account.id)}
                        >
                          删除
                        </Button>
                      </div>
                    )}
                  </div>
                ))}
                {groups[provider]?.accounts.length === 0 && (
                  <p className="p-6 text-center text-sm text-muted-foreground">
                    尚未添加账号
                  </p>
                )}
              </div>
            </>
          )}
          {(error || groups[provider]?.migration_error) && (
            <p role="alert" className="break-words text-sm text-danger-text">
              {error || groups[provider]?.migration_error}
            </p>
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}
