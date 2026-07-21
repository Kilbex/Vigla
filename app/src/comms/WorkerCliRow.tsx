import type { CliAuthState, CliAuthStatusDto } from "../bindings";
import {
  CLI_MODEL_OPTIONS,
  WORKER_LABEL,
  WORKER_OPTIONS,
  cliModelOptionFor,
  type WorkerCliModelValue,
  type WorkerVendor,
} from "./deploy-models";

interface WorkerCliRowProps {
  index: number;
  model: WorkerVendor;
  cliModel: WorkerCliModelValue;
  auth: CliAuthStatusDto | null;
  disabled: boolean;
  loginBusy: boolean;
  onModelChange: (model: WorkerVendor) => void;
  onCliModelChange: (model: WorkerCliModelValue) => void;
  onLogin: () => void;
}

export default function WorkerCliRow({
  index,
  model,
  cliModel,
  auth,
  disabled,
  loginBusy,
  onModelChange,
  onCliModelChange,
  onLogin,
}: WorkerCliRowProps) {
  const state = auth?.state ?? "unknown";
  const canLogin = auth?.binary_present === true && state !== "ready";
  const selectedCliModel = cliModelOptionFor(model, cliModel);
  return (
    <div className="deploy-worker-cli-row">
      <div className="deploy-worker-cli-main">
        <span className="deploy-worker-cli-name">Employee {index + 1}</span>
        <select
          id={`deploy-worker-${index}-cli`}
          name={`worker-${index}-cli`}
          className="deploy-select deploy-worker-cli-select"
          value={model}
          onChange={(event) =>
            onModelChange(event.target.value as WorkerVendor)
          }
          disabled={disabled}
          aria-label={`employee ${index + 1} CLI`}
        >
          {WORKER_OPTIONS.map((vendor) => (
            <option key={vendor} value={vendor}>
              {WORKER_LABEL[vendor]}
            </option>
          ))}
        </select>
        <select
          id={`deploy-worker-${index}-model`}
          name={`worker-${index}-model`}
          className="deploy-select deploy-worker-model-select"
          value={cliModel ?? ""}
          onChange={(event) => onCliModelChange(event.target.value || null)}
          disabled={disabled}
          aria-label={`employee ${index + 1} applied model`}
        >
          {CLI_MODEL_OPTIONS[model].map((option) => (
            <option key={option.value ?? "default"} value={option.value ?? ""}>
              {option.label}
            </option>
          ))}
        </select>
      </div>
      <div className="deploy-worker-cli-status">
        <span
          className={[
            "deploy-auth-state",
            state === "ready" ? "deploy-auth-state--ready" : "",
            state === "not_logged_in" ? "deploy-auth-state--warn" : "",
            state === "missing_cli" ? "deploy-auth-state--muted" : "",
          ]
            .filter(Boolean)
            .join(" ")}
        >
          {authStateLabel(state)}
        </span>
        <span className="deploy-auth-detail">
          {auth?.detail ?? "Status has not been checked yet."}
        </span>
        <span className="deploy-model-current">
          Model: {selectedCliModel.label}
        </span>
        <span className="deploy-model-detail">{selectedCliModel.detail}</span>
        {canLogin ? (
          <button
            type="button"
            className="deploy-inline-btn"
            onClick={onLogin}
            disabled={disabled || loginBusy}
            aria-label={`log in to ${WORKER_LABEL[model]}`}
          >
            {loginBusy ? "Opening…" : "Login"}
          </button>
        ) : null}
      </div>
    </div>
  );
}

function authStateLabel(state: CliAuthState): string {
  switch (state) {
    case "ready":
      return "Logged in";
    case "missing_cli":
      return "CLI missing";
    case "not_logged_in":
      return "Login needed";
    case "unknown":
      return "Unknown";
  }
}
