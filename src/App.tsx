import { useEffect, useState, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { PhysicalPosition } from "@tauri-apps/api/dpi";
import "./App.css";
import { AudioVisualizer } from "./components/AudioVisualizer";

interface NodePayload {
  nodeId: string;
  title: string;
  script: string;
  keyPoints?: string[];
  warnings?: string[];
  responses: Array<{ label: string; nextNode: string }>;
}

interface AISuggestion {
  nodeId: string;
  title: string;
  confidence: "high" | "medium";
  reasoning: string;
  phraseHash: string;
}

interface TranscriptLine {
  text: string;
  timestamp: string;
  speaker: number;
}

type ViewMode = "script" | "transcript";

const OPACITY_LEVELS = [
  { label: "S", title: "Solid (96%)", value: 0.96 },
  { label: "M", title: "Semi (75%)", value: 0.75 },
  { label: "G", title: "Ghost (50%)", value: 0.50 },
] as const;

function getInitialOpacity(): number {
  try {
    return parseFloat(localStorage.getItem("companion_opacity") ?? "0.96");
  } catch {
    return 0.96;
  }
}

function App() {
  const [isActive, setIsActive] = useState(false);
  const [levels, setLevels] = useState({ mic: 0, sys: 0 });
  const [updateVersion, setUpdateVersion] = useState<string | null>(null);
  const [updating, setUpdating] = useState(false);
  const [view, setView] = useState<ViewMode>("script");
  const [currentNode, setCurrentNode] = useState<NodePayload | null>(null);
  const [aiSuggestion, setAiSuggestion] = useState<AISuggestion | null>(null);
  const [transcriptLines, setTranscriptLines] = useState<TranscriptLine[]>([]);
  const [opacity, setOpacity] = useState<number>(getInitialOpacity);
  const [showKeyPoints, setShowKeyPoints] = useState(false);
  const [showWarnings, setShowWarnings] = useState(false);

  const transcriptEndRef = useRef<HTMLDivElement>(null);
  const appWindow = getCurrentWebviewWindow();

  // Apply opacity CSS variable
  useEffect(() => {
    document.documentElement.style.setProperty("--bg-opacity", String(opacity));
    try { localStorage.setItem("companion_opacity", String(opacity)); } catch {}
  }, [opacity]);

  // Position persistence
  useEffect(() => {
    // Restore saved position on startup
    try {
      const savedPos = localStorage.getItem("companion_pos");
      if (savedPos) {
        const { x, y } = JSON.parse(savedPos);
        void appWindow.setPosition(new PhysicalPosition(x, y));
      }
    } catch {}

    // Save position whenever the window moves
    const unlistenMove = appWindow.listen("tauri://move", async () => {
      try {
        const pos = await appWindow.outerPosition();
        localStorage.setItem("companion_pos", JSON.stringify({ x: pos.x, y: pos.y }));
      } catch {}
    });

    return () => { unlistenMove.then(u => u()); };
  }, []);

  // Auto-scroll transcript
  useEffect(() => {
    if (view === "transcript") {
      transcriptEndRef.current?.scrollIntoView({ behavior: "smooth" });
    }
  }, [transcriptLines, view]);

  // Reset collapsibles when node changes
  useEffect(() => {
    setShowKeyPoints(false);
    setShowWarnings(false);
  }, [currentNode?.nodeId]);

  // All event listeners + companion startup
  useEffect(() => {
    const startCompanion = async () => {
      try {
        await invoke("start_companion");
        setIsActive(true);
      } catch {
        setIsActive(false);
      }
    };
    startCompanion();

    const unlistenAudio = listen<{ mic: number; sys: number }>("audio-levels", (e) => {
      setLevels(e.payload);
    });

    const unlistenUpdate = listen<string>("update-available", (e) => {
      setUpdateVersion(e.payload);
    });

    const unlistenNode = listen<NodePayload>("node_update", (e) => {
      setCurrentNode(e.payload);
    });

    const unlistenAI = listen<AISuggestion>("ai_suggestion", (e) => {
      setAiSuggestion(e.payload);
    });

    const unlistenClear = listen<void>("clear_suggestion", () => {
      setAiSuggestion(null);
    });

    const unlistenTranscript = listen<TranscriptLine>("transcript", (e) => {
      setTranscriptLines(prev => [...prev.slice(-9), e.payload]);
    });

    return () => {
      unlistenAudio.then(u => u());
      unlistenUpdate.then(u => u());
      unlistenNode.then(u => u());
      unlistenAI.then(u => u());
      unlistenClear.then(u => u());
      unlistenTranscript.then(u => u());
    };
  }, []);

  const handleClose = async () => {
    await appWindow.close();
  };

  const handleUpdate = async () => {
    setUpdating(true);
    await invoke("install_update").catch(() => setUpdating(false));
  };

  const handleNavigate = (nodeId: string) => {
    void invoke("send_to_browser", {
      payload: JSON.stringify({ type: "navigate", nodeId }),
    });
  };

  const handleAIFeedback = (action: "accept" | "reject") => {
    if (!aiSuggestion) return;
    void invoke("send_to_browser", {
      payload: JSON.stringify({ type: "ai_feedback", action, phraseHash: aiSuggestion.phraseHash }),
    });
    if (action === "accept") {
      handleNavigate(aiSuggestion.nodeId);
    }
    setAiSuggestion(null);
  };

  const cycleOpacity = () => {
    const idx = OPACITY_LEVELS.findIndex(l => Math.abs(l.value - opacity) < 0.05);
    const next = OPACITY_LEVELS[(idx + 1) % OPACITY_LEVELS.length];
    setOpacity(next.value);
  };

  const opacityBtn = OPACITY_LEVELS.find(l => Math.abs(l.value - opacity) < 0.05) ?? OPACITY_LEVELS[0];

  return (
    <div className="overlay-root">
      {/* Header — drag region */}
      <header className="overlay-header" data-tauri-drag-region>
        <div className="status-pill" data-tauri-drag-region>
          <div className={`status-dot ${isActive ? "active" : ""}`} />
          <span className="brand" data-tauri-drag-region>BrainSales</span>
        </div>
        <div className="header-controls">
          <div className="view-switcher">
            <button
              className={`view-btn ${view === "script" ? "active" : ""}`}
              onClick={() => setView("script")}
            >Script</button>
            <button
              className={`view-btn ${view === "transcript" ? "active" : ""}`}
              onClick={() => setView("transcript")}
            >Transcript</button>
          </div>
          <button
            className="opacity-btn"
            onClick={cycleOpacity}
            title={opacityBtn.title}
          >{opacityBtn.label}</button>
          <button className="close-btn" onClick={handleClose} title="Minimize to tray">✕</button>
        </div>
      </header>

      {/* AI Suggestion Chip */}
      {aiSuggestion && (
        <div className="ai-chip">
          <div className="ai-chip-top">
            <span className="ai-chip-icon">✦</span>
            <span className="ai-chip-node">{aiSuggestion.title}</span>
            <span className={`ai-chip-conf ${aiSuggestion.confidence}`}>
              {aiSuggestion.confidence}
            </span>
          </div>
          {aiSuggestion.reasoning && (
            <div className="ai-chip-reasoning">{aiSuggestion.reasoning}</div>
          )}
          <div className="ai-chip-btns">
            <button className="ai-accept-btn" onClick={() => handleAIFeedback("accept")}>
              ✓ Navigate
            </button>
            <button className="ai-reject-btn" onClick={() => handleAIFeedback("reject")}>
              ✗ No
            </button>
          </div>
        </div>
      )}

      {/* Main content area */}
      <div className="overlay-body">
        {view === "script" ? (
          currentNode ? (
            <div className="script-view">
              <div className="node-title">{currentNode.title}</div>
              <div className="node-script">{currentNode.script}</div>

              {(currentNode.keyPoints ?? []).length > 0 && (
                <div className="section">
                  <button className="section-header" onClick={() => setShowKeyPoints(v => !v)}>
                    <span>Key Points</span>
                    <span className="chevron">{showKeyPoints ? "▲" : "▼"}</span>
                  </button>
                  {showKeyPoints && (
                    <ul className="section-list">
                      {currentNode.keyPoints!.map((kp, i) => <li key={i}>{kp}</li>)}
                    </ul>
                  )}
                </div>
              )}

              {(currentNode.warnings ?? []).length > 0 && (
                <div className="section warning-section">
                  <button className="section-header" onClick={() => setShowWarnings(v => !v)}>
                    <span>⚠ Warnings</span>
                    <span className="chevron">{showWarnings ? "▲" : "▼"}</span>
                  </button>
                  {showWarnings && (
                    <ul className="section-list">
                      {currentNode.warnings!.map((w, i) => <li key={i}>{w}</li>)}
                    </ul>
                  )}
                </div>
              )}

              {currentNode.responses.length > 0 && (
                <div className="responses">
                  {currentNode.responses.map((r, i) => (
                    <button
                      key={i}
                      className="response-btn"
                      onClick={() => handleNavigate(r.nextNode)}
                    >
                      {r.label}
                    </button>
                  ))}
                </div>
              )}
            </div>
          ) : (
            <div className="empty-state">
              <span className="empty-icon">📋</span>
              <span>Waiting for call to start…</span>
            </div>
          )
        ) : (
          <div className="transcript-view">
            {transcriptLines.length === 0 ? (
              <div className="empty-state">
                <span className="empty-icon">🎙</span>
                <span>No transcript yet</span>
              </div>
            ) : (
              transcriptLines.map((line, i) => (
                <div key={i} className={`t-line ${line.speaker === 0 ? "rep" : "prospect"}`}>
                  <span className="t-speaker">{line.speaker === 0 ? "Rep" : "Prospect"}</span>
                  <span className="t-text">{line.text}</span>
                </div>
              ))
            )}
            <div ref={transcriptEndRef} />
          </div>
        )}
      </div>

      {/* Footer */}
      <footer className="overlay-footer">
        {updateVersion ? (
          <div className="update-banner">
            <span className="update-text">v{updateVersion} available</span>
            <button className="update-btn" onClick={handleUpdate} disabled={updating}>
              {updating ? "Installing…" : "Update Now"}
            </button>
          </div>
        ) : (
          <div className="footer-vis">
            <AudioVisualizer micLevel={levels.mic} sysLevel={levels.sys} />
          </div>
        )}
      </footer>
    </div>
  );
}

export default App;
