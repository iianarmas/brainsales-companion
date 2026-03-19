import { useEffect, useState, useRef, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { PhysicalPosition, PhysicalSize } from "@tauri-apps/api/dpi";
import "./App.css";
import { AudioVisualizer } from "./components/AudioVisualizer";

interface CallSummary {
  summary: string;
  keyPoints: string[];
  prospectSentiment: string;
  followUpItems: string[];
  objectionsRaised: string[];
}

interface CoachingCue {
  type: 'talk_too_long' | 'speaking_too_fast' | string;
  message: string;
}

interface NodePayload {
  nodeId: string;
  title: string;
  script: string;
  keyPoints?: string[];
  warnings?: string[];
  responses: Array<{ label: string; nextNode: string }>;
  gatekeeperNodeId?: string | null;
  voicemailNodeId?: string | null;
}

type RecordingState = 'idle' | 'recording' | 'paused';

interface AISuggestion {
  nodeId: string;
  title: string;
  confidence: "high" | "medium";
  reasoning: string;
  phraseHash: string;
  autoNavigated?: boolean;
}

interface TranscriptLine {
  text: string;
  timestamp: string;
  speaker: number;
}

interface OpeningScript {
  id: string;
  title: string;
}

interface ScriptNode {
  id: string;
  title: string;
  script: string;
}

type ViewMode = "script" | "transcript" | "actions";
type Theme = "dark" | "darker" | "slate" | "purple" | "rose" | "teal";

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

function getInitialTheme(): Theme {
  try {
    return (localStorage.getItem("companion_theme") as Theme) ?? "dark";
  } catch {
    return "dark";
  }
}

function getInitialFontScale(): number {
  try {
    return parseFloat(localStorage.getItem("companion_font_scale") ?? "1.0");
  } catch {
    return 1.0;
  }
}

const OUTCOME_LABELS: Record<string, string> = {
  meeting_set: "Meeting Set",
  follow_up: "Follow Up",
  send_info: "Send Info",
  not_interested: "Not Interested",
  wrong_person: "Wrong Person",
  no_answer: "No Answer",
  left_voicemail: "Left Voicemail",
  disconnected: "Disconnected",
  redirected: "Redirected",
};

function App() {
  const [, setIsActive] = useState(false);
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
  const [transcriptionState, setTranscriptionState] = useState<RecordingState>('idle');
  const [isPracticeMode, setIsPracticeMode] = useState(false);
  const [outcome, setOutcome] = useState<string | null>(null);
  const [outcomeSource, setOutcomeSource] = useState<string | null>(null);
  const [deepgramState, setDeepgramState] = useState<'idle' | 'streaming'>('idle');
  const [audioDevices, setAudioDevices] = useState<{ inputs: string[]; outputs: string[] }>({ inputs: [], outputs: [] });
  const [selectedInput, setSelectedInput] = useState<string | null>(localStorage.getItem('preferred_input'));
  const [selectedOutput, setSelectedOutput] = useState<string | null>(localStorage.getItem('preferred_output'));
  const [micGain, setMicGain] = useState<number>(() => {
    const saved = localStorage.getItem('mic_gain');
    return saved ? parseFloat(saved) : 1.0;
  });

  // Opening script selector state
  const [openingScripts, setOpeningScripts] = useState<OpeningScript[]>([]);
  const [activeFlowId, setActiveFlowId] = useState<string | null>(null);

  // AI correction mode
  const [showCorrectionPicker, setShowCorrectionPicker] = useState(false);
  const [correctionSearch, setCorrectionSearch] = useState("");
  const [allNodes, setAllNodes] = useState<ScriptNode[]>([]);

  // Theme, font scale, and custom primary color
  const [theme, setTheme] = useState<Theme>(getInitialTheme);
  const [fontScale, setFontScale] = useState<number>(getInitialFontScale);
  const [customPrimary, setCustomPrimary] = useState<string>(() => {
    try { return localStorage.getItem("companion_custom_primary") ?? ""; } catch { return ""; }
  });

  // Co-Pilot active state (synced from web app)
  const [isCompanionActive, setIsCompanionActive] = useState(false);

  // Web app connection state (Phase 4)
  const [isWebAppConnected, setIsWebAppConnected] = useState(false);
  const [authStatus, setAuthStatus] = useState<'pending' | 'authenticated' | 'failed'>('pending');

  // Talk-to-listen ratio (Phase 5B)
  const [talkRatio, setTalkRatio] = useState(0.5);
  const repSpeakTimeRef = useRef(0);
  const totalSpeakTimeRef = useRef(0);
  const lastAudioTimestampRef = useRef(Date.now());

  // Coaching cue toast (Phase 5D)
  const [coachingCue, setCoachingCue] = useState<CoachingCue | null>(null);
  const coachingTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Call summary (Phase 5C)
  const [callSummary, setCallSummary] = useState<CallSummary | null>(null);
  const [showSummary, setShowSummary] = useState(false);

  // In-overlay "logged" toast
  const [loggedLabel, setLoggedLabel] = useState<string | null>(null);
  const loggedTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const transcriptEndRef = useRef<HTMLDivElement>(null);
  const appWindow = getCurrentWebviewWindow();

  // Apply opacity CSS variable
  useEffect(() => {
    document.documentElement.style.setProperty("--bg-opacity", String(opacity));
    try { localStorage.setItem("companion_opacity", String(opacity)); } catch {}
  }, [opacity]);

  // Apply theme class and optional custom primary color
  useEffect(() => {
    document.documentElement.setAttribute("data-theme", theme);
    try { localStorage.setItem("companion_theme", theme); } catch {}
    if (customPrimary) {
      document.documentElement.style.setProperty("--primary", customPrimary);
    } else {
      document.documentElement.style.removeProperty("--primary");
    }
    try { localStorage.setItem("companion_custom_primary", customPrimary); } catch {}
  }, [theme, customPrimary]);

  // Apply font scale via CSS zoom (scales all content uniformly)
  useEffect(() => {
    (document.documentElement.style as any).zoom = String(fontScale);
    try { localStorage.setItem("companion_font_scale", String(fontScale)); } catch {}
  }, [fontScale]);

  // Position + size persistence
  useEffect(() => {
    try {
      const savedPos = localStorage.getItem("companion_pos");
      if (savedPos) {
        const { x, y } = JSON.parse(savedPos);
        void appWindow.setPosition(new PhysicalPosition(x, y));
      }
      const savedSize = localStorage.getItem("companion_size");
      if (savedSize) {
        const { w, h } = JSON.parse(savedSize);
        void appWindow.setSize(new PhysicalSize(w, h));
      }
    } catch {}

    const unlistenMove = appWindow.listen("tauri://move", async () => {
      try {
        const pos = await appWindow.outerPosition();
        localStorage.setItem("companion_pos", JSON.stringify({ x: pos.x, y: pos.y }));
      } catch {}
    });

    const unlistenResize = appWindow.listen("tauri://resize", async () => {
      try {
        const size = await appWindow.outerSize();
        localStorage.setItem("companion_size", JSON.stringify({ w: size.width, h: size.height }));
      } catch {}
    });

    return () => {
      unlistenMove.then(u => u());
      unlistenResize.then(u => u());
    };
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

  // Reset deepgramState when transcription stops
  useEffect(() => {
    if (transcriptionState !== 'recording') setDeepgramState('idle');
  }, [transcriptionState]);

  // All event listeners + companion startup
  useEffect(() => {
    const startCompanion = async () => {
      try {
        const savedGain = localStorage.getItem('mic_gain');
        const gain = savedGain ? parseFloat(savedGain) : 1.0;
        await invoke("start_companion", { gain });
        setIsActive(true);
      } catch {
        setIsActive(false);
      }
    };
    startCompanion();

    const unlistenAudio = listen<{ mic: number; sys: number }>("audio-levels", (e) => {
      setLevels(e.payload);
      // Track talk-to-listen ratio
      const now = Date.now();
      const delta = Math.min(now - lastAudioTimestampRef.current, 500); // cap at 500ms
      lastAudioTimestampRef.current = now;
      const micActive = e.payload.mic > 0.02;
      const sysActive = e.payload.sys > 0.02;
      if (micActive || sysActive) {
        totalSpeakTimeRef.current += delta;
        if (micActive) repSpeakTimeRef.current += delta;
        const total = totalSpeakTimeRef.current;
        if (total > 0) {
          setTalkRatio(repSpeakTimeRef.current / total);
        }
      }
    });

    const unlistenUpdate = listen<string>("update-available", (e) => {
      setUpdateVersion(e.payload);
    });

    const unlistenNode = listen<NodePayload>("node_update", (e) => {
      setCurrentNode(e.payload);
    });

    const unlistenAI = listen<AISuggestion>("ai_suggestion", (e) => {
      console.log("Companion: AI Suggestion received:", e.payload);
      setAiSuggestion(e.payload);
      setShowCorrectionPicker(false);
    });

    const unlistenClear = listen<void>("clear_suggestion", () => {
      console.log("Companion: Clear Suggestion received");
      setAiSuggestion(null);
      setShowCorrectionPicker(false);
    });

    const unlistenTranscript = listen<TranscriptLine>("transcript", (e) => {
      setTranscriptLines(prev => [...prev.slice(-9), e.payload]);
    });

    const unlistenRecording = listen<{ state: RecordingState }>('recording_state', (e) => {
      console.log("Companion: Recording State sync:", e.payload.state);
      setTranscriptionState(e.payload.state);
      if (e.payload.state === 'idle') {
        setTranscriptLines([]);
        // Reset talk ratio tracking
        repSpeakTimeRef.current = 0;
        totalSpeakTimeRef.current = 0;
        setTalkRatio(0.5);
        // Clear coaching cue and summary
        setCoachingCue(null);
      }
    });
    const unlistenPractice = listen<{ enabled: boolean }>('practice_mode_state', (e) => {
      setIsPracticeMode(e.payload.enabled);
    });
    const unlistenOutcome = listen<{ outcome: string | null; outcomeSource?: string | null }>('outcome_state', (e) => {
      setOutcome(e.payload.outcome);
      setOutcomeSource(e.payload.outcomeSource ?? null);
    });

    const unlistenDG = listen<{ state: 'idle' | 'streaming' }>('deepgram_state', (e) => {
      setDeepgramState(e.payload.state);
    });

    const unlistenOpeningScripts = listen<{ scripts: OpeningScript[]; activeFlowId: string | null }>('opening_scripts', (e) => {
      setOpeningScripts(e.payload.scripts);
      setActiveFlowId(e.payload.activeFlowId);
    });

    const unlistenActiveFlow = listen<{ flowId: string | null }>('active_flow_changed', (e) => {
      setActiveFlowId(e.payload.flowId);
    });

    const unlistenCompanion = listen<{ active: boolean }>('companion_state', (e) => {
      setIsCompanionActive(e.payload.active);
    });

    const unlistenScriptsList = listen<{ nodes: ScriptNode[] }>('scripts_list', (e) => {
      setAllNodes(e.payload.nodes);
    });

    const unlistenCallLogged = listen<{ outcome: string | null }>('call_logged', (e) => {
      const o = e.payload.outcome;
      if (loggedTimerRef.current) clearTimeout(loggedTimerRef.current);
      setLoggedLabel(o ? (OUTCOME_LABELS[o] ?? o) : "Call logged");
      loggedTimerRef.current = setTimeout(() => setLoggedLabel(null), 2500);
    });

    // Phase 4: Browser connection state
    const unlistenBrowserConnected = listen<void>('browser_connected', () => {
      setIsWebAppConnected(true);
    });
    const unlistenBrowserDisconnected = listen<void>('browser_disconnected', () => {
      setIsWebAppConnected(false);
    });

    // Phase 3: Auth events
    const unlistenAuthReceived = listen<void>('auth_received', () => {
      setAuthStatus('authenticated');
    });
    const unlistenAuthFailed = listen<string>('auth_failed', () => {
      setAuthStatus('failed');
    });

    // Phase 5D: Coaching cues from web app
    const unlistenCoaching = listen<CoachingCue>('coaching_cue', (e) => {
      setCoachingCue(e.payload);
      if (coachingTimerRef.current) clearTimeout(coachingTimerRef.current);
      coachingTimerRef.current = setTimeout(() => setCoachingCue(null), 8000);
    });

    // Phase 5C: Call summary from web app
    const unlistenCallSummary = listen<CallSummary>('call_summary', (e) => {
      setCallSummary(e.payload);
      setShowSummary(true);
    });

    // Phase 5A: Global shortcut handler
    const unlistenShortcut = listen<string>('global_shortcut', (e) => {
      const key = e.payload.toLowerCase();
      if (key.includes('ctrl+shift+b')) {
        // Toggle visibility
        void appWindow.isVisible().then(visible => {
          if (visible) void appWindow.hide();
          else { void appWindow.show(); void appWindow.setFocus(); }
        });
      } else if (key.includes('ctrl+shift+p')) {
        // Toggle pause/resume
        if (transcriptionState === 'recording') handleControl('pause');
        else if (transcriptionState === 'paused') handleControl('start');
      } else if (key.includes('ctrl+shift+n')) {
        // New contact / reset AI context
        handleResetAIContext();
      } else if (key.includes('ctrl+shift+o')) {
        // Quick log outcome — cycle to actions tab
        setView('actions');
      }
    });

    return () => {
      unlistenAudio.then(u => u());
      unlistenUpdate.then(u => u());
      unlistenNode.then(u => u());
      unlistenAI.then(u => u());
      unlistenClear.then(u => u());
      unlistenTranscript.then(u => u());
      unlistenRecording.then(u => u());
      unlistenPractice.then(u => u());
      unlistenOutcome.then(u => u());
      unlistenDG.then(u => u());
      unlistenOpeningScripts.then(u => u());
      unlistenActiveFlow.then(u => u());
      unlistenCompanion.then(u => u());
      unlistenScriptsList.then(u => u());
      unlistenCallLogged.then(u => u());
      unlistenBrowserConnected.then(u => u());
      unlistenBrowserDisconnected.then(u => u());
      unlistenAuthReceived.then(u => u());
      unlistenAuthFailed.then(u => u());
      unlistenCoaching.then(u => u());
      unlistenCallSummary.then(u => u());
      unlistenShortcut.then(u => u());
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
      if (!aiSuggestion.autoNavigated) {
        handleNavigate(aiSuggestion.nodeId);
      }
      setShowCorrectionPicker(false);
      setCorrectionSearch("");
      setAiSuggestion(null);
    } else {
      setShowCorrectionPicker(true);
      setCorrectionSearch("");
    }
  };

  const handleAICorrection = (nodeId: string) => {
    if (!aiSuggestion) return;
    void invoke("send_to_browser", {
      payload: JSON.stringify({
        type: "ai_feedback",
        action: "reject",
        phraseHash: aiSuggestion.phraseHash,
        correctedNodeId: nodeId,
      }),
    });
    handleNavigate(nodeId);
    setAiSuggestion(null);
    setShowCorrectionPicker(false);
    setCorrectionSearch("");
  };

  const handleDismissSuggestion = () => {
    if (!aiSuggestion) return;
    void invoke("send_to_browser", {
      payload: JSON.stringify({ type: "ai_feedback", action: "reject", phraseHash: aiSuggestion.phraseHash }),
    });
    setAiSuggestion(null);
    setShowCorrectionPicker(false);
    setCorrectionSearch("");
  };

  const handleControl = (cmd: 'start' | 'pause' | 'stop') => {
    if (cmd === 'start') setTranscriptionState('recording');
    else if (cmd === 'pause') setTranscriptionState('paused');
    else if (cmd === 'stop') {
      setTranscriptionState('idle');
      setOutcome(null);
      setOutcomeSource(null);
      setTranscriptLines([]);
    }
    void invoke('send_to_browser', { payload: JSON.stringify({ type: 'control', command: cmd }) });
  };

  // Log outcome: tells web app to set outcome + stop + persist session + reset
  const handleLogOutcome = (o: string) => {
    setOutcome(o);
    void invoke('send_to_browser', { payload: JSON.stringify({ type: 'log_outcome', outcome: o }) });
    // Show brief "✓ Logged" feedback in overlay
    if (loggedTimerRef.current) clearTimeout(loggedTimerRef.current);
    setLoggedLabel(OUTCOME_LABELS[o] ?? o);
    loggedTimerRef.current = setTimeout(() => setLoggedLabel(null), 2500);
  };

  const handleNextCall = () => {
    void invoke('send_to_browser', { payload: JSON.stringify({ type: 'next_call' }) });
  };

  const handleTogglePracticeMode = () => {
    void invoke('send_to_browser', {
      payload: JSON.stringify({ type: 'set_practice_mode', enabled: !isPracticeMode }),
    });
  };

  const handleResetAIContext = () => {
    void invoke('send_to_browser', { payload: JSON.stringify({ type: 'reset_ai_context' }) });
  };

  const handleChangeFlow = (flowId: string) => {
    setActiveFlowId(flowId);
    setTranscriptLines([]);
    void invoke('send_to_browser', {
      payload: JSON.stringify({ type: 'set_active_flow', flowId }),
    });
  };

  const handleToggleCopilot = () => {
    void invoke('send_to_browser', { payload: JSON.stringify({ type: 'toggle_companion' }) });
  };

  const fetchDevices = async () => {
    try {
      const result = await invoke<{ inputs: string[]; outputs: string[] }>('list_audio_devices');
      setAudioDevices(result);
    } catch (e) {
      console.error('Failed to fetch audio devices:', e);
    }
  };

  const handleSetDevices = async (input: string | null, output: string | null) => {
    if (input !== undefined) {
      setSelectedInput(input);
      if (input) localStorage.setItem('preferred_input', input);
      else localStorage.removeItem('preferred_input');
    }
    if (output !== undefined) {
      setSelectedOutput(output);
      if (output) localStorage.setItem('preferred_output', output);
      else localStorage.removeItem('preferred_output');
    }
    try {
      await invoke('set_audio_devices', {
        input: input ?? selectedInput,
        output: output ?? selectedOutput,
      });
    } catch (e) {
      console.error('Failed to set audio devices:', e);
    }
  };

  const handleSetMicGain = async (gain: number) => {
    setMicGain(gain);
    localStorage.setItem('mic_gain', String(gain));
    try {
      await invoke('set_mic_gain', { gain });
    } catch (e) {
      console.error('Failed to set mic gain:', e);
    }
  };

  const cycleOpacity = () => {
    const idx = OPACITY_LEVELS.findIndex(l => Math.abs(l.value - opacity) < 0.05);
    const next = OPACITY_LEVELS[(idx + 1) % OPACITY_LEVELS.length];
    setOpacity(next.value);
  };

  const opacityBtn = OPACITY_LEVELS.find(l => Math.abs(l.value - opacity) < 0.05) ?? OPACITY_LEVELS[0];

  const showNextCall = outcome && outcomeSource === 'auto' && !isPracticeMode;

  // Talk ratio color: green (40-60%), amber (30-40% or 60-70%), red (<30% or >70%)
  const talkRatioColor = useCallback((ratio: number) => {
    if (ratio >= 0.4 && ratio <= 0.6) return 'green';
    if (ratio >= 0.3 && ratio <= 0.7) return 'amber';
    return 'red';
  }, []);

  const dismissSummary = useCallback(() => {
    setShowSummary(false);
    setCallSummary(null);
  }, []);

  return (
    <div className="overlay-root">
      <header className="overlay-header" data-tauri-drag-region>
        <div className="status-pill" data-tauri-drag-region>
          <div className={`status-dot ${
            transcriptionState === 'paused'                                        ? 'paused'    :
            transcriptionState === 'recording' && deepgramState === 'streaming'   ? 'active'    :
            transcriptionState === 'recording'                                     ? 'listening' : ''
          }`} />
          <span className="brand" data-tauri-drag-region>BrainSales</span>
        </div>
        <div className="header-controls">
          <div className="view-switcher">
            <button className={`view-btn ${view === "script" ? "active" : ""}`} onClick={() => setView("script")}>Script</button>
            <button className={`view-btn ${view === "transcript" ? "active" : ""}`} onClick={() => setView("transcript")}>Transcript</button>
            <button className={`view-btn ${view === "actions" ? "active" : ""}`} onClick={() => setView("actions")}>⚙</button>
          </div>
          {transcriptionState === 'recording' && (
            <div className={`talk-ratio-gauge ${talkRatioColor(talkRatio)}`} title={`You: ${Math.round(talkRatio * 100)}% | Prospect: ${Math.round((1 - talkRatio) * 100)}%`}>
              <div className="talk-ratio-bar" style={{ width: `${talkRatio * 100}%` }} />
              <span className="talk-ratio-label">{Math.round(talkRatio * 100)}%</span>
            </div>
          )}
          <button
            className={`copilot-btn ${isCompanionActive ? 'active' : ''}`}
            onClick={handleToggleCopilot}
            title={isCompanionActive ? "Co-Pilot is open" : "Open Co-Pilot"}
          >🤖</button>
          <button className="opacity-btn" onClick={cycleOpacity} title={opacityBtn.title}>{opacityBtn.label}</button>
          <button className="close-btn" onClick={handleClose} title="Minimize to tray">✕</button>
        </div>
      </header>

      {/* Waiting overlay — shown when auth is done but web app hasn't connected */}
      {authStatus === 'authenticated' && !isWebAppConnected && (
        <div className="waiting-overlay">
          <div className="waiting-spinner" />
          <span className="waiting-text">Waiting for web app to connect…</span>
          <span className="waiting-hint">Open BrainSales in your browser</span>
        </div>
      )}

      {/* Auth failed overlay */}
      {authStatus === 'failed' && (
        <div className="waiting-overlay error">
          <span className="waiting-icon">⚠</span>
          <span className="waiting-text">Authentication failed</span>
          <span className="waiting-hint">Please relaunch from the web app</span>
        </div>
      )}

      {/* Coaching cue toast (Phase 5D) */}
      {coachingCue && (
        <div className="coaching-toast">
          <span className="coaching-icon">💡</span>
          <span className="coaching-msg">{coachingCue.message}</span>
          <button className="coaching-dismiss" onClick={() => setCoachingCue(null)}>✕</button>
        </div>
      )}

      {/* Call Summary Panel (Phase 5C) */}
      {showSummary && callSummary && (
        <div className="summary-panel">
          <div className="summary-header">
            <span className="summary-title">📊 Call Summary</span>
            <button className="summary-close" onClick={dismissSummary}>✕</button>
          </div>
          <div className="summary-body">
            <p className="summary-text">{callSummary.summary}</p>
            {callSummary.keyPoints.length > 0 && (
              <div className="summary-section">
                <span className="summary-section-title">Key Points</span>
                <ul>{callSummary.keyPoints.map((kp, i) => <li key={i}>{kp}</li>)}</ul>
              </div>
            )}
            {callSummary.followUpItems.length > 0 && (
              <div className="summary-section">
                <span className="summary-section-title">Follow-up Items</span>
                <ul>{callSummary.followUpItems.map((f, i) => <li key={i}>{f}</li>)}</ul>
              </div>
            )}
            {callSummary.objectionsRaised.length > 0 && (
              <div className="summary-section">
                <span className="summary-section-title">Objections Raised</span>
                <ul>{callSummary.objectionsRaised.map((o, i) => <li key={i}>{o}</li>)}</ul>
              </div>
            )}
            <div className="summary-sentiment">
              Prospect Sentiment: <strong>{callSummary.prospectSentiment}</strong>
            </div>
          </div>
        </div>
      )}

      {/* AI Suggestion Chip */}
      {aiSuggestion && (
        <div className={`ai-chip ${aiSuggestion.autoNavigated ? 'auto-nav' : ''}`}>
          <div className="ai-chip-top">
            <span className="ai-chip-icon">✦</span>
            <span className="ai-chip-node">
              {aiSuggestion.autoNavigated ? `AI Navigated to: ${aiSuggestion.title}` : aiSuggestion.title}
            </span>
            <span className={`ai-chip-conf ${aiSuggestion.confidence}`}>
              {aiSuggestion.confidence}
            </span>
          </div>
          {aiSuggestion.reasoning && (
            <div className="ai-chip-reasoning">{aiSuggestion.reasoning}</div>
          )}
          <div className="ai-chip-btns">
            <button className="ai-eval-btn like" onClick={() => handleAIFeedback("accept")}>
              <span className="eval-icon">👍</span> {aiSuggestion.autoNavigated ? 'Correct' : 'Navigate'}
            </button>
            <button className="ai-eval-btn dislike" onClick={() => handleAIFeedback("reject")}>
              <span className="eval-icon">👎</span> {aiSuggestion.autoNavigated ? 'Wrong' : 'Incorrect'}
            </button>
            <button className="ai-dismiss-btn" onClick={handleDismissSuggestion}>✕</button>
          </div>
          {showCorrectionPicker && (
            <div className="ai-correction">
              <div className="ai-correction-label">Pick correct node:</div>
              <input
                className="ai-correction-search"
                type="text"
                placeholder="Search scripts…"
                value={correctionSearch}
                onChange={e => setCorrectionSearch(e.target.value)}
                autoFocus
              />
              <div className="ai-correction-list">
                {(correctionSearch.trim()
                  ? allNodes.filter(n =>
                      n.title.toLowerCase().includes(correctionSearch.toLowerCase()) ||
                      n.script.toLowerCase().includes(correctionSearch.toLowerCase())
                    )
                  : (currentNode?.responses ?? []).map(r => ({ id: r.nextNode, title: r.label, script: "" }))
                ).map((node) => (
                  <button key={node.id} className="ai-correction-btn" onClick={() => handleAICorrection(node.id)}>
                    <span className="ai-correction-title">{node.title}</span>
                    {node.script && (
                      <span className="ai-correction-script">{node.script}</span>
                    )}
                  </button>
                ))}
              </div>
            </div>
          )}
        </div>
      )}

      {/* Call logged toast */}
      {loggedLabel && (
        <div className="logged-toast">✓ Logged — {loggedLabel}</div>
      )}

      <div className="overlay-body">
        {view === "script" ? (
          <>
            {openingScripts.length > 1 && (
              <div className="flow-selector">
                <label className="flow-selector-label">Opening Script</label>
                <select
                  className="flow-selector-select"
                  value={activeFlowId ?? ''}
                  onChange={e => handleChangeFlow(e.target.value)}
                >
                  {openingScripts.map(s => (
                    <option key={s.id} value={s.id}>{s.title}</option>
                  ))}
                </select>
              </div>
            )}

            {currentNode ? (
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
                      <button key={i} className="response-btn" onClick={() => handleNavigate(r.nextNode)}>
                        {r.label}
                      </button>
                    ))}
                  </div>
                )}

                {/* Next Call button — appears when AI auto-detected an outcome */}
                {showNextCall && (
                  <button className="next-call-btn" onClick={handleNextCall}>
                    ↩ Next Call
                  </button>
                )}

                {/* Outcome logging rows */}
                <div className="outcome-log-section">
                  <div className="outcome-log-row dead-end-row">
                    <button className="log-btn dead-end" onClick={() => handleLogOutcome('no_answer')}>📵 No Answer</button>
                    <button className="log-btn dead-end" onClick={() => {
                      if (currentNode?.voicemailNodeId) handleNavigate(currentNode.voicemailNodeId);
                      handleLogOutcome('left_voicemail');
                    }}>📨 Voicemail</button>
                    <button className="log-btn dead-end" onClick={() => handleLogOutcome('disconnected')}>✕ Discon.</button>
                    <button className="log-btn dead-end" onClick={() => handleLogOutcome('redirected')}>↗ Redir.</button>
                  </div>
                  <div className="outcome-log-row success-row">
                    <button className="log-btn success" onClick={() => handleLogOutcome('meeting_set')}>✅ Meeting</button>
                    <button className="log-btn success" onClick={() => handleLogOutcome('follow_up')}>📅 Follow Up</button>
                    <button className="log-btn success" onClick={() => handleLogOutcome('send_info')}>📄 Info</button>
                    <button className="log-btn success" onClick={() => handleLogOutcome('not_interested')}>🚫 No</button>
                    <button className="log-btn success" onClick={() => handleLogOutcome('wrong_person')}>👥 Wrong</button>
                  </div>
                </div>
              </div>
            ) : (
              <div className="empty-state">
                <span className="empty-icon">📋</span>
                <span>Waiting for call to start…</span>
              </div>
            )}

            <div className="compact-controls">
              <div className="compact-controls-left">
                <button className="compact-ctrl-btn start" onClick={() => handleControl('start')} disabled={transcriptionState === 'recording'}>▶</button>
                <button className="compact-ctrl-btn pause" onClick={() => handleControl('pause')} disabled={transcriptionState !== 'recording'}>⏸</button>
                <button className="compact-ctrl-btn stop" onClick={() => handleControl('stop')} disabled={transcriptionState === 'idle'}>■</button>
                <span className="compact-state-dot">
                  {transcriptionState === 'recording' && deepgramState === 'streaming' && <span className="mini-dot active" />}
                  {transcriptionState === 'recording' && deepgramState !== 'streaming' && <span className="mini-dot listening" />}
                  {transcriptionState === 'paused' && <span className="mini-dot paused" />}
                </span>
              </div>
              <div className="compact-controls-right">
                {currentNode?.gatekeeperNodeId && (
                  <button className="compact-quick-btn" onClick={() => handleNavigate(currentNode.gatekeeperNodeId!)} title="Gatekeeper">🛡</button>
                )}
                <button className="compact-quick-btn" onClick={handleResetAIContext} title="New Contact">👤</button>
              </div>
            </div>
          </>
        ) : view === "transcript" ? (
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
        ) : (
          <div className="actions-view">
            <div className="actions-section">
              <div className="actions-section-title">Settings</div>
              <div className="practice-row">
                <span className="practice-label">Practice Mode</span>
                <button className="practice-toggle" onClick={handleTogglePracticeMode} style={{ backgroundColor: isPracticeMode ? 'var(--primary)' : 'var(--border)' }}>
                  <span className={`practice-thumb ${isPracticeMode ? 'on' : ''}`} />
                </button>
              </div>
              <div className="device-divider" />
              <div className="actions-section-title" style={{ marginTop: 4 }}>Audio Devices</div>
              <div className="device-row">
                <label className="device-label">🎙 Microphone</label>
                <select className="device-select" value={selectedInput ?? ''} onFocus={fetchDevices} onChange={e => handleSetDevices(e.target.value || null, null)}>
                  <option value="">System Default</option>
                  {audioDevices.inputs.map(d => <option key={d} value={d}>{d}</option>)}
                </select>
              </div>
              <div className="device-row">
                <label className="device-label">🔊 Speaker / Loopback</label>
                <select className="device-select" value={selectedOutput ?? ''} onFocus={fetchDevices} onChange={e => handleSetDevices(null, e.target.value || null)}>
                  <option value="">System Default</option>
                  {audioDevices.outputs.map(d => <option key={d} value={d}>{d}</option>)}
                </select>
              </div>
              <div className="device-divider" />
              <div className="device-row">
                <div className="device-label-row">
                  <label className="device-label">🎤 Mic Gain (Boost)</label>
                  <span className="device-value">{micGain.toFixed(1)}x</span>
                </div>
                <input
                  type="range"
                  className="device-slider"
                  min="1.0"
                  max="20.0"
                  step="0.5"
                  value={micGain}
                  onChange={e => handleSetMicGain(parseFloat(e.target.value))}
                />
              </div>
              <div className="device-divider" />
              <div className="actions-section-title" style={{ marginTop: 4 }}>Appearance</div>
              <div className="appearance-row">
                <span className="device-label">Theme</span>
                <div className="theme-picker">
                  <button className={`theme-swatch dark ${theme === 'dark' ? 'active' : ''}`} onClick={() => setTheme('dark')} title="Dark" />
                  <button className={`theme-swatch darker ${theme === 'darker' ? 'active' : ''}`} onClick={() => setTheme('darker')} title="Darker (OLED)" />
                  <button className={`theme-swatch slate ${theme === 'slate' ? 'active' : ''}`} onClick={() => setTheme('slate')} title="Slate Blue" />
                  <button className={`theme-swatch purple ${theme === 'purple' ? 'active' : ''}`} onClick={() => setTheme('purple')} title="Deep Purple" />
                  <button className={`theme-swatch rose ${theme === 'rose' ? 'active' : ''}`} onClick={() => setTheme('rose')} title="Rose" />
                  <button className={`theme-swatch teal ${theme === 'teal' ? 'active' : ''}`} onClick={() => setTheme('teal')} title="Teal" />
                </div>
              </div>
              <div className="appearance-row">
                <span className="device-label">Accent Color</span>
                <div className="color-picker-row">
                  <input
                    type="color"
                    className="accent-color-input"
                    value={customPrimary || "#818cf8"}
                    onChange={e => setCustomPrimary(e.target.value)}
                    title="Custom accent color"
                  />
                  {customPrimary && (
                    <button className="accent-reset-btn" onClick={() => setCustomPrimary("")} title="Reset to theme default">✕</button>
                  )}
                </div>
              </div>
              <div className="appearance-row">
                <span className="device-label">Font Size</span>
                <div className="font-size-picker">
                  <button className={`font-size-btn ${fontScale === 0.85 ? 'active' : ''}`} onClick={() => setFontScale(0.85)}>A−</button>
                  <button className={`font-size-btn ${fontScale === 1.0 ? 'active' : ''}`} onClick={() => setFontScale(1.0)}>A</button>
                  <button className={`font-size-btn ${fontScale === 1.15 ? 'active' : ''}`} onClick={() => setFontScale(1.15)}>A+</button>
                </div>
              </div>
            </div>
          </div>
        )}
      </div>

      <footer className="overlay-footer">
        {updateVersion ? (
          <div className="update-banner">
            <span className="update-text">v{updateVersion} available</span>
            <button className="update-btn" onClick={handleUpdate} disabled={updating}>{updating ? "Installing…" : "Update Now"}</button>
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
