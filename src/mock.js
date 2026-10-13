/**
 * Заглушка ядра для вёрстки в обычном браузере (`npm run dev` без Tauri).
 * Подключается только в режиме разработки, см. ipc.js. Процессов не запускает:
 * статусы и вывод выдуманы, чтобы было на чём смотреть все состояния экрана.
 */
const listeners = {}
const emit = (event, payload) => (listeners[event] || []).forEach(fn => fn(payload))

export function listen(event, fn) {
  ;(listeners[event] ||= []).push(fn)
  return () => { listeners[event] = listeners[event].filter(f => f !== fn) }
}

let seq = 100
const newId = () => 'm' + seq++

const launcher = (title, kind, command, cwd, extra = {}) => ({
  id: newId(), title, kind, command, cwd, url: null, autostart: false, ...extra,
})

const DEV = 'C:\\Users\\designer\\Desktop\\Dev\\'
const config = {
  settings: {
    accent: 'mint', launchWithWindows: false, startHidden: true, closeToTray: true, notifyOnCrash: true, logLines: 5000,
  },
  projects: [
    { id: newId(), name: 'SZ Learn', path: DEV + 'SZ Learn', launchers: [launcher('dev.cmd', 'script', DEV + 'SZ Learn\\dev.cmd', DEV + 'SZ Learn')] },
    {
      id: newId(), name: 'Mega Agent', path: DEV + 'Mega Agent', launchers: [
        launcher('npm run dev', 'npm', 'npm run dev', DEV + 'Mega Agent'),
        launcher('web · npm run dev', 'npm', 'npm run dev', DEV + 'Mega Agent\\web'),
      ],
    },
    { id: newId(), name: 'RAG-chat', path: DEV + 'RAG-chat', launchers: [launcher('start_local.cmd', 'script', DEV + 'RAG-chat\\start_local.cmd', DEV + 'RAG-chat')] },
    { id: newId(), name: 'Boom cats', path: DEV + 'Boom cats', launchers: [launcher('pnpm run dev', 'npm', 'pnpm run dev', DEV + 'Boom cats')] },
  ],
}

const procs = {}
const logs = {}
const copy = (v) => JSON.parse(JSON.stringify(v))
const pushProcs = () => emit('procs', Object.values(procs))

/** Строка заглушки → кусок вывода терминала, каким его отдаёт настоящее ядро. */
const chunk = (line) => ({
  n: line.n,
  s: (line.k === 'sys' ? '\x1b[36m▸ ' + line.s + '\x1b[0m' : line.s) + '\r\n',
})

function log(id, k, s) {
  const list = (logs[id] ||= [])
  const line = { n: (list.at(-1)?.n || 0) + 1, k, s }
  list.push(line)
  emit('term', [{ id, chunks: [chunk(line)] }])
}

const BANNER = [
  '',
  '  \x1b[32m\x1b[1mVITE\x1b[22m v5.4.11\x1b[39m  \x1b[2mready in\x1b[22m \x1b[1m412\x1b[22m ms',
  '',
  '  \x1b[32m➜\x1b[39m  \x1b[1mLocal\x1b[22m:   \x1b[36mhttp://localhost:\x1b[1m%PORT%\x1b[22m/\x1b[39m',
  '  \x1b[32m➜\x1b[39m  \x1b[1mNetwork\x1b[22m: \x1b[2muse --host to expose\x1b[22m',
]

function start(id, port) {
  const l = config.projects.flatMap(p => p.launchers).find(x => x.id === id)
  if (!l) throw 'Способ запуска не найден'
  procs[id] = { id, status: 'starting', pid: 4000 + seq++, startedAt: Date.now(), exitCode: null, url: null, ports: [], note: null }
  log(id, 'sys', 'Запуск: ' + l.command)
  pushProcs()
  setTimeout(() => {
    if (procs[id]?.status !== 'starting') return
    BANNER.forEach(s => log(id, 'out', s.replace('%PORT%', port)))
    Object.assign(procs[id], { status: 'running', url: 'http://localhost:' + port + '/', ports: [port] })
    pushProcs()
  }, 1200)
}

function stop(id) {
  if (!procs[id] || !['starting', 'running'].includes(procs[id].status)) return
  Object.assign(procs[id], { status: 'stopped', pid: null, startedAt: null, exitCode: 1, url: null, ports: [] })
  log(id, 'sys', 'Остановлен')
  pushProcs()
}

// Стартовая картина: один работает давно, один только что упал.
{
  const [learn, agent, rag, cats] = config.projects
  const a = learn.launchers[0].id
  procs[a] = { id: a, status: 'running', pid: 4012, startedAt: Date.now() - 47 * 60000, exitCode: null, url: 'http://localhost:7430', ports: [7430, 7431] }
  logs[a] = [
    { n: 1, k: 'sys', s: 'Запуск: ' + learn.launchers[0].command },
    { n: 2, k: 'out', s: '[sz-learn] sync env...' },
    { n: 3, k: 'out', s: '[sz-learn] starting web :7430 + api :7431' },
    { n: 4, k: 'out', s: '\x1b[36m[web]\x1b[39m   \x1b[32m➜\x1b[39m  Local:   \x1b[36mhttp://localhost:7430/\x1b[39m' },
    { n: 5, k: 'out', s: '\x1b[35m[api]\x1b[39m Server listening at http://127.0.0.1:7431' },
    { n: 6, k: 'err', s: '\x1b[35m[api]\x1b[39m \x1b[33mwarn\x1b[39m  slow query 812ms: SELECT * FROM lessons' },
  ]
  const b = agent.launchers[1].id
  procs[b] = { id: b, status: 'running', pid: 4040, startedAt: Date.now() - 95000, exitCode: null, url: 'http://localhost:5291/', ports: [5291] }
  const d = cats.launchers[0].id
  const busy = 'Порт 8765 уже занят: похоже, панель уже запущена. Адрес: http://127.0.0.1:8765'
  procs[d] = { id: d, status: 'exited', pid: null, startedAt: null, exitCode: 0, url: null, ports: [], note: busy }
  logs[d] = [
    { n: 1, k: 'sys', s: 'Запуск: ' + cats.launchers[0].command },
    { n: 2, k: 'out', s: busy },
    { n: 3, k: 'sys', s: 'Процесс завершился сам, код выхода 0' },
  ]
  const c = rag.launchers[0].id
  procs[c] = {
    id: c, status: 'crashed', pid: null, startedAt: null, exitCode: 1, url: null, ports: [],
    note: '[ERROR] .env not found - copy .env.example and set POLZA_API_KEY',
  }
  logs[c] = [
    { n: 1, k: 'sys', s: 'Запуск: ' + rag.launchers[0].command },
    { n: 2, k: 'out', s: '[ERROR] .env not found - copy .env.example and set POLZA_API_KEY' },
    { n: 3, k: 'sys', s: 'Процесс упал, код выхода 1' },
  ]
}

const SCAN = {
  name: 'SZapp', path: DEV + 'SZapp',
  candidates: [
    ['npm', 'npm run dev', 'npm run dev', 'package.json', 'node scripts/start-local.mjs', 100],
    ['script', 'start-local.cmd', DEV + 'SZapp\\start-local.cmd', 'файл', '', 90],
    ['npm', 'npm run start', 'npm run start', 'package.json', 'node scripts/start-local.mjs', 80],
    ['npm', 'npm run dev:api', 'npm run dev:api', 'package.json', 'node scripts/dev-api.mjs', 50],
    ['npm', 'npm run dev:web', 'npm run dev:web', 'package.json', 'concurrently -n login,staff,parent "npm run dev:login" "npm run dev:staff"', 50],
    ['npm', 'npm run build', 'npm run build', 'package.json', 'npm run build -w @szapp/web-login && npm run build -w @szapp/api', 10],
    ['npm', 'npm run db:migrate', 'npm run db:migrate', 'package.json', 'npm run db:migrate -w @szapp/api', 10],
    ['npm', 'npm run lint', 'npm run lint', 'package.json', 'npm run lint -w @szapp/shared', 10],
    ['command', '.venv\\Scripts\\python.exe webui.py', '.venv\\Scripts\\python.exe webui.py', 'из start_webui.bat',
      'Сервер из этого файла напрямую: без отдельного окна, вывод во встроенный терминал', 91],
    ['script', 'start_webui.bat', DEV + 'SZapp\\start_webui.bat', 'файл', '', 90,
      'Сам открывает отдельное окно консоли (команда start): вывод сервера уйдёт туда, а не во встроенный терминал.'],
  ].map(([kind, title, command, source, detail, score, warning = null]) => ({
    kind, title, command, cwd: DEV + 'SZapp', source, detail, score, recommended: score >= 50, warning,
  })).sort((a, b) => b.score - a.score),
}

const HOUR = 3600000
const ago = (hours) => Date.now() - hours * HOUR
let PORTS = [
  [3000, true, 'node.exe', null, 240, 0],
  [5173, false, 'node.exe', 'Boom cats', 90, 26],
  [5175, true, 'node.exe', null, 240, 0],
  [5291, true, 'node.exe', 'Mega Agent · web · npm run dev', 30, 0],
  [5432, true, 'postgres.exe', null, 400, 0],
  [7430, true, 'node.exe', 'SZ Learn', 120, 0],
  [7431, true, 'node.exe', 'SZ Learn', 120, 0],
  [7440, false, 'python.exe', 'RAG-chat', 70, 49],
  [8311, true, 'python.exe', null, 12, 0],
  [8765, true, 'node.exe', null, 3, 0],
  [9390, false, 'node.exe', null, 300, 170],
].map(([port, open, process, owner, first, last]) => ({
  port, open, pid: open ? 4000 + port : null, process, owner, firstSeen: ago(first), lastSeen: ago(last),
}))

let nextPort = 5173

const handlers = {
  get_config: () => copy(config),
  get_procs: () => Object.values(procs),
  app_info: () => ({ version: '0.1.0', dataDir: 'C:\\Users\\designer\\AppData\\Roaming\\com.localservermanager.app', loadError: null }),
  scan_folder: () => copy(SCAN),
  add_project: ({ project }) => {
    config.projects.push({ ...project, id: newId(), launchers: project.launchers.map(l => ({ ...l, id: newId() })) })
    return copy(config)
  },
  update_project: ({ project }) => {
    const i = config.projects.findIndex(p => p.id === project.id)
    config.projects[i] = { ...project, launchers: project.launchers.map(l => ({ ...l, id: l.id || newId() })) }
    return copy(config)
  },
  remove_project: ({ id }) => {
    config.projects = config.projects.filter(p => p.id !== id)
    return copy(config)
  },
  start_launcher: ({ id }) => start(id, nextPort++),
  stop_launcher: ({ id }) => stop(id),
  restart_launcher: ({ id }) => { stop(id); start(id, nextPort++) },
  pause_launcher: ({ id }) => { procs[id].status = 'paused'; pushProcs() },
  resume_launcher: ({ id }) => { procs[id].status = 'running'; pushProcs() },
  start_all: () => {
    config.projects.flatMap(p => p.launchers).forEach(l => {
      if (!['starting', 'running'].includes(procs[l.id]?.status)) start(l.id, nextPort++)
    })
    return 0
  },
  stop_all: () => Object.keys(procs).forEach(stop),
  get_output: ({ id }) => (logs[id] || []).map(chunk),
  clear_output: ({ id }) => { logs[id] = [] },
  // Эхо набранного: видно, что ввод в терминал доходит.
  term_input: ({ id, data }) => {
    const list = (logs[id] ||= [])
    const n = (list.at(-1)?.n || 0) + 1
    list.push({ n, k: 'raw', s: '' })
    emit('term', [{ id, chunks: [{ n, s: data.replace(/\r/g, '\r\n') }] }])
  },
  term_resize: () => {},
  get_ports: () => copy(PORTS),
  forget_port: ({ port }) => {
    PORTS = PORTS.filter(p => p.port !== port)
    return copy(PORTS)
  },
  save_settings: ({ settings }) => { config.settings = settings; return copy(config) },
  open_url: ({ url }) => { window.open(url, '_blank') },
  open_folder: () => {},
  quit_app: () => {},
}

export async function invoke(cmd, args) {
  if (!handlers[cmd]) throw 'Неизвестная команда: ' + cmd
  return handlers[cmd](args || {})
}

export const pickFolder = async () => SCAN.path
export const pickFile = async (dir) => dir + '\\scripts\\dev-server.cmd'
