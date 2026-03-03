import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import "./App.css";
import { AudioVisualizer } from "./components/AudioVisualizer";

function App() {
  const [isActive, setIsActive] = useState(false);
  const [levels, setLevels] = useState({ mic: 0, sys: 0 });

  const handleClose = async () => {
    await getCurrentWebviewWindow().close();
  };

  useEffect(() => {
    const startCompanion = async () => {
      try {
        await invoke("start_companion");
        setIsActive(true);
      } catch (err) {
        setIsActive(false);
      }
    };
    startCompanion();

    // Listen for audio levels from Rust backend
    const unlisten = listen<{ mic: number; sys: number }>("audio-levels", (event) => {
      setLevels(event.payload);
    });

    return () => {
      unlisten.then(u => u());
    };
  }, []);

  return (
    <main className="container">
      <header className="header">
        <div className="status-badge">
          <div className={`status-dot ${isActive ? 'active' : ''}`} />
          <span className="status-text">
            {isActive ? "System Active" : "Standing By"}
          </span>
        </div>
        <button className="close-btn" onClick={handleClose} title="Close">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
            <line x1="18" y1="6" x2="6" y2="18"></line>
            <line x1="6" y1="6" x2="18" y2="18"></line>
          </svg>
        </button>
      </header>

      <div className="visualizer-wrapper">
        <AudioVisualizer micLevel={levels.mic} sysLevel={levels.sys} />
      </div>

      <footer className="footer">
        <span className="helper-text">
          {isActive ? "Capturing real-time audio..." : "Waiting for connection..."}
        </span>
      </footer>
    </main>
  );
}

export default App;
