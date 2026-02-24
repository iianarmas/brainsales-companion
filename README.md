# Brainsales Companion

Brainsales Companion is a lightweight, production-ready desktop application designed to bridge the gap between local audio capture and the Brainsales web ecosystem. It facilitates real-time transcription and AI-driven conversation highlights for sales professionals.

## 🚀 Key Features

- **Mini-Player Mode**: A compact, non-intrusive UI (320x200) that stays "Always on Top" during your calls.
- **Dynamic Organic Visualizer**: Real-time wave feedback for both Microphone (Indigo) and System Audio (Emerald).
- **Deepgram Integration**: High-performance, low-latency transcription using Deepgram's Nova-2 model.
- **WebSocket Bridge**: Automatically serves transcribed data to the local web app on Port 4141.
- **Premium Aesthetic**: Minimalist, professional design optimized for high-performance workflows.

## 🛠️ Tech Stack

- **Core**: [Tauri](https://tauri.app/) (Rust-based secure app framework)
- **Frontend**: [React](https://react.dev/) + [TypeScript](https://www.typescriptlang.org/)
- **Build Tool**: [Vite](https://vite.dev/)
- **Transcription**: [Deepgram API](https://deepgram.com/)
- **Audio Capture**: [CPAL](https://github.com/RustAudio/cpal)

## 🚦 Getting Started

### Prerequisites

1. **Rust**: [Install Rust](https://www.rust-lang.org/tools/install)
2. **Node.js**: [Install Node.js](https://nodejs.org/)
3. **Tauri Dependencies**: [Follow the Tauri Quick Start](https://tauri.app/v1/guides/getting-started/prerequisites)

### Installation

1. Clone the repository:
   ```bash
   git clone <repository-url>
   cd brainsales-companion
   ```

2. Install dependencies:
   ```bash
   npm install
   ```

3. Setup environment variables:
   Create a `.env` file in `src-tauri/.env`:
   ```env
   DEEPGRAM_API_KEY=your_api_key_here
   ```

### Development

Run the application in development mode:
```bash
npm run tauri dev
```

### Building for Production

To create a production build:
```bash
npm run tauri build
```

## 🔒 Security

This application utilizes Tauri's secure bridge to ensure that local audio capture and transcription data are handled with minimal exposure to external risks.

---

Built by the **Brainsales Team**.
