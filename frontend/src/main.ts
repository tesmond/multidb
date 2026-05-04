import './style.css'
import App from './App.svelte'
import { mount } from 'svelte'

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

const target = document.getElementById('app')

if (!target) {
  throw new Error('App root element #app was not found')
}

const app = mount(App, {
  target
})

export default app
