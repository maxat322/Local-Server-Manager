/** Акцентные темы. Набор общий с Mega Panel и Mega Agent, чтобы панели выглядели как одна семья. */
export const ACCENTS = {
  mint: { label: 'Мятный', main: '#7de2c3', hi: '#abf2dd', soft: '#12211d', line: '#315e51', rgb: '125, 226, 195' },
  violet: { label: 'Логотип', main: '#d57bea', hi: '#e5abf2', soft: '#1e1221', line: '#56315e', rgb: '213, 123, 234' },
  magenta: { label: 'Пурпурный', main: '#ea7bd7', hi: '#f2abe6', soft: '#21121f', line: '#5e3157', rgb: '234, 123, 215' },
  pink: { label: 'Розовый', main: '#ea7bb3', hi: '#f2abcf', soft: '#21121a', line: '#5e3147', rgb: '234, 123, 179' },
  rose: { label: 'Коралловый', main: '#ea7b8e', hi: '#f2abb7', soft: '#211214', line: '#5e3138', rgb: '234, 123, 142' },
  red: { label: 'Красный', main: '#ea917b', hi: '#f2b9ab', soft: '#211512', line: '#5e3a31', rgb: '234, 145, 123' },
  orange: { label: 'Оранжевый', main: '#eaaf7b', hi: '#f2ccab', soft: '#211912', line: '#5e4631', rgb: '234, 175, 123' },
  amber: { label: 'Янтарный', main: '#eace7b', hi: '#f2e0ab', soft: '#211d12', line: '#5e5331', rgb: '234, 206, 123' },
  lime: { label: 'Лаймовый', main: '#e6ea7b', hi: '#f0f2ab', soft: '#212112', line: '#5d5e31', rgb: '230, 234, 123' },
  grass: { label: 'Травяной', main: '#b6ea7b', hi: '#d1f2ab', soft: '#1a2112', line: '#495e31', rgb: '182, 234, 123' },
  green: { label: 'Зелёный', main: '#7beaa0', hi: '#abf2c3', soft: '#122117', line: '#315e40', rgb: '123, 234, 160' },
  teal: { label: 'Бирюзовый', main: '#7beaea', hi: '#abf2f2', soft: '#122121', line: '#315e5e', rgb: '123, 234, 234' },
  cyan: { label: 'Голубой', main: '#7bccea', hi: '#abdff2', soft: '#121d21', line: '#31525e', rgb: '123, 204, 234' },
  blue: { label: 'Синий', main: '#7bafea', hi: '#abccf2', soft: '#121921', line: '#31465e', rgb: '123, 175, 234' },
  indigo: { label: 'Индиго', main: '#7b8aea', hi: '#abb4f2', soft: '#121421', line: '#31375e', rgb: '123, 138, 234' },
  purple: { label: 'Сиреневый', main: '#a47bea', hi: '#c5abf2', soft: '#171221', line: '#41315e', rgb: '164, 123, 234' },
}

export const DEFAULT_ACCENT = 'mint'

export function applyAccent(name) {
  const a = ACCENTS[name] || ACCENTS[DEFAULT_ACCENT]
  const root = document.documentElement
  root.style.setProperty('--accent', a.main)
  root.style.setProperty('--accent-hi', a.hi)
  root.style.setProperty('--accent-soft', a.soft)
  root.style.setProperty('--accent-line', a.line)
  root.style.setProperty('--accent-rgb', a.rgb)
}

/** Статус сервера, пока о нём ничего не известно: ядро помнит только те, что запускались. */
export const IDLE = { status: 'stopped', pid: null, startedAt: null, exitCode: null, url: null, ports: [], note: null }

/** Сервер выполняется прямо сейчас: запускается или работает. */
export const isActive = (view) => view.status === 'starting' || view.status === 'running'

/** Процессы сервера существуют: он выполняется или стоит на паузе. */
export const isAlive = (view) => isActive(view) || view.status === 'paused'

export const LED = { running: 'ok', starting: 'warn blink', paused: 'info', crashed: 'err', exited: 'warn', stopped: '' }

/** Процесс завершился сам, а не по кнопке: упал или вышел с кодом 0. */
export const endedItself = (view) => view.status === 'crashed' || view.status === 'exited'

export function fmtUptime(ms) {
  const s = Math.max(0, Math.floor(ms / 1000))
  if (s < 60) return s + ' с'
  const m = Math.floor(s / 60)
  if (m < 60) return m + ' мин'
  const h = Math.floor(m / 60)
  if (h < 24) return h + ' ч ' + (m % 60) + ' мин'
  return Math.floor(h / 24) + ' д ' + (h % 24) + ' ч'
}

/** Подпись статуса и её тон для строки сервера. */
export function statusText(view, now) {
  switch (view.status) {
    case 'running':
      return { text: 'работает ' + (view.startedAt ? fmtUptime(now - view.startedAt) : ''), tone: 'ok' }
    case 'starting':
      return { text: 'запускается…', tone: 'warn' }
    case 'paused':
      return { text: 'на паузе', tone: 'info' }
    case 'crashed':
      return { text: view.exitCode == null ? 'не запустился' : 'упал, код ' + view.exitCode, tone: 'err' }
    case 'exited':
      // Код 0, но сервер не работает: не ошибка и не остановка по кнопке.
      return { text: 'вышел сам', tone: 'warn' }
    default:
      return { text: 'остановлен', tone: '' }
  }
}

/** Объём памяти слитно, для строки расхода: 412МБ, 1.3ГБ. */
export function fmtBytes(bytes) {
  const mb = bytes / 1048576
  if (mb < 1000) return Math.round(mb) + 'МБ'
  return (mb / 1024).toFixed(1) + 'ГБ'
}

/** Проценты целыми: строке расхода важна величина, а не десятые. */
export const fmtPct = (value) => Math.round(value) + '%'

/** Момент времени коротко: сегодня — только часы, иначе с датой. */
export function fmtWhen(ms) {
  if (!ms) return '—'
  const d = new Date(ms)
  const time = d.toLocaleTimeString('ru-RU', { hour: '2-digit', minute: '2-digit' })
  if (d.toDateString() === new Date().toDateString()) return 'сегодня ' + time
  return d.toLocaleDateString('ru-RU', { day: '2-digit', month: '2-digit', year: '2-digit' }) + ' ' + time
}

/** Адрес сервера: заданный вручную главнее найденного. */
export const urlOf = (launcher, view) => launcher.url || view.url || null

export const shortUrl = (url) => url.replace(/^https?:\/\//, '').replace(/\/$/, '')

/** Ключ кандидата из сканера: одна и та же команда в разных папках — разные способы запуска. */
export const candidateKey = (c) => c.kind + '|' + c.cwd.toLowerCase() + '|' + c.command

/** Слова для числа: 1 сервер, 2 сервера, 5 серверов. */
export function plural(n, one, few, many) {
  const d = n % 10
  const h = n % 100
  if (d === 1 && h !== 11) return one
  if (d >= 2 && d <= 4 && (h < 12 || h > 14)) return few
  return many
}
