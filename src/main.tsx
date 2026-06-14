import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { installDevMock } from './dev/tauriMock'
// Styles are imported in App.tsx via styles/index.css
import App from './App.tsx'

// Install the dev-only Tauri IPC mock before React renders, so stores that call
// invoke() during initial load (projectStore, agentStore, …) find data even in a
// plain browser. No-op in production and inside the real Tauri webview.
installDevMock()

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
)
