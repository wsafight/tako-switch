import { useTranslation } from "react-i18next";
import { useState, useEffect } from "react";
import {
  ChevronDown,
  ChevronRight,
  FlaskConical,
  Coins,
  Terminal,
  LogIn,
  Boxes,
} from "lucide-react";
import {
  takoStatuslineStatus,
  takoStatuslineEnable,
  takoStatuslineDisable,
  takoApplyKey,
  takoListModels,
  type TakoModel,
} from "@/lib/api/tako";
import { startTakoLogin } from "@/lib/takoAuth";
import { TakoModelsList } from "./TakoModelsList";
import { toast } from "sonner";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { cn } from "@/lib/utils";
import type { ProviderTestConfig } from "@/types";

export type PricingModelSourceOption = "inherit" | "request" | "response";

interface ProviderPricingConfig {
  enabled: boolean;
  costMultiplier?: string;
  pricingModelSource: PricingModelSourceOption;
}

interface ProviderAdvancedConfigProps {
  testConfig: ProviderTestConfig;
  pricingConfig: ProviderPricingConfig;
  onTestConfigChange: (config: ProviderTestConfig) => void;
  onPricingConfigChange: (config: ProviderPricingConfig) => void;
  /** True when editing the built-in Tako provider — unlocks Tako-only options. */
  isTako?: boolean;
  /** Current cr_ key (Tako provider only) — used to list supported models. */
  takoApiKey?: string;
}

export function ProviderAdvancedConfig({
  testConfig,
  pricingConfig,
  onTestConfigChange,
  onPricingConfigChange,
  isTako = false,
  takoApiKey = "",
}: ProviderAdvancedConfigProps) {
  const { t } = useTranslation();
  const [isTestConfigOpen, setIsTestConfigOpen] = useState(testConfig.enabled);
  const [isPricingConfigOpen, setIsPricingConfigOpen] = useState(
    pricingConfig.enabled,
  );

  useEffect(() => {
    setIsTestConfigOpen(testConfig.enabled);
  }, [testConfig.enabled]);

  useEffect(() => {
    setIsPricingConfigOpen(pricingConfig.enabled);
  }, [pricingConfig.enabled]);

  // Tako-only: statusline injection state (reads ~/.claude/settings.json).
  const [statuslineOn, setStatuslineOn] = useState(false);
  useEffect(() => {
    if (isTako) {
      takoStatuslineStatus().then(setStatuslineOn).catch(() => {});
    }
  }, [isTako]);

  const toggleStatusline = async (next: boolean) => {
    try {
      if (next) await takoStatuslineEnable();
      else await takoStatuslineDisable();
      setStatuslineOn(next);
    } catch (e) {
      console.error("[Tako] statusline toggle failed", e);
    }
  };

  // Tako-only: OAuth 浏览器授权登录 + 手动粘贴兜底。
  const [loggingIn, setLoggingIn] = useState(false);
  const [showPaste, setShowPaste] = useState(false);
  const [pasteKey, setPasteKey] = useState("");

  const handleBrowserLogin = async () => {
    setLoggingIn(true);
    try {
      const r = await startTakoLogin();
      if (r.ok) {
        toast.success(`已登录${r.name ? `：${r.name}` : ""}`);
      } else {
        toast.error(r.error || "登录失败");
      }
    } finally {
      setLoggingIn(false);
    }
  };

  const handlePasteLogin = async () => {
    const key = pasteKey.trim();
    if (!key) return;
    setLoggingIn(true);
    try {
      const r = await takoApplyKey(key);
      if (r.ok) {
        toast.success(`已登录${r.name ? `：${r.name}` : ""}`);
        setPasteKey("");
        setShowPaste(false);
      } else {
        toast.error(r.error || "Key 无效");
      }
    } catch (e) {
      toast.error(String(e));
    } finally {
      setLoggingIn(false);
    }
  };

  // Tako-only: 支持的模型展示（懒加载，展开时拉取）。
  const [showModels, setShowModels] = useState(false);
  const [models, setModels] = useState<TakoModel[] | null>(null);
  const [modelsLoading, setModelsLoading] = useState(false);

  const toggleModels = async () => {
    const next = !showModels;
    setShowModels(next);
    if (next && models === null && takoApiKey.trim()) {
      setModelsLoading(true);
      try {
        setModels(await takoListModels(takoApiKey.trim()));
      } catch (e) {
        toast.error(`获取模型失败：${e}`);
        setModels([]);
      } finally {
        setModelsLoading(false);
      }
    }
  };

  return (
    <div className="space-y-4">
      {isTako && (
        <div className="rounded-lg border border-border/50 bg-muted/20 p-4 space-y-3">
          <div className="flex items-center justify-between">
            <div className="flex items-center gap-3">
              <LogIn className="h-4 w-4 text-muted-foreground" />
              <div>
                <Label className="text-sm">
                  {t("providerAdvanced.takoLogin.title", {
                    defaultValue: "使用 Tako 账号登录",
                  })}
                </Label>
                <p className="text-xs text-muted-foreground">
                  {t("providerAdvanced.takoLogin.description", {
                    defaultValue: "在浏览器中授权，自动填入 API Key",
                  })}
                </p>
              </div>
            </div>
            <button
              type="button"
              disabled={loggingIn}
              onClick={handleBrowserLogin}
              className="rounded-lg bg-[var(--app-link)] px-3 py-1.5 text-sm font-medium text-[var(--app-bg)] disabled:opacity-50"
            >
              {loggingIn
                ? t("common.loading", { defaultValue: "处理中…" })
                : t("providerAdvanced.takoLogin.button", {
                    defaultValue: "浏览器登录",
                  })}
            </button>
          </div>
          <button
            type="button"
            onClick={() => setShowPaste((v) => !v)}
            className="text-xs text-muted-foreground hover:text-foreground"
          >
            {showPaste
              ? t("common.collapse", { defaultValue: "收起" })
              : t("providerAdvanced.takoLogin.pasteToggle", {
                  defaultValue: "或手动粘贴 API Key",
                })}
          </button>
          {showPaste && (
            <div className="flex gap-2">
              <Input
                value={pasteKey}
                onChange={(e) => setPasteKey(e.target.value)}
                placeholder="cr_..."
                className="flex-1"
              />
              <button
                type="button"
                disabled={loggingIn || !pasteKey.trim()}
                onClick={handlePasteLogin}
                className="rounded-lg border border-input px-3 py-1.5 text-sm disabled:opacity-50"
              >
                {t("common.confirm", { defaultValue: "确认" })}
              </button>
            </div>
          )}
        </div>
      )}
      {isTako && (
        <div className="rounded-lg border border-border/50 bg-muted/20 p-4">
          <div className="flex items-center justify-between">
            <div className="flex items-center gap-3">
              <Terminal className="h-4 w-4 text-muted-foreground" />
              <div>
                <Label className="text-sm">
                  {t("providerAdvanced.takoStatusline.title", {
                    defaultValue: "Tako 状态栏",
                  })}
                </Label>
                <p className="text-xs text-muted-foreground">
                  {t("providerAdvanced.takoStatusline.description", {
                    defaultValue:
                      "在 Claude Code 底部显示目录 / Git / 模型 / 上下文 / 配额",
                  })}
                </p>
              </div>
            </div>
            <Switch checked={statuslineOn} onCheckedChange={toggleStatusline} />
          </div>
        </div>
      )}
      {isTako && (
        <div className="rounded-lg border border-border/50 bg-muted/20">
          <button
            type="button"
            className="flex w-full items-center justify-between p-4 hover:bg-muted/30 transition-colors"
            onClick={toggleModels}
          >
            <div className="flex items-center gap-3">
              <Boxes className="h-4 w-4 text-muted-foreground" />
              <span className="font-medium">
                {t("providerAdvanced.takoModels.title", {
                  defaultValue: "支持的模型",
                })}
              </span>
            </div>
            {showModels ? (
              <ChevronDown className="h-4 w-4 text-muted-foreground" />
            ) : (
              <ChevronRight className="h-4 w-4 text-muted-foreground" />
            )}
          </button>
          {showModels && (
            <div className="border-t border-border/50 p-4">
              <TakoModelsList
                loading={modelsLoading}
                models={models}
                hasKey={!!takoApiKey.trim()}
              />
            </div>
          )}
        </div>
      )}
      <div className="rounded-lg border border-border/50 bg-muted/20">
        <button
          type="button"
          className="flex w-full items-center justify-between p-4 hover:bg-muted/30 transition-colors"
          onClick={() => setIsTestConfigOpen(!isTestConfigOpen)}
        >
          <div className="flex items-center gap-3">
            <FlaskConical className="h-4 w-4 text-muted-foreground" />
            <span className="font-medium">
              {t("providerAdvanced.testConfig", {
                defaultValue: "模型测试配置",
              })}
            </span>
          </div>
          <div className="flex items-center gap-3">
            <div
              className="flex items-center gap-2"
              onClick={(e) => e.stopPropagation()}
            >
              <Label
                htmlFor="test-config-enabled"
                className="text-sm text-muted-foreground"
              >
                {t("providerAdvanced.useCustomConfig", {
                  defaultValue: "使用单独配置",
                })}
              </Label>
              <Switch
                id="test-config-enabled"
                checked={testConfig.enabled}
                onCheckedChange={(checked) => {
                  onTestConfigChange({ ...testConfig, enabled: checked });
                  if (checked) setIsTestConfigOpen(true);
                }}
              />
            </div>
            {isTestConfigOpen ? (
              <ChevronDown className="h-4 w-4 text-muted-foreground" />
            ) : (
              <ChevronRight className="h-4 w-4 text-muted-foreground" />
            )}
          </div>
        </button>
        <div
          className={cn(
            "overflow-hidden transition-all duration-200",
            isTestConfigOpen
              ? "max-h-[500px] opacity-100"
              : "max-h-0 opacity-0",
          )}
        >
          <div className="border-t border-border/50 p-4 space-y-4">
            <p className="text-sm text-muted-foreground">
              {t("providerAdvanced.testConfigDesc", {
                defaultValue:
                  "为此供应商配置单独的模型测试参数，不启用时使用全局配置。",
              })}
            </p>
            <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
              <div className="space-y-2">
                <Label htmlFor="test-model">
                  {t("providerAdvanced.testModel", {
                    defaultValue: "测试模型",
                  })}
                </Label>
                <Input
                  id="test-model"
                  value={testConfig.testModel || ""}
                  onChange={(e) =>
                    onTestConfigChange({
                      ...testConfig,
                      testModel: e.target.value || undefined,
                    })
                  }
                  placeholder={t("providerAdvanced.testModelPlaceholder", {
                    defaultValue: "留空使用全局配置",
                  })}
                  disabled={!testConfig.enabled}
                />
              </div>
              <div className="space-y-2">
                <Label htmlFor="test-timeout">
                  {t("providerAdvanced.timeoutSecs", {
                    defaultValue: "超时时间（秒）",
                  })}
                </Label>
                <Input
                  id="test-timeout"
                  type="number"
                  min={1}
                  max={300}
                  value={testConfig.timeoutSecs || ""}
                  onChange={(e) =>
                    onTestConfigChange({
                      ...testConfig,
                      timeoutSecs: e.target.value
                        ? parseInt(e.target.value, 10)
                        : undefined,
                    })
                  }
                  placeholder="45"
                  disabled={!testConfig.enabled}
                />
              </div>
              <div className="space-y-2">
                <Label htmlFor="test-prompt">
                  {t("providerAdvanced.testPrompt", {
                    defaultValue: "测试提示词",
                  })}
                </Label>
                <Input
                  id="test-prompt"
                  value={testConfig.testPrompt || ""}
                  onChange={(e) =>
                    onTestConfigChange({
                      ...testConfig,
                      testPrompt: e.target.value || undefined,
                    })
                  }
                  placeholder="Who are you?"
                  disabled={!testConfig.enabled}
                />
              </div>
              <div className="space-y-2">
                <Label htmlFor="degraded-threshold">
                  {t("providerAdvanced.degradedThreshold", {
                    defaultValue: "降级阈值（毫秒）",
                  })}
                </Label>
                <Input
                  id="degraded-threshold"
                  type="number"
                  min={100}
                  max={60000}
                  value={testConfig.degradedThresholdMs || ""}
                  onChange={(e) =>
                    onTestConfigChange({
                      ...testConfig,
                      degradedThresholdMs: e.target.value
                        ? parseInt(e.target.value, 10)
                        : undefined,
                    })
                  }
                  placeholder="6000"
                  disabled={!testConfig.enabled}
                />
              </div>
              <div className="space-y-2">
                <Label htmlFor="max-retries">
                  {t("providerAdvanced.maxRetries", {
                    defaultValue: "最大重试次数",
                  })}
                </Label>
                <Input
                  id="max-retries"
                  type="number"
                  min={0}
                  max={10}
                  value={testConfig.maxRetries ?? ""}
                  onChange={(e) =>
                    onTestConfigChange({
                      ...testConfig,
                      maxRetries: e.target.value
                        ? parseInt(e.target.value, 10)
                        : undefined,
                    })
                  }
                  placeholder="2"
                  disabled={!testConfig.enabled}
                />
              </div>
            </div>
          </div>
        </div>
      </div>

      {/* 计费配置 */}
      <div className="rounded-lg border border-border/50 bg-muted/20">
        <button
          type="button"
          className="flex w-full items-center justify-between p-4 hover:bg-muted/30 transition-colors"
          onClick={() => setIsPricingConfigOpen(!isPricingConfigOpen)}
        >
          <div className="flex items-center gap-3">
            <Coins className="h-4 w-4 text-muted-foreground" />
            <span className="font-medium">
              {t("providerAdvanced.pricingConfig", {
                defaultValue: "计费配置",
              })}
            </span>
          </div>
          <div className="flex items-center gap-3">
            <div
              className="flex items-center gap-2"
              onClick={(e) => e.stopPropagation()}
            >
              <Label
                htmlFor="pricing-config-enabled"
                className="text-sm text-muted-foreground"
              >
                {t("providerAdvanced.useCustomPricing", {
                  defaultValue: "使用单独配置",
                })}
              </Label>
              <Switch
                id="pricing-config-enabled"
                checked={pricingConfig.enabled}
                onCheckedChange={(checked) => {
                  onPricingConfigChange({ ...pricingConfig, enabled: checked });
                  if (checked) setIsPricingConfigOpen(true);
                }}
              />
            </div>
            {isPricingConfigOpen ? (
              <ChevronDown className="h-4 w-4 text-muted-foreground" />
            ) : (
              <ChevronRight className="h-4 w-4 text-muted-foreground" />
            )}
          </div>
        </button>
        <div
          className={cn(
            "overflow-hidden transition-all duration-200",
            isPricingConfigOpen
              ? "max-h-[500px] opacity-100"
              : "max-h-0 opacity-0",
          )}
        >
          <div className="border-t border-border/50 p-4 space-y-4">
            <p className="text-sm text-muted-foreground">
              {t("providerAdvanced.pricingConfigDesc", {
                defaultValue:
                  "为此供应商配置单独的计费参数，不启用时使用全局默认配置。",
              })}
            </p>
            <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
              <div className="space-y-2">
                <Label htmlFor="cost-multiplier">
                  {t("providerAdvanced.costMultiplier", {
                    defaultValue: "成本倍率",
                  })}
                </Label>
                <Input
                  id="cost-multiplier"
                  type="number"
                  step="0.01"
                  min="0"
                  inputMode="decimal"
                  value={pricingConfig.costMultiplier || ""}
                  onChange={(e) =>
                    onPricingConfigChange({
                      ...pricingConfig,
                      costMultiplier: e.target.value || undefined,
                    })
                  }
                  placeholder={t("providerAdvanced.costMultiplierPlaceholder", {
                    defaultValue: "留空使用全局默认（1）",
                  })}
                  disabled={!pricingConfig.enabled}
                />
                <p className="text-xs text-muted-foreground">
                  {t("providerAdvanced.costMultiplierHint", {
                    defaultValue: "实际成本 = 基础成本 × 倍率，支持小数如 1.5",
                  })}
                </p>
              </div>
              <div className="space-y-2">
                <Label htmlFor="pricing-model-source">
                  {t("providerAdvanced.pricingModelSourceLabel", {
                    defaultValue: "计费模式",
                  })}
                </Label>
                <Select
                  value={pricingConfig.pricingModelSource}
                  onValueChange={(value) =>
                    onPricingConfigChange({
                      ...pricingConfig,
                      pricingModelSource: value as PricingModelSourceOption,
                    })
                  }
                  disabled={!pricingConfig.enabled}
                >
                  <SelectTrigger id="pricing-model-source">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="inherit">
                      {t("providerAdvanced.pricingModelSourceInherit", {
                        defaultValue: "继承全局默认",
                      })}
                    </SelectItem>
                    <SelectItem value="request">
                      {t("providerAdvanced.pricingModelSourceRequest", {
                        defaultValue: "请求模型",
                      })}
                    </SelectItem>
                    <SelectItem value="response">
                      {t("providerAdvanced.pricingModelSourceResponse", {
                        defaultValue: "返回模型",
                      })}
                    </SelectItem>
                  </SelectContent>
                </Select>
                <p className="text-xs text-muted-foreground">
                  {t("providerAdvanced.pricingModelSourceHint", {
                    defaultValue: "选择按请求模型还是返回模型进行定价匹配",
                  })}
                </p>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
