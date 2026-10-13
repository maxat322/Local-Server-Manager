import React, { useEffect, useState } from 'react'
import Icon from './Icon.jsx'
import LauncherPicker, { toLauncher } from './LauncherPicker.jsx'
import { api, errText } from '../ipc.js'
import { candidateKey } from '../constants.js'

const KIND_LABEL = { npm: 'npm-скрипт', script: 'файл', command: 'команда' }

/** Правка репозитория: имя, способы запуска и их параметры. */
export default function ProjectDialog({ project, busy, say, onSave, onClose }) {
  const [name, setName] = useState(project.name)
  const [launchers, setLaunchers] = useState(() => project.launchers.map(l => ({ ...l })))
  // Сканирование папки для «добавить способ запуска»: null, пока не просили.
  const [found, setFound] = useState(null)
  const [picked, setPicked] = useState(() => new Set())

  useEffect(() => {
    const esc = (e) => e.key === 'Escape' && onClose()
    window.addEventListener('keydown', esc)
    return () => window.removeEventListener('keydown', esc)
  }, [onClose])

  const patch = (index, fields) =>
    setLaunchers(list => list.map((l, i) => (i === index ? { ...l, ...fields } : l)))

  const rescan = async () => {
    try {
      const result = await api.scan(project.path)
      const have = new Set(launchers.map(candidateKey))
      setFound(result.candidates.filter(c => !have.has(candidateKey(c))))
    } catch (e) {
      // Папку могли переименовать или удалить: ручной ввод остаётся доступен.
      say(errText(e), true)
      setFound([])
    }
  }

  const toggle = (key) => setPicked(prev => {
    const next = new Set(prev)
    if (!next.delete(key)) next.add(key)
    return next
  })

  const addManual = (c) => {
    const key = candidateKey(c)
    setFound(list => (list.some(x => candidateKey(x) === key) ? list : [...list, c]))
    setPicked(prev => new Set(prev).add(key))
  }

  const save = () => {
    const added = (found || []).filter(c => picked.has(candidateKey(c))).map(toLauncher)
    onSave({ ...project, name, launchers: [...launchers, ...added] })
  }

  return (
    <div className="overlay" onClick={onClose}>
      <div className="modal wide" onClick={e => e.stopPropagation()}>
        <h2>Репозиторий</h2>
        <div className="sub mn cut" style={{ fontSize: 12, marginBottom: 16 }} title={project.path}>{project.path}</div>

        <div className="field">
          <label>Название</label>
          <input value={name} onChange={e => setName(e.target.value)} />
        </div>

        <label>Способы запуска</label>
        <div className="card" style={{ marginBottom: 14 }}>
          {launchers.map((l, i) => (
            <div className="launcher-edit" key={l.id || i}>
              <div className="row" style={{ marginBottom: 10 }}>
                <input value={l.title} onChange={e => patch(i, { title: e.target.value })}
                       placeholder="Название в списке" style={{ fontWeight: 500 }} />
                <span className="chip">{KIND_LABEL[l.kind] || l.kind}</span>
                <button className="ghost icon danger" title="Убрать способ запуска" disabled={launchers.length < 2}
                        onClick={() => setLaunchers(list => list.filter((_, n) => n !== i))}>
                  <Icon name="trash" size={15} />
                </button>
              </div>

              <div className="field">
                <label>{l.kind === 'script' ? 'Файл' : 'Команда'}</label>
                <input className="mn" style={{ fontSize: 12.5 }} value={l.command}
                       onChange={e => patch(i, { command: e.target.value })} />
              </div>
              <div className="grid-2">
                <div className="field">
                  <label>Рабочая папка</label>
                  <input className="mn" style={{ fontSize: 12.5 }} value={l.cwd}
                         onChange={e => patch(i, { cwd: e.target.value })} />
                </div>
                <div className="field">
                  <label>Адрес сервера</label>
                  <input className="mn" style={{ fontSize: 12.5 }} value={l.url || ''}
                         placeholder="определится сам по порту"
                         onChange={e => patch(i, { url: e.target.value })} />
                </div>
              </div>

              <div className="row" style={{ gap: 10 }}>
                <div className={'switch' + (l.autostart ? ' on' : '')}
                     onClick={() => patch(i, { autostart: !l.autostart })}>
                  <span />
                </div>
                <span style={{ fontSize: 13, color: 'var(--text-2)' }}>Запускать при старте приложения</span>
              </div>
            </div>
          ))}
        </div>

        {found === null ? (
          <button className="with-icon" onClick={rescan}>
            <Icon name="plus" size={15} />Добавить способ запуска
          </button>
        ) : (
          <>
            <label>Добавить ещё</label>
            <LauncherPicker
              root={project.path} candidates={found} picked={picked} onToggle={toggle} onManual={addManual} say={say}
              emptyText="Сверх уже добавленного сканер ничего не нашёл. Укажите файл запуска или команду вручную."
            />
          </>
        )}

        <div className="modal-foot">
          <button onClick={onClose} disabled={busy}>Отмена</button>
          <button className="primary" disabled={busy} onClick={save}>
            {busy ? 'Сохраняем…' : 'Сохранить'}
          </button>
        </div>
      </div>
    </div>
  )
}
