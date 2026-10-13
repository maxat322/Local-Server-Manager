import { invoke as tauriInvoke } from '@tauri-apps/api/core'
import { listen as tauriListen } from '@tauri-apps/api/event'
import { open as openDialog } from '@tauri-apps/plugin-dialog'

/**
 * Страницу можно открыть и в обычном браузере (`npm run dev`), где ядра нет: тогда подставляется
 * заглушка с выдуманными серверами. Это режим для вёрстки; в сборку заглушка не попадает.
 */
const inTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
const mock = !inTauri && import.meta.env.DEV ? import('./mock.js') : null

const invoke = (cmd, args) => (mock ? mock.then(m => m.invoke(cmd, args)) : tauriInvoke(cmd, args))
const listen = (event, fn) =>
  mock ? mock.then(m => m.listen(event, fn)) : tauriListen(event, e => fn(e.payload))

const SCRIPT_FILTERS = [
  { name: 'Файлы запуска', extensions: ['cmd', 'bat', 'ps1', 'exe', 'js', 'mjs', 'cjs', 'py'] },
  { name: 'Все файлы', extensions: ['*'] },
]

export const api = {
  config: () => invoke('get_config'),
  procs: () => invoke('get_procs'),
  info: () => invoke('app_info'),
  scan: (path) => invoke('scan_folder', { path }),
  addProject: (project) => invoke('add_project', { project }),
  updateProject: (project) => invoke('update_project', { project }),
  removeProject: (id) => invoke('remove_project', { id }),
  start: (id) => invoke('start_launcher', { id }),
  stop: (id) => invoke('stop_launcher', { id }),
  restart: (id) => invoke('restart_launcher', { id }),
  pause: (id) => invoke('pause_launcher', { id }),
  resume: (id) => invoke('resume_launcher', { id }),
  startAll: () => invoke('start_all'),
  stopAll: () => invoke('stop_all'),
  output: (id) => invoke('get_output', { id }),
  clearOutput: (id) => invoke('clear_output', { id }),
  termInput: (id, data) => invoke('term_input', { id, data }),
  termResize: (cols, rows) => invoke('term_resize', { cols, rows }),
  ports: () => invoke('get_ports'),
  forgetPort: (port) => invoke('forget_port', { port }),
  saveSettings: (settings) => invoke('save_settings', { settings }),
  openUrl: (url) => invoke('open_url', { url }),
  openFolder: (path) => invoke('open_folder', { path }),
  quit: () => invoke('quit_app'),
  pickFolder: () =>
    mock ? mock.then(m => m.pickFolder()) : openDialog({ directory: true, title: 'Папка репозитория' }),
  pickFile: (dir) =>
    mock
      ? mock.then(m => m.pickFile(dir))
      : openDialog({ defaultPath: dir, title: 'Файл запуска', filters: SCRIPT_FILTERS }),
}

/**
 * Подписка на событие ядра. Возвращает функцию отписки сразу, хотя сама подписка асинхронна:
 * эффект React может размонтироваться раньше, чем она оформится.
 */
export function on(event, fn) {
  let off = null
  let dead = false
  listen(event, fn).then(unlisten => {
    if (dead) unlisten()
    else off = unlisten
  })
  return () => {
    dead = true
    if (off) off()
  }
}

/** Ядро отклоняет вызов строкой, а не объектом ошибки. */
export const errText = (e) => (typeof e === 'string' ? e : e?.message || String(e))
