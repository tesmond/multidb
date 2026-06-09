import './style.css'
import App from './App.svelte'
import { mount } from 'svelte'

type StartupMark = {
  name: string
  ms: number
  detail?: unknown
}

declare global {
  interface Window {
    __MULTIDB_STARTUP__?: {
      marks: StartupMark[]
      mark: (name: string, detail?: unknown) => void
    }
    __MULTIDB_MARK_STARTUP__?: (name: string, detail?: unknown) => void
    __MULTIDB_REPORT_STARTUP__?: () => void
    ipc?: {
      postMessage: (message: string) => void
    }
  }
}

function startupMark(name: string, detail?: unknown) {
  window.__MULTIDB_STARTUP__?.mark(name, detail)
}

function startupReport() {
  const startup = window.__MULTIDB_STARTUP__
  if (!startup || !window.ipc?.postMessage) return

  const payload = {
    timeOrigin: performance.timeOrigin,
    now: performance.now(),
    marks: startup.marks,
    paint: performance.getEntriesByType('paint').map(entry => ({
      name: entry.name,
      startTime: entry.startTime,
      duration: entry.duration,
    })),
    resources: performance
      .getEntriesByType('resource')
      .filter(entry => entry.name.startsWith('multidb://'))
      .map(entry => ({
        name: entry.name,
        startTime: entry.startTime,
        duration: entry.duration,
        transferSize: (entry as PerformanceResourceTiming).transferSize,
        decodedBodySize: (entry as PerformanceResourceTiming).decodedBodySize,
      })),
  }

  window.ipc.postMessage(JSON.stringify({
    id: '__startup_profile',
    command: '__startup_profile',
    args: payload,
  }))
}

window.__MULTIDB_MARK_STARTUP__ = startupMark
window.__MULTIDB_REPORT_STARTUP__ = startupReport
startupMark('main_module_evaluated')

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

target.replaceChildren()
startupMark('boot_shell_cleared')

const app = mount(App, {
  target
})
startupMark('svelte_mount_returned')

export default app
