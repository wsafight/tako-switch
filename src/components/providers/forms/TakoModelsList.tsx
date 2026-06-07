import { useTranslation } from "react-i18next";
import type { TakoModel } from "@/lib/api/tako";

/** 客户端标识 → 展示名。 */
const CLIENT_LABELS: Record<string, string> = {
  claude: "Claude Code",
  codex: "Codex",
  gemini: "Gemini",
};

interface Props {
  loading: boolean;
  models: TakoModel[] | null;
  hasKey: boolean;
}

/** Tako 支持的模型列表：按厂商分组，展示模型名 + 适用客户端 chip。 */
export function TakoModelsList({ loading, models, hasKey }: Props) {
  const { t } = useTranslation();

  if (!hasKey) {
    return (
      <p className="text-xs text-muted-foreground">
        {t("providerAdvanced.takoModels.needLogin", {
          defaultValue: "请先登录或填入 API Key 后查看支持的模型",
        })}
      </p>
    );
  }

  if (loading) {
    return (
      <p className="text-xs text-muted-foreground">
        {t("common.loading", { defaultValue: "加载中…" })}
      </p>
    );
  }

  if (!models || models.length === 0) {
    return (
      <p className="text-xs text-muted-foreground">
        {t("providerAdvanced.takoModels.empty", {
          defaultValue: "暂无可用模型",
        })}
      </p>
    );
  }

  // 按厂商分组，保留接口返回顺序。
  const groups = new Map<string, TakoModel[]>();
  for (const m of models) {
    const key = m.provider || t("common.other", { defaultValue: "其他" });
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key)!.push(m);
  }

  return (
    <div className="space-y-4">
      {Array.from(groups.entries()).map(([provider, list]) => (
        <div key={provider} className="space-y-2">
          <div className="text-xs font-medium text-muted-foreground">
            {provider}
          </div>
          <div className="space-y-1.5">
            {list.map((m) => (
              <div
                key={m.id}
                className="flex items-center justify-between gap-2 rounded-md bg-background/60 px-2.5 py-1.5"
              >
                <span className="truncate text-sm">{m.name}</span>
                <div className="flex flex-shrink-0 gap-1">
                  {m.clients.map((c) => (
                    <span
                      key={c}
                      className="rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground"
                    >
                      {CLIENT_LABELS[c] ?? c}
                    </span>
                  ))}
                </div>
              </div>
            ))}
          </div>
        </div>
      ))}
    </div>
  );
}
