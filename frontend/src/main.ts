import './style.css'
import App from './App.svelte'

document.addEventListener('contextmenu', e => {
  if (!import.meta.env.DEV) {
    e.preventDefault()
    return
  }

  const target = e.target as HTMLElement | null
  const insideSqlEditor = !!target?.closest('.cm-editor')

  // In dev, allow native context menu (Inspect) inside SQL editor only.
  if (!insideSqlEditor) {
    e.preventDefault()
  }
})

const app = new App({
  target: document.getElementById('app')
})

export default app
