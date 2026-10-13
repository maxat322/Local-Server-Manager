import React from 'react'
import { createRoot } from 'react-dom/client'
// Шрифты лежат в сборке: приложение должно выглядеть одинаково и без сети.
import '@fontsource/space-grotesk/400.css'
import '@fontsource/space-grotesk/500.css'
import '@fontsource/space-grotesk/700.css'
import '@fontsource/jetbrains-mono/400.css'
import '@fontsource/jetbrains-mono/500.css'
import '@fontsource/jetbrains-mono/700.css'
import './styles.css'
import App from './App.jsx'

createRoot(document.getElementById('root')).render(<App />)
