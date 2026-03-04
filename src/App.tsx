import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import "./App.css";
import { AudioVisualizer } from "./components/AudioVisualizer";

function App() {
  const [isActive, setIsActive] = useState(false);
  const [levels, setLevels] = useState({ mic: 0, sys: 0 });
  const [updateVersion, setUpdateVersion] = useState<string | null>(null);
  const [updating, setUpdating] = useState(false);

  const handleClose = async () => {
    await getCurrentWebviewWindow().close();
  };

  const handleUpdate = async () => {
    setUpdating(true);
    await invoke("install_update").catch(() => setUpdating(false));
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

    const unlisten = listen<{ mic: number; sys: number }>("audio-levels", (event) => {
      setLevels(event.payload);
    });

    const unlistenUpdate = listen<string>("update-available", (event) => {
      setUpdateVersion(event.payload);
    });

    return () => {
      unlisten.then(u => u());
      unlistenUpdate.then(u => u());
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
        <button className="close-btn" onClick={handleClose} title="Minimize to tray">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
            <line x1="18" y1="6" x2="6" y2="18"></line>
            <line x1="6" y1="6" x2="18" y2="18"></line>
          </svg>
        </button>
      </header>

      <div className="visualizer-wrapper">
        <AudioVisualizer micLevel={levels.mic} sysLevel={levels.sys} />
      </div>

      {updateVersion ? (
        <footer className="footer update-banner">
          <span className="helper-text">v{updateVersion} available</span>
          <button
            className="update-btn"
            onClick={handleUpdate}
            disabled={updating}
          >
            {updating ? "Installing…" : "Update Now"}
          </button>
        </footer>
      ) : (
        <footer className="footer">
          <span className="helper-text">
            {isActive ? "Capturing real-time audio..." : "Waiting for connection..."}
          </span>
        </footer>
      )}
    </main>
  );
}

export default App;
