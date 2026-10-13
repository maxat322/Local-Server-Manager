import React, { useEffect, useState } from 'react'
import Icon from './Icon.jsx'
import ProjectRow from './ProjectRow.jsx'
import { IDLE, isActive, isAlive } from '../constants.js'

/** Текущее время для счётчиков «работает 12 мин». Тикает, только пока есть что считать. */
function useNow(ticking) {
  const [now, setNow] = useState(Date.now())
  useEffect(() => {
    if (!ticking) return
    setNow(Date.now())
    const timer = setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(timer)
  }, [ticking])
  return now
}

/** Главная отвечает на один вопрос: что сейчас запущено. */
export default function Home({ config, procs, busy, logId, act }) {
  const launchers = config.projects.flatMap(p => p.launchers)
  const running = launchers.filter(l => isActive(procs[l.id] || IDLE)).length
  // Приостановленные не работают, но и запускать их заново не нужно: у них свой счёт.
  const alive = launchers.filter(l => isAlive(procs[l.id] || IDLE)).length
  const paused = alive - running
  const now = useNow(running > 0)

  if (!config.projects.length) {
    return (
      <div className="card empty">
        <div className="ic"><Icon name="folder" size={22} /></div>
        <h3>Пока ни одного репозитория</h3>
        <p>
          Укажите папку проекта: приложение само найдёт npm-скрипты и файлы запуска
          (.cmd, .bat, .ps1) и предложит, чем поднимать сервер.
        </p>
        <button className="primary with-icon" onClick={act.add}>
          <Icon name="plus" size={15} />Добавить репозиторий
        </button>
      </div>
    )
  }

  return (
    <>
      <div className="section-title">
        <span>СЕРВЕРЫ · РАБОТАЕТ {running} ИЗ {launchers.length}{paused ? ' · НА ПАУЗЕ ' + paused : ''}</span>
        <span className="row" style={{ marginLeft: 'auto', gap: 6 }}>
          <button className="sm" disabled={alive === launchers.length} onClick={act.startAll}>Запустить все</button>
          <button className="sm danger" disabled={!alive} onClick={act.stopAll}>Остановить все</button>
          <button className="sm primary with-icon" onClick={act.add}>
            <Icon name="plus" size={13} />Добавить
          </button>
        </span>
      </div>

      <div className="card">
        {config.projects.map(p => (
          <ProjectRow key={p.id} project={p} procs={procs} now={now} busy={busy} logId={logId} act={act} />
        ))}
      </div>
    </>
  )
}
