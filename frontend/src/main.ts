import './style.css'
import App from './App.svelte'

document.addEventListener('contextmenu', e => e.preventDefault())

const app = new App({
  target: document.getElementById('app')
})

export default app
