"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { ListOrdered, Radio, Shuffle } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { consoleAPI, type IntegrationClient } from "@/lib/api";
import { localProxy } from "./local-proxy-panel";
import { toast } from "sonner";

export function ProxyControls({
  client,
  activeId,
  onQueue,
  onChange,
}: {
  client: IntegrationClient;
  activeId?: string;
  onQueue: () => void;
  onChange: () => void;
}) {
  const [state, setState] = useState({ enabled: false, failover: false });
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const pending = useRef(false);
  const refresh = useCallback(async () => {
    try {
      const data = await localProxy("status");
      const route = data.routes.find((r) => r.client === client);
      setState({
        enabled: Boolean(route),
        failover: route?.autoFailoverEnabled ?? false,
      });
    } catch {
      setState({ enabled: false, failover: false });
    } finally {
      setLoading(false);
    }
  }, [client]);
  useEffect(() => {
    void refresh();
    window.addEventListener("junohub-proxy-change", refresh);
    return () => window.removeEventListener("junohub-proxy-change", refresh);
  }, [refresh, activeId]);
  const toggle = async (kind: "takeover" | "failover") => {
    if (pending.current) return;
    pending.current = true;
    setBusy(true);
    try {
      if (kind === "failover") {
        await localProxy("settings", {
          client,
          autoFailoverEnabled: !state.failover,
        });
      } else if (state.enabled) {
        await localProxy("disable", { client });
      } else {
        if (!activeId) throw Error("请先启用一个供应商，再开启客户端接管。");
        const pair = await consoleAPI.integrationSync(activeId, client);
        await localProxy("configure", {
          client,
          pairUrls: [
            `${window.location.origin}/client-connect/redeem?token=${pair.token}`,
          ],
          autoFailoverEnabled: false,
        });
      }
      await refresh();
      onChange();
    } catch (error) {
      toast.error(error instanceof Error ? error.message : "代理操作失败");
    } finally {
      pending.current = false;
      setBusy(false);
    }
  };
  return (
    <div className="flex flex-wrap items-center gap-2">
      {(
        [
          {
            key: "takeover",
            label: "客户端接管",
            checked: state.enabled,
            icon: Radio,
          },
          {
            key: "failover",
            label: "自动故障转移",
            checked: state.failover,
            icon: Shuffle,
          },
        ] as const
      ).map(({ key, label, checked, icon: Icon }) => (
        <div
          key={key}
          className="flex min-h-9 items-center gap-3 rounded-lg bg-bg-sunken px-3 text-muted-foreground"
        >
          <Icon className={`size-4 ${checked ? "text-success-text" : ""}`} />
          <Switch
            checked={checked}
            onCheckedChange={() => void toggle(key)}
            aria-label={label}
            aria-describedby={key === "failover" ? `${client}-failover-help` : undefined}
            disabled={loading || busy || (key === "failover" && !state.enabled) || (key === "takeover" && !state.enabled && !activeId)}
          />
          {key === "failover" && <span id={`${client}-failover-help`} className="sr-only">请先开启客户端接管，再设置自动故障转移。</span>}
        </div>
      ))}
      <Button
        variant="ghost"
        size="icon-sm"
        aria-label="故障转移队列"
        title="故障转移队列"
        onClick={onQueue}
      >
        <ListOrdered />
      </Button>
    </div>
  );
}
