import { useEffect, useState } from "react";
import { commands } from "../bindings";

interface DiffViewerProps {
  workerId: string;
  /** Changes only when file/terminal events make the cached diff stale. */
  revision?: number;
}

export default function DiffViewer({ workerId, revision = 0 }: DiffViewerProps) {
  const [diff, setDiff] = useState<string>("");
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);

    (async () => {
      try {
        const result = await commands.getWorkerDiff(workerId);
        if (!cancelled) {
          if (result.status === "ok") {
            setDiff(result.data);
          } else {
            setError(result.error);
          }
          setLoading(false);
        }
      } catch (err) {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : "Failed to load diff");
          setLoading(false);
        }
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [workerId, revision]);

  if (loading) {
    return <div className="diff-viewer diff-viewer--loading">Loading diff…</div>;
  }

  if (error) {
    return <div className="diff-viewer diff-viewer--error">Error: {error}</div>;
  }

  if (!diff) {
    return (
      <div className="diff-viewer diff-viewer--empty">
        No diff available for this worker.
      </div>
    );
  }

  // Render unified diff format
  return (
    <pre className="diff-viewer diff-viewer--content">
      <code>{diff}</code>
    </pre>
  );
}
