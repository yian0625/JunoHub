"use client";
import { Plus, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { SelectField } from "@/components/ui/select-field";
import { record, str, type Config } from "@/lib/cc-switch/native-config";
const levels = [
  "none",
  "minimal",
  "low",
  "medium",
  "high",
  "xhigh",
  "max",
  "ultra",
];
export function CodexCatalog({
  value,
  onChange,
}: {
  value: unknown;
  onChange: (value: Config) => void;
}) {
  const catalog = record(value),
    models = (
      Array.isArray(value)
        ? value
        : Array.isArray(catalog.models)
          ? catalog.models
          : []
    ).map(record);
  const save = (rows: Config[]) => onChange({ ...catalog, models: rows });
  const update = (index: number, patch: Config) =>
    save(models.map((m, i) => (i === index ? { ...m, ...patch } : m)));
  return (
    <details className="rounded-lg border border-border-default">
      <summary className="cursor-pointer px-4 py-3 text-sm font-medium">
        模型目录{models.length ? `（${models.length}）` : ""}
      </summary>
      <div className="space-y-3 border-t border-border-default p-3">
        <p className="text-xs text-muted-foreground">
          配置会同步到 Codex 模型选择器，声明每个模型支持的上下文和推理档位。
        </p>
        {models.map((model, index) => (
          <fieldset
            key={index}
            className="space-y-3 rounded-lg border border-border-default p-3"
          >
            <legend className="px-1 text-sm">模型 {index + 1}</legend>
            <div className="flex items-end gap-2">
              <label className="min-w-0 flex-1 space-y-1 text-sm">
                模型 ID
                <Input
                  value={str(model.model) || str(model.id)}
                  onChange={(e) => update(index, { model: e.target.value })}
                  aria-label={`目录模型 ${index + 1} ID`}
                />
              </label>
              <Button
                size="icon-sm"
                variant="ghost"
                aria-label={`删除目录模型 ${index + 1}`}
                onClick={() => save(models.filter((_, i) => i !== index))}
              >
                <Trash2 />
              </Button>
            </div>
            <div className="grid gap-3 sm:grid-cols-2">
              <label className="space-y-1 text-sm">
                显示名称
                <Input
                  value={str(model.displayName)}
                  onChange={(e) =>
                    update(index, { displayName: e.target.value })
                  }
                />
              </label>
              <label className="space-y-1 text-sm">
                上下文窗口
                <Input
                  type="number"
                  min="1"
                  placeholder="128000"
                  value={
                    typeof model.contextWindow === "number"
                      ? model.contextWindow
                      : ""
                  }
                  onChange={(e) =>
                    update(index, {
                      contextWindow: e.target.value
                        ? Number(e.target.value)
                        : undefined,
                    })
                  }
                />
              </label>
            </div>
            <NativePricing
              value={record(model.cost)}
              onChange={(cost) => update(index, { cost })}
            />
            <fieldset>
              <legend className="mb-2 text-sm">推理档位</legend>
              <div className="flex flex-wrap gap-x-4 gap-y-2">
                {levels.map((level) => (
                  <label
                    className="flex items-center gap-1.5 text-sm"
                    key={level}
                  >
                    <input
                      type="checkbox"
                      checked={
                        Array.isArray(model.reasoningLevels) &&
                        model.reasoningLevels.includes(level)
                      }
                      onChange={(e) => {
                        const old = Array.isArray(model.reasoningLevels)
                          ? model.reasoningLevels
                          : [];
                        update(index, {
                          reasoningLevels: e.target.checked
                            ? [...old, level]
                            : old.filter((v) => v !== level),
                        });
                      }}
                    />
                    {level}
                  </label>
                ))}
              </div>
            </fieldset>
            <label className="block space-y-1 text-sm">
              默认推理档位
              <SelectField
                id={`catalog-default-${index}`}
                value={str(model.defaultReasoningLevel) || "auto"}
                onValueChange={(v) =>
                  update(index, {
                    defaultReasoningLevel: v === "auto" ? undefined : v,
                  })
                }
                options={[
                  { value: "auto", label: "自动" },
                  ...levels
                    .filter(
                      (v) =>
                        Array.isArray(model.reasoningLevels) &&
                        model.reasoningLevels.includes(v),
                    )
                    .map((value) => ({ value, label: value })),
                ]}
              />
            </label>
          </fieldset>
        ))}
        <Button
          size="sm"
          variant="outline"
          onClick={() => save([...models, { model: "", displayName: "" }])}
        >
          <Plus />
          添加模型
        </Button>
      </div>
    </details>
  );
}
export function CodexReasoning({
  value,
  onChange,
}: {
  value: Config;
  onChange: (value: Config) => void;
}) {
  const change = (key: string, val: unknown) => {
    const next = { ...value };
    if (val === undefined) delete next[key];
    else next[key] = val;
    onChange(next);
  };
  return (
    <details className="rounded-lg border border-border-default">
      <summary className="cursor-pointer px-4 py-3 text-sm font-medium">
        上游推理参数
      </summary>
      <div className="grid gap-3 border-t border-border-default p-3 sm:grid-cols-2">
        {[
          ["supportsThinking", "思考开关"],
          ["supportsEffort", "推理强度参数"],
        ].map(([key, label]) => (
          <label className="space-y-1 text-sm" key={key}>
            {label}
            <SelectField
              id={`reasoning-${key}`}
              value={
                typeof value[key] === "boolean" ? String(value[key]) : "auto"
              }
              options={[
                { value: "auto", label: "自动" },
                { value: "true", label: "支持" },
                { value: "false", label: "不支持" },
              ]}
              onValueChange={(v) =>
                change(key, v === "auto" ? undefined : v === "true")
              }
            />
          </label>
        ))}
        {[
          ["thinkingParam", "思考参数路径", "enable_thinking"],
          ["effortParam", "强度参数路径", "reasoning_effort"],
        ].map(([key, label, placeholder]) => (
          <label className="space-y-1 text-sm" key={key}>
            {label}
            <Input
              value={str(value[key])}
              placeholder={placeholder}
              onChange={(e) => change(key, e.target.value || undefined)}
            />
          </label>
        ))}
        <label className="space-y-1 text-sm">
          强度值映射
          <SelectField
            id="effort-value-mode"
            value={str(value.effortValueMode) || "auto"}
            options={[
              { value: "auto", label: "自动" },
              ...[
                "passthrough",
                "deepseek",
                "low_high",
                "openrouter",
                "zen",
              ].map((value) => ({ value, label: value })),
            ]}
            onValueChange={(v) =>
              change("effortValueMode", v === "auto" ? undefined : v)
            }
          />
        </label>
      </div>
    </details>
  );
}
export function PiThinkingMap({
  value,
  onChange,
}: {
  value: Config;
  onChange: (value: Config) => void;
}) {
  return (
    <details className="rounded-lg border border-border-default">
      <summary className="cursor-pointer px-3 py-2.5 text-sm">
        思考等级映射
      </summary>
      <div className="space-y-2 border-t border-border-default p-3">
        {["off", "minimal", "low", "medium", "high", "xhigh", "max"].map(
          (level) => {
            const mode = !(level in value)
              ? "default"
              : value[level] === null
                ? "unsupported"
                : "value";
            return (
              <div
                key={level}
                className="grid grid-cols-[4rem_minmax(0,1fr)] items-center gap-2"
              >
                <span className="text-sm">{level}</span>
                <div className="grid min-w-0 gap-2 sm:grid-cols-2">
                  <SelectField
                    id={`pi-thinking-${level}`}
                    value={mode}
                    options={[
                      { value: "default", label: "跟随默认" },
                      { value: "unsupported", label: "不支持" },
                      { value: "value", label: "映射到" },
                    ]}
                    onValueChange={(v) => {
                      const next = { ...value };
                      if (v === "default") delete next[level];
                      else next[level] = v === "unsupported" ? null : level;
                      onChange(next);
                    }}
                  />
                  {mode === "value" && (
                    <Input
                      aria-label={`${level} 映射值`}
                      value={str(value[level])}
                      onChange={(e) =>
                        onChange({ ...value, [level]: e.target.value })
                      }
                    />
                  )}
                </div>
              </div>
            );
          },
        )}
      </div>
    </details>
  );
}

export function NativePricing({
  value,
  onChange,
}: {
  value: Config;
  onChange: (value: Config) => void;
}) {
  return (
    <details className="rounded-lg border border-border-default">
      <summary className="cursor-pointer px-3 py-2.5 text-sm">
        用量价格（美元 / 百万 Token）
      </summary>
      <div className="grid gap-3 border-t border-border-default p-3 sm:grid-cols-2">
        {[
          ["input", "输入"],
          ["output", "输出"],
          ["cacheRead", "缓存读取"],
          ["cacheWrite", "缓存写入"],
        ].map(([key, label]) => (
          <label className="space-y-1 text-sm" key={key}>
            {label}
            <Input
              type="number"
              min="0"
              step="any"
              value={typeof value[key] === "number" ? String(value[key]) : ""}
              placeholder="未配置"
              onChange={(e) => {
                const next = { ...value };
                if (e.target.value === "") delete next[key];
                else next[key] = Number(e.target.value);
                onChange(next);
              }}
            />
          </label>
        ))}
      </div>
    </details>
  );
}
