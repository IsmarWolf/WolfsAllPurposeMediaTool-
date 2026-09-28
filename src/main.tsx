import '@fontsource/roboto/latin-400.css'
import '@fontsource/roboto/latin-500.css'
import '@fontsource/roboto/latin-700.css'
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import App from './App'
import './styles/tailwind.css'

const container = document.getElementById('root')

if (!container) {
  throw new Error('root container not found')
}

createRoot(container).render(
  <StrictMode>
    <App />
  </StrictMode>,
)
