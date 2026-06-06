import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
// Styles are imported in App.tsx via styles/index.css
import App from './App.tsx'

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
)
