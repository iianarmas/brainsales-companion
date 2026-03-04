# BrainSales Companion

A lightweight desktop companion app for [BrainSales](https://brainsales.app) that listens to your calls in real-time and powers AI-driven navigation suggestions.

## Features

- **Always-on-top mini player** — compact 320×200 window that stays visible during calls
- **Real-time audio visualizer** — live feedback for microphone and system audio
- **Deepgram transcription** — low-latency speech-to-text via Deepgram Nova-2
- **WebSocket bridge** — streams transcripts to the BrainSales web app on port 4141
- **System tray** — minimize to tray instead of closing; starts automatically on Windows login
- **Auto-update** — notifies you when a new version is available and installs it in one click

## Installation (for users)

Download the latest installer from the [Releases](https://github.com/iianarmas/brainsales-companion/releases/latest) page.

> **Windows SmartScreen warning?** Click **More info → Run anyway**. The app is unsigned during the testing phase but is safe to install.

## Development Setup

### Prerequisites

- [Rust](https://www.rust-lang.org/tools/install)
- [Node.js](https://nodejs.org/)
- [Tauri prerequisites](https://tauri.app/start/prerequisites/)

### Getting started

```bash
git clone https://github.com/iianarmas/brainsales-companion
cd brainsales-companion
npm install
```

```bash
npm run tauri dev
```

### Building

```bash
npm run tauri build
```

## Releasing a new version

1. Bump `version` in `src-tauri/tauri.conf.json` and `src-tauri/Cargo.toml`
2. Commit, tag, and push:
   ```powershell
   git add .
   git commit -m "chore: bump version to x.x.x"
   git tag vx.x.x
   git push
   git push --tags
   ```
3. GitHub Actions builds the installer and publishes a GitHub Release automatically.
4. Users already running the app will be prompted to update on next launch.

## Tech Stack

- [Tauri v2](https://tauri.app/) — Rust-based desktop framework
- [React](https://react.dev/) + [TypeScript](https://www.typescriptlang.org/)
- [Vite](https://vite.dev/)
- [Deepgram API](https://deepgram.com/) — speech-to-text
- [CPAL](https://github.com/RustAudio/cpal) — cross-platform audio capture
