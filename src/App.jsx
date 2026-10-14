import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import Icon from './components/Icon.jsx'
import Home from './components/Home.jsx'
import TerminalPanel from './components/TerminalPanel.jsx'
import Ports from './components/Ports.jsx'
import AddProjectDialog from './components/AddProjectDialog.jsx'
import ProjectDialog from './components/ProjectDialog.jsx'
import SettingsPage from './components/SettingsPage.jsx'
import ConfirmDialog from './components/ConfirmDialog.jsx'
import { api, errText, on } from './ipc.js'
import { IDLE, applyAccent, endedItself, isActive, isAlive, plural } from './constants.js'

const byId = (list) => Object.fromEntries(list.map(v => [v.id, v]))
/**
 * Запуски мимо менеджера по способу запуска. Их может быть несколько (сервер и его база, две копии):
 * строка показывает первый, а порты и число процессов — по всем. Остановка гасит все.
 */
function groupForeign(list) {
  const out = {}
  for (const f of list) {
    const g = out[f.id]
    out[f.id] = g
      ? { ...g, runs: g.runs + 1, pids: [...g.pids, ...f.pids], ports: [...new Set([...g.ports, ...f.ports])].sort((a, b) => a - b) }
      : { ...f, runs: 1 }
  }
  return out
}
/** Завершение раньше этого срока после запуска считаем неудавшимся запуском. */
const QUICK_EXIT_MS = 20000
const BUSY_MIN_MS = 400

export default function App() {
  const [config, setConfig] = useState(null)
  const [procs, setProcs] = useState({})
  const [info, setInfo] = useState(null)
  // Расход ресурсов по репозиториям: ядро присылает его, пока окно на экране.
  const [usage, setUsage] = useState({ totalMem: 0, projects: {} })
  // Серверы из списка, запущенные мимо менеджера: из своего терминала, двойным щелчком по .bat.
  const [foreign, setForeign] = useState({})
  // Серверы, над которыми прямо сейчас выполняется действие: их кнопки заперты.
  const [busy, setBusy] = useState(() => new Set())
  const [logId, setLogId] = useState(null)
  const [screen, setScreen] = useState('home')

  const [scan, setScan] = useState(null)
  const [editing, setEditing] = useState(null)
  const [removing, setRemoving] = useState(null)
  const [quitting, setQuitting] = useState(false)
  const [settingsOpen, setSettingsOpen] = useState(false)
  const [saving, setSaving] = useState(false)

  const [toast, setToast] = useState(null)
  const toastTimer = useRef(null)
  const say = useCallback((text, bad = false) => {
    clearTimeout(toastTimer.current)
    setToast({ text, bad })
    toastTimer.current = setTimeout(() => setToast(null), bad ? 6000 : 3000)
  }, [])

  useEffect(() => {
    // Событие со статусами может обогнать ответ на первый запрос: тогда ответ уже устарел.
    let live = false
    let before = {}
    const onProcs = (list) => {
      live = true
      const next = byId(list)
      // Сервер завершился сам почти сразу после запуска: показываем его терминал, там причина.
      // Терминал, который уже открыт, не подменяем: пользователь в нём работает.
      const failed = list.find(v => {
        const was = before[v.id]
        return endedItself(v) && was && isAlive(was) && Date.now() - was.startedAt < QUICK_EXIT_MS
      })
      if (failed) setLogId(open => open ?? failed.id)
      before = next
      setProcs(next)
    }
    const offs = [
      on('procs', onProcs),
      on('usage', setUsage),
      on('foreign', list => setForeign(groupForeign(list))),
      on('ask-quit', () => setQuitting(true)),
    ]
    Promise.all([api.config(), api.procs(), api.info()])
      .then(([cfg, list, about]) => {
        setConfig(cfg)
        applyAccent(cfg.settings.accent)
        setInfo(about)
        if (!live) {
          before = byId(list)
          setProcs(before)
        }
        if (about.loadError) say(about.loadError, true)
      })
      .catch(e => say(errText(e), true))
    api.foreign().then(list => setForeign(groupForeign(list))).catch(() => {})
    return () => offs.forEach(off => off())
  }, [say])

  const applyConfig = (cfg) => {
    setConfig(cfg)
    applyAccent(cfg.settings.accent)
  }

  const guard = async (fn) => {
    try { return await fn() } catch (e) { say(errText(e), true) }
  }

  /**
   * Действие над одним сервером: на время выполнения его кнопки запираются. Запираются не меньше
   * чем на BUSY_MIN_MS: первая кнопка меняет смысл сразу после нажатия (пауза ↔ продолжить),
   * и второй щелчок двойного клика иначе тут же отменял бы первый.
   */
  const run = async (id, fn) => {
    setBusy(prev => new Set(prev).add(id))
    await Promise.all([guard(() => fn(id)), new Promise(done => setTimeout(done, BUSY_MIN_MS))])
    setBusy(prev => {
      const next = new Set(prev)
      next.delete(id)
      return next
    })
  }

  const act = {
    start: (id) => run(id, api.start),
    stop: (id) => run(id, api.stop),
    restart: (id) => run(id, api.restart),
    pause: (id) => run(id, api.pause),
    resume: (id) => run(id, api.resume),
    stopForeign: (id) => run(id, api.stopForeign),
    restartForeign: (id) => run(id, api.restartForeign),
    startAll: () => guard(async () => {
      const failed = await api.startAll()
      if (failed) say('Не запустилось: ' + failed + '. Причина видна в терминале сервера', true)
    }),
    stopAll: () => guard(api.stopAll),
    add: () => guard(async () => {
      const dir = await api.pickFolder()
      if (dir) setScan(await api.scan(dir))
    }),
    edit: setEditing,
    remove: setRemoving,
    showLogs: setLogId,
    openUrl: (url) => guard(() => api.openUrl(url)),
    openFolder: (path) => guard(() => api.openFolder(path)),
  }

  /** Сохранение из диалога: окно закрывается только при успехе, иначе введённое пропало бы. */
  const commit = async (request, close, done) => {
    setSaving(true)
    try {
      applyConfig(await request())
      close()
      if (done) say(done)
    } catch (e) { say(errText(e), true) } finally { setSaving(false) }
  }

  const launchers = useMemo(() => (config ? config.projects.flatMap(p => p.launchers) : []), [config])
  const running = launchers.filter(l => isActive(procs[l.id] || IDLE)).length
  const alive = launchers.filter(l => isAlive(procs[l.id] || IDLE)).length
  const paused = alive - running

  const logTarget = useMemo(() => {
    if (!config || !logId) return null
    for (const project of config.projects) {
      const launcher = project.launchers.find(l => l.id === logId)
      if (launcher) return { project, launcher }
    }
    return null
  }, [config, logId])

  if (!config) return null

  const removingActive = removing ? removing.launchers.some(l => isAlive(procs[l.id] || IDLE)) : false

  return (
    <div className="app">
      <div className="rail">
        <div className="mark"><Icon name="server" size={18} /></div>
        <div className={'rail-item' + (screen === 'home' ? ' on' : '')} title="Серверы" onClick={() => setScreen('home')}>
          <Icon name="home" size={19} />
        </div>
        <div className={'rail-item' + (screen === 'ports' ? ' on' : '')} title="Порты" onClick={() => setScreen('ports')}>
          <Icon name="plug" size={19} />
        </div>
        <div className="rail-foot">
          <div className="rail-item" title="Настройки" onClick={() => setSettingsOpen(true)}>
            <Icon name="gear" size={19} />
          </div>
        </div>
      </div>

      <div className="main">
        <div className="topbar">
          <div className="sweep-track"><div className="sweep" /></div>
          <span className={'led' + (running ? ' ok' : '')} />
          <span style={{ fontSize: 16, fontWeight: 700, letterSpacing: '-0.01em' }}>LOCAL SERVER MANAGER</span>
          {screen === 'ports' && <span className="mn" style={{ fontSize: 11, color: 'var(--text-4)' }}>/ Порты</span>}
          <span className="mn" style={{ marginLeft: 'auto', fontSize: 11, color: running ? 'var(--accent)' : 'var(--text-3)' }}>
            {running ? 'РАБОТАЕТ ' + running + ' ИЗ ' + launchers.length : paused ? 'ВСЁ НА ПАУЗЕ' : 'ВСЁ ОСТАНОВЛЕНО'}
            {running && paused ? ' · НА ПАУЗЕ ' + paused : ''}
          </span>
        </div>

        <div className="screen">
          {screen === 'home' && <Home config={config} procs={procs} usage={usage} foreign={foreign} busy={busy} logId={logId} act={act} />}
          {screen === 'ports' && <Ports say={say} />}
        </div>

        {screen === 'home' && logTarget && (
          <TerminalPanel
            project={logTarget.project} launcher={logTarget.launcher}
            view={procs[logId] || IDLE} busy={busy.has(logId)}
            scrollback={config.settings.logLines} accent={config.settings.accent}
            act={act} say={say} onClose={() => setLogId(null)}
          />
        )}
      </div>

      {scan && (
        <AddProjectDialog
          scan={scan} busy={saving} say={say} onClose={() => setScan(null)}
          onAdd={(project) => commit(() => api.addProject(project), () => setScan(null), 'Репозиторий добавлен')}
        />
      )}

      {editing && (
        <ProjectDialog
          project={editing} busy={saving} say={say} onClose={() => setEditing(null)}
          onSave={(project) => commit(() => api.updateProject(project), () => setEditing(null), 'Сохранено')}
        />
      )}

      {removing && (
        <ConfirmDialog
          title={'Убрать «' + removing.name + '» из списка?'}
          consequences={
            (removingActive ? 'Сервер сейчас работает и будет остановлен. ' : '') +
            'Папка и файлы проекта останутся на месте: исчезнет только запись в этом приложении.'
          }
          confirmLabel="Убрать" danger busy={saving}
          onConfirm={() => commit(() => api.removeProject(removing.id), () => setRemoving(null))}
          onClose={() => !saving && setRemoving(null)}
        />
      )}

      {settingsOpen && (
        <SettingsPage
          settings={config.settings} info={info} say={say}
          onSaved={applyConfig} onOpenFolder={act.openFolder} onClose={() => setSettingsOpen(false)}
        />
      )}

      {/* Вопрос о выходе приходит из трея или от крестика окна в любой момент, поэтому он
          последний в разметке: иначе открытые настройки заслонили бы его. */}
      {quitting && (
        <ConfirmDialog
          title="Выйти и остановить серверы?"
          consequences={
            'Сейчас ' + plural(alive, 'запущен', 'запущены', 'запущено') + ' ' + alive + ' ' +
            plural(alive, 'сервер', 'сервера', 'серверов') + '. Вместе с приложением ' +
            plural(alive, 'он будет остановлен', 'они будут остановлены', 'они будут остановлены') + '. ' +
            (config.settings.closeToTray
              ? 'Чтобы серверы продолжали работать, просто закройте окно: приложение останется в трее.'
              : 'Чтобы крестик сворачивал окно в трей и серверы продолжали работать, включите это в настройках.')
          }
          confirmLabel="Выйти" danger
          onConfirm={() => guard(api.quit)}
          onClose={() => setQuitting(false)}
        />
      )}

      {toast && <div className={'toast' + (toast.bad ? ' bad' : '')}>{toast.text}</div>}
    </div>
  )
}
