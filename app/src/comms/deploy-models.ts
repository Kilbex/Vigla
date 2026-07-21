export type SupervisorVendor = "claude";
export type WorkerVendor =
  | "claude"
  | "codex"
  | "gemini"
  | "antigravity"
  | "kiro"
  | "copilot";
export type WorkerCliModelValue = string | null;
export type WorkerCountChoice = "auto" | "1" | "2" | "3" | "4" | "5";

export const SUPERVISOR_OPTIONS: readonly SupervisorVendor[] = ["claude"];
export const WORKER_OPTIONS: readonly WorkerVendor[] = [
  "claude",
  "codex",
  "antigravity",
  "kiro",
  "copilot",
  "gemini",
];
export const WORKER_COUNT_OPTIONS: readonly WorkerCountChoice[] = [
  "auto",
  "1",
  "2",
  "3",
  "4",
  "5",
];

export const MODEL_LABEL: Record<SupervisorVendor, string> = {
  claude: "Claude",
};

export const WORKER_LABEL: Record<WorkerVendor, string> = {
  claude: "Claude",
  codex: "Codex",
  gemini: "Gemini (legacy)",
  antigravity: "Antigravity",
  kiro: "Kiro",
  copilot: "Copilot",
};

export const COUNT_LABEL: Record<WorkerCountChoice, string> = {
  auto: "Auto",
  "1": "1",
  "2": "2",
  "3": "3",
  "4": "4",
  "5": "5",
};

export const DEFAULT_WORKER_MODELS: readonly WorkerVendor[] = [
  "claude",
  "codex",
  "antigravity",
  "claude",
  "codex",
];

interface WorkerCliModelOption {
  value: WorkerCliModelValue;
  label: string;
  detail: string;
}

export const CLI_MODEL_OPTIONS: Record<
  WorkerVendor,
  readonly WorkerCliModelOption[]
> = {
  claude: [
    {
      value: null,
      label: "Default",
      detail: "Use Claude Code's current account and provider default",
    },
    {
      value: "sonnet",
      label: "Sonnet",
      detail: "Sonnet 4.6 · Best for everyday tasks",
    },
    {
      value: "haiku",
      label: "Haiku",
      detail: "Haiku 4.5 · Fastest for quick answers",
    },
  ],
  codex: [
    {
      value: null,
      label: "Default",
      detail: "Use Codex CLI's current account and configuration default",
    },
    {
      value: "gpt-5.5",
      label: "gpt-5.5",
      detail: "Frontier model for complex coding, research, and real-world work",
    },
    {
      value: "gpt-5.4",
      label: "gpt-5.4",
      detail: "Strong model for everyday coding",
    },
    {
      value: "gpt-5.4-mini",
      label: "gpt-5.4-mini",
      detail: "Small, fast, and cost-efficient model for simpler coding tasks",
    },
    {
      value: "gpt-5.3-codex",
      label: "gpt-5.3-codex",
      detail: "Coding-optimized model",
    },
    {
      value: "gpt-5.3-codex-spark",
      label: "gpt-5.3-codex-spark",
      detail: "Ultra-fast coding model",
    },
    {
      value: "gpt-5.2",
      label: "gpt-5.2",
      detail: "Optimized for professional work and long-running agents",
    },
  ],
  gemini: [
    {
      value: "auto",
      label: "Auto",
      detail:
        "Legacy / enterprise compatibility; consumer Login with Google has ended",
    },
    {
      value: "pro",
      label: "Pro",
      detail: "Complex reasoning and coding tasks",
    },
    {
      value: "flash",
      label: "Flash",
      detail: "Fast, balanced model for most tasks",
    },
    {
      value: "flash-lite",
      label: "Flash-Lite",
      detail: "Fastest Gemini option for simple tasks",
    },
  ],
  antigravity: [
    {
      value: null,
      label: "Default",
      detail: "Antigravity's default model for the current session",
    },
  ],
  kiro: [
    {
      value: null,
      label: "Default",
      detail: "Kiro CLI's bundled default model",
    },
    {
      value: "auto",
      label: "Auto",
      detail: "Let Kiro route each task to an available model",
    },
    {
      value: "claude-sonnet-4.5",
      label: "Sonnet 4.5",
      detail: "Anthropic Sonnet 4.5 via Kiro",
    },
    {
      value: "claude-haiku-4.5",
      label: "Haiku 4.5",
      detail: "Fast, lower-credit model via Kiro",
    },
  ],
  copilot: [
    {
      value: null,
      label: "Default",
      detail: "GitHub Copilot's bundled default model",
    },
    {
      value: "claude-sonnet-4.6",
      label: "Claude Sonnet 4.6",
      detail: "GitHub Copilot CLI's documented general-purpose default",
    },
    {
      value: "gpt-5.4",
      label: "gpt-5.4",
      detail: "Complex reasoning via Copilot",
    },
    {
      value: "claude-haiku-4.5",
      label: "Claude Haiku 4.5",
      detail: "Fast, lightweight operations via Copilot",
    },
    {
      value: "gpt-5.3-codex",
      label: "gpt-5.3-codex",
      detail: "Code-focused tasks via Copilot",
    },
    {
      value: "gemini-3.1-pro-preview",
      label: "Gemini 3.1 Pro Preview",
      detail: "Google reasoning model via Copilot",
    },
    {
      value: "gemini-3.5-flash",
      label: "Gemini 3.5 Flash",
      detail: "Fast Google model via Copilot",
    },
    {
      value: "mai-code-1-flash",
      label: "MAI-Code-1 Flash",
      detail: "Fast, adaptive coding via Copilot",
    },
  ],
};

const WORKER_VALUES = new Set<string>(WORKER_OPTIONS);

export function normalizeWorkerVendor(value: unknown): WorkerVendor | null {
  return typeof value === "string" && WORKER_VALUES.has(value)
    ? (value as WorkerVendor)
    : null;
}

export function fillWorkerModels(
  seed: readonly WorkerVendor[],
): WorkerVendor[] {
  const source = seed.length > 0 ? seed : DEFAULT_WORKER_MODELS;
  return Array.from({ length: 5 }, (_, i) => source[i % source.length]);
}

export function cliModelOptionFor(
  vendor: WorkerVendor,
  value: WorkerCliModelValue,
): WorkerCliModelOption {
  return (
    CLI_MODEL_OPTIONS[vendor].find((option) => option.value === value) ??
    CLI_MODEL_OPTIONS[vendor][0]
  );
}

export function normalizeWorkerCliModel(
  vendor: WorkerVendor,
  value: unknown,
): WorkerCliModelValue {
  const normalized =
    typeof value === "string" && value.trim().length > 0
      ? value.trim()
      : null;
  return cliModelOptionFor(vendor, normalized).value;
}

export function fillWorkerCliModels(
  seed: readonly unknown[],
  workerModels: readonly WorkerVendor[],
): WorkerCliModelValue[] {
  return Array.from({ length: 5 }, (_, i) => {
    const vendor =
      workerModels[i] ??
      DEFAULT_WORKER_MODELS[i % DEFAULT_WORKER_MODELS.length];
    return normalizeWorkerCliModel(vendor, seed[i]);
  });
}

export function workerCountNumber(choice: WorkerCountChoice): number {
  return choice === "auto" ? 0 : Number.parseInt(choice, 10);
}

export function encodeWorkerModelRoster(
  workerCount: WorkerCountChoice,
  workerModels: readonly WorkerVendor[],
  workerCliModels: readonly WorkerCliModelValue[],
): string | null {
  const count = workerCountNumber(workerCount);
  if (count <= 0) return null;
  const vendors = fillWorkerModels(workerModels);
  const cliModels = fillWorkerCliModels(workerCliModels, vendors);
  return Array.from({ length: count }, (_, i) => {
    const vendor = vendors[i];
    const cliModel = cliModels[i];
    return cliModel ? `${vendor}:${cliModel}` : vendor;
  }).join(",");
}
