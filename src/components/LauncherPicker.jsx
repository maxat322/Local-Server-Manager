import React, { useState } from 'react'
import Icon from './Icon.jsx'
import { api, errText } from '../ipc.js'
import { candidateKey } from '../constants.js'

/**
 * Выбор способов запуска из найденных сканером плюс ручной ввод.
 * Общий для добавления репозитория и для его правки.
 *
 * Скрипты вроде build и lint сканер тоже находит, но показывает их свёрнутыми:
 * серверами они не являются, а список из тридцати npm-скриптов прячет нужный.
 */
const NOTHING_FOUND =
  'В папке не нашлось ни package.json со скриптами, ни файлов .cmd, .bat или .ps1. ' +
  'Укажите файл запуска или команду вручную.'

export default function LauncherPicker({ root, candidates, picked, onToggle, onManual, say, emptyText = NOTHING_FOUND }) {
  const main = candidates.filter(c => c.recommended || c.manual)
  const other = candidates.filter(c => !c.recommended && !c.manual)
  const [showOther, setShowOther] = useState(false)
  const [command, setCommand] = useState('')

  // Если рекомендовать нечего, прятать остальное бессмысленно: это всё, что есть.
  const otherOpen = showOther || !main.length

  const pickFile = async () => {
    try {
      const file = await api.pickFile(root)
      if (!file) return
      const name = file.split(/[\\/]/).pop()
      const inside = file.toLowerCase().startsWith(root.toLowerCase())
      onManual({
        kind: 'script', title: name, command: file, source: 'файл', detail: file, manual: true,
        // Скрипт из самого репозитория запускаем из его корня, чужой — из его собственной папки.
        cwd: inside ? root : file.slice(0, file.length - name.length - 1),
      })
    } catch (e) { say(errText(e), true) }
  }

  const addCommand = () => {
    const text = command.trim()
    if (!text) return
    onManual({ kind: 'command', title: text, command: text, cwd: root, source: 'команда', detail: '', manual: true })
    setCommand('')
  }

  const row = (c) => {
    const key = candidateKey(c)
    return <Row key={key} c={c} on={picked.has(key)} onToggle={() => onToggle(key)} />
  }

  return (
    <>
      {!!main.length && <div className="card" style={{ marginBottom: 10 }}>{main.map(row)}</div>}

      {!candidates.length && (
        <div className="notebox" style={{ marginBottom: 10 }}>
          <Icon name="info" size={16} style={{ flexShrink: 0, marginTop: 1 }} />
          <span>{emptyText}</span>
        </div>
      )}

      {!!other.length && (
        <>
          {!!main.length && (
            <div className={'fold' + (otherOpen ? ' open' : '')} style={{ margin: '4px 0 10px' }}
                 onClick={() => setShowOther(v => !v)}>
              <Icon name="chevron" size={13} />
              Другие скрипты и файлы · {other.length}
            </div>
          )}
          {otherOpen && <div className="card" style={{ marginBottom: 10 }}>{other.map(row)}</div>}
        </>
      )}

      <div className="row" style={{ gap: 8, marginTop: 4 }}>
        <button className="with-icon" onClick={pickFile}>
          <Icon name="file" size={15} />Указать файл…
        </button>
        <input
          className="mn" style={{ fontSize: 12.5 }} value={command}
          placeholder="или своя команда: node server.js --port 3000"
          onChange={e => setCommand(e.target.value)}
          onKeyDown={e => e.key === 'Enter' && addCommand()}
        />
        <button disabled={!command.trim()} onClick={addCommand}>Добавить</button>
      </div>
    </>
  )
}

function Row({ c, on, onToggle }) {
  return (
    <div className="cand" onClick={onToggle}>
      <span className={'check' + (on ? ' on' : '')}><Icon name="check" size={12} /></span>
      <div className="col grow" style={{ gap: 2 }}>
        <span className="what cut">{c.title}</span>
        {c.detail && <span className="body cut" title={c.detail}>{c.detail}</span>}
        {c.warning && <span className="body warn">{c.warning}</span>}
      </div>
      <span className="chip">{c.source}</span>
    </div>
  )
}

/** Кандидат сканера → запись способа запуска. Идентификатор выдаст ядро. */
export const toLauncher = (c) => ({
  id: '', title: c.title, kind: c.kind, command: c.command, cwd: c.cwd, url: null, autostart: false,
})
