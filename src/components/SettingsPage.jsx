import React, { useEffect, useState } from 'react'
import Icon from './Icon.jsx'
import { api, errText } from '../ipc.js'
import { ACCENTS } from '../constants.js'

const TABS = [
  ['behavior', 'Поведение'],
  ['look', 'Внешний вид'],
  ['about', 'О приложении'],
]

export default function SettingsPage({ settings, info, say, onSaved, onOpenFolder, onClose }) {
  const [tab, setTab] = useState('behavior')

  useEffect(() => {
    const esc = (e) => e.key === 'Escape' && onClose()
    window.addEventListener('keydown', esc)
    return () => window.removeEventListener('keydown', esc)
  }, [onClose])

  // Каждая настройка сохраняется сразу: отдельной кнопки «Сохранить» у переключателей нет.
  const save = async (patch) => {
    try { onSaved(await api.saveSettings({ ...settings, ...patch })) }
    catch (e) { say(errText(e), true) }
  }

  return (
    <div className="overlay" onClick={onClose}>
      <div className="modal wide" onClick={e => e.stopPropagation()}>
        <div className="row" style={{ marginBottom: 16 }}>
          <h2 className="grow">Настройки</h2>
          <button className="ghost icon" onClick={onClose}><Icon name="close" size={16} /></button>
        </div>

        <div className="tabs">
          {TABS.map(([id, label]) => (
            <div key={id} className={'tab' + (tab === id ? ' on' : '')} onClick={() => setTab(id)}>{label}</div>
          ))}
        </div>

        {tab === 'behavior' && <BehaviorTab settings={settings} save={save} />}
        {tab === 'look' && <LookTab settings={settings} save={save} />}
        {tab === 'about' && <AboutTab info={info} onOpenFolder={onOpenFolder} />}
      </div>
    </div>
  )
}

// ─────────────────────────── поведение ───────────────────────────

function BehaviorTab({ settings, save }) {
  const [lines, setLines] = useState(String(settings.logLines))
  useEffect(() => setLines(String(settings.logLines)), [settings.logLines])

  const saveLines = () => {
    const n = Number(lines)
    if (Number.isFinite(n) && n !== settings.logLines) save({ logLines: Math.round(n) })
    else setLines(String(settings.logLines))
  }

  return (
    <>
      <div className="section-title">ЗАПУСК И ОКНО</div>
      <div className="card" style={{ marginBottom: 18 }}>
        <Toggle
          icon="power" title="Запускать вместе с Windows"
          text="Приложение стартует при входе в систему и поднимает серверы, отмеченные для автозапуска."
          on={settings.launchWithWindows} onChange={v => save({ launchWithWindows: v })}
        />
        <Toggle
          icon="tray" title="При автозапуске стартовать в трее"
          text="Окно не открывается само: приложение появляется только значком возле часов."
          on={settings.startHidden} locked={!settings.launchWithWindows}
          onChange={v => save({ startHidden: v })}
        />
        <Toggle
          icon="window" title="Закрытие окна сворачивает в трей"
          text="Серверы продолжают работать. Если выключить, крестик завершает приложение и все серверы."
          on={settings.closeToTray} onChange={v => save({ closeToTray: v })}
        />
        <Toggle
          icon="bell" title="Уведомлять, если сервер упал"
          text="Уведомление Windows, когда процесс завершился с ошибкой сам, а не по кнопке «Остановить»."
          on={settings.notifyOnCrash} onChange={v => save({ notifyOnCrash: v })}
        />
      </div>

      <div className="section-title">ТЕРМИНАЛ</div>
      <div className="field" style={{ maxWidth: 260 }}>
        <label>Строк прокрутки на сервер</label>
        <input type="number" min="200" max="50000" step="100" value={lines}
               onChange={e => setLines(e.target.value)} onBlur={saveLines}
               onKeyDown={e => e.key === 'Enter' && e.target.blur()} />
      </div>
      <div className="notebox">
        <Icon name="info" size={16} style={{ flexShrink: 0, marginTop: 1 }} />
        <span>
          Вывод терминала хранится в памяти и на диск не пишется: после выхода из приложения
          он пропадает. Старые строки вытесняются новыми.
        </span>
      </div>
    </>
  )
}

function Toggle({ icon, title, text, on, locked, onChange }) {
  return (
    <div className={'mod' + (on && !locked ? ' on' : '') + (locked ? ' off' : '')}>
      <div className="ic"><Icon name={icon} size={17} /></div>
      <div className="col grow" style={{ gap: 3 }}>
        <span style={{ fontSize: 14, fontWeight: 500 }}>{title}</span>
        <span style={{ fontSize: 12, color: 'var(--text-3)', lineHeight: 1.5 }}>{text}</span>
      </div>
      <div className={'switch' + (on ? ' on' : '') + (locked ? ' locked' : '')}
           title={locked ? 'Сначала включите запуск вместе с Windows' : ''}
           onClick={() => !locked && onChange(!on)}>
        <span />
      </div>
    </div>
  )
}

// ─────────────────────────── внешний вид ───────────────────────────

function LookTab({ settings, save }) {
  return (
    <>
      <div className="field">
        <label>Цвет акцента</label>
        <div className="swatches">
          {Object.entries(ACCENTS).map(([id, a]) => (
            <div key={id} className={'swatch' + (settings.accent === id ? ' on' : '')}
                 style={{ background: a.main }} title={a.label} onClick={() => save({ accent: id })} />
          ))}
        </div>
      </div>
      <div className="notebox">
        <Icon name="info" size={16} style={{ flexShrink: 0, marginTop: 1 }} />
        <span>Меняется только акцент: тёмная основа одна, как в Mega Panel и Mega Agent.</span>
      </div>
    </>
  )
}

// ─────────────────────────── о приложении ───────────────────────────

function AboutTab({ info, onOpenFolder }) {
  return (
    <div style={{ fontSize: 13, lineHeight: 1.7, color: 'var(--text-2)' }}>
      <p style={{ marginTop: 0 }}>
        Local Server Manager {info ? 'версии ' + info.version : ''} запускает локальные серверы
        разработки и держит их под рукой в трее.
      </p>
      <p>
        Серверы живут, пока работает приложение. Закрытие окна их не трогает, а «Выход» в меню трея
        останавливает все разом. Остановка завершает всё дерево процессов сервера, поэтому занятых
        портов и осиротевших node.exe после неё не остаётся.
      </p>
      <p style={{ marginBottom: 6 }}>Настройки и список репозиториев лежат здесь:</p>
      <div className="row">
        <div className="term inline grow cut">{info?.dataDir || '—'}</div>
        <button className="with-icon" disabled={!info} onClick={() => onOpenFolder(info.dataDir)}>
          <Icon name="folder" size={15} />Открыть
        </button>
      </div>
    </div>
  )
}
