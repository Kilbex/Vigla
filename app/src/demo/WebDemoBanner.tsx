import { useEffect, useRef, useState } from "react";
import { useOpsStore } from "../store";
import {
  DEMO_RECORDINGS,
  findDemoRecording,
  type DemoRecordingId,
} from "./recordings";
import "./web-demo.css";

function loadRecording(id: DemoRecordingId): void {
  const recording = findDemoRecording(id);
  let store = useOpsStore.getState();
  if (store.replay.mode === "replay") {
    store.exitReplay();
    store = useOpsStore.getState();
  }

  store.enterReplay([recording.session]);
  store = useOpsStore.getState();
  store.beginReplay(recording.session.id);
  store = useOpsStore.getState();
  store.appendReplayPage(recording.session.id, recording.events);
  store = useOpsStore.getState();
  store.finishReplay();
  store.setReplayPosition(0);
  store.setReplaySpeed(4);
  store.setReplayPlaying(true);
}

export default function WebDemoBanner() {
  const [selected, setSelected] = useState<DemoRecordingId>("happy");
  const loaded = useRef(false);
  const recording = findDemoRecording(selected);

  useEffect(() => {
    if (loaded.current) return;
    loaded.current = true;
    loadRecording("happy");
  }, []);

  const choose = (id: DemoRecordingId) => {
    setSelected(id);
    loadRecording(id);
  };

  return (
    <section className="web-demo-bar" aria-label="Recorded replay demo">
      <div className="web-demo-copy">
        <strong>Recorded replay</strong>
        <span>Read-only events. Build locally for real vendor runs.</span>
      </div>
      <div className="web-demo-scenarios" aria-label="Choose a recorded mission">
        {DEMO_RECORDINGS.map((candidate) => (
          <button
            key={candidate.id}
            type="button"
            className={
              "web-demo-scenario" +
              (candidate.id === selected ? " web-demo-scenario--active" : "")
            }
            aria-pressed={candidate.id === selected}
            onClick={() => choose(candidate.id)}
          >
            {candidate.label}
          </button>
        ))}
      </div>
      <p className="web-demo-outcome" aria-live="polite">
        <span>{recording.outcome}</span>
        <small>{recording.description}</small>
      </p>
      <a
        className="web-demo-source"
        href="https://github.com/Kilbex/Vigla#build-a-local-dmg"
      >
        Build from source
        <span aria-hidden>↗</span>
      </a>
    </section>
  );
}
