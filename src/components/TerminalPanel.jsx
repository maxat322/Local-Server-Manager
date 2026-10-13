import React, { useEffect, useRef, useState } from 'react'
import { Terminal } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import '@xterm/xterm/css/xterm.css'
import Icon from './Icon.jsx'
import RunButtons from './RunButtons.jsx'
import { api, errText, on } from '../ipc.js'
import { ACCENTS, DEFAULT_ACCENT, LED, isActive, shortUrl, statusText, urlOf } from '../constants.js'

/**
 * Палитра терминала под тёмную подложку. «Чёрный» и «синий» из классических шестнадцати цветов
 * на почти чёрном фоне не читаются, поэтому они подняты. Голубой отдан акценту приложения:
 * им инструменты обычно красят адреса, и им же написаны собственные отметки «Запуск», «Остановлен».
 */
function theme(accentName) {
  const accent = ACCENTS[accentName] || ACCENTS[DEFAULT_ACCENT]
  return {
    background: '#070909', foreground: '#a8b4b6',
    cursor: accent.main, cursorAccent: '#070909', selectionBackground: `rgba(${accent.rgb}, .28)`,
    black: '#5c6a6d', red: '#e06c6c', green: '#5fd38d', yellow: '#e0a33e',
    blue: '#6ea8e0', magenta: '#c58af0', cyan: accent.main, white: '#d8e0e2',
    brightBlack: '#7f8f92', brightRed: '#f08a8a', brightGreen: '#86efac', brightYellow: '#f0c674',
    brightBlue: '#9cc4f0', brightMagenta: '#dcb0f8', brightCyan: accent.hi, brightWhite: '#ffffff',
  }
}

/**
 * Терминал выбранного сервера. Процесс запущен в настоящей псевдоконсоли, так что это не журнал,
 * а полноценное окно консоли: с цветами и индикаторами прогресса, с вводом (ответить на вопрос
 * скрипта, нажать клавишу на `pause`) и с Ctrl+C.
 */
export default function TerminalPanel({ project, launcher, view, busy, scrollback, accent, act, say, onClose }) {
  const id = launcher.id
  const host = useRef(null)
  const term = useRef(null)
  const [tall, setTall] = useState(false)

  useEffect(() => {
    let dead = false
    const t = new Terminal({
      fontFamily: "'JetBrains Mono', Consolas, monospace",
      fontSize: 12.5,
      lineHeight: 1.25,
      scrollback,
      cursorBlink: true,
      theme: theme(accent),
    })
    const fit = new FitAddon()
    t.loadAddon(fit)
    t.open(host.current)
    term.current = t

    // Размер окна терминала сообщаем ядру: процесс должен знать ширину, иначе переносы
    // строк и индикаторы прогресса рисуются под чужую.
    const sync = () => {
      if (dead || !host.current?.clientWidth) return
      try { fit.fit() } catch { return }
      api.termResize(t.cols, t.rows).catch(() => {})
    }
    sync()
    // Шрифт мог ещё не загрузиться: с запасным ширина знака другая.
    document.fonts.ready.then(sync)
    let timer = null
    const observer = new ResizeObserver(() => {
      clearTimeout(timer)
      timer = setTimeout(sync, 60)
    })
    observer.observe(host.current)

    // Подписываемся раньше, чем приходит сохранённый вывод, и копим свежее в очереди:
    // иначе новые куски легли бы на экран раньше истории. Повторы отсекаются по номеру.
    let last = 0
    let ready = false
    let queue = []
    const write = (chunks) => {
      for (const c of chunks) {
        if (c.n > last) {
          t.write(c.s)
          last = c.n
        }
      }
    }
    const off = on('term', batches => {
      for (const b of batches) {
        if (b.id !== id) continue
        if (ready) write(b.chunks)
        else queue.push(...b.chunks)
      }
    })
    api.output(id)
      .then(chunks => {
        if (dead) return
        write(chunks)
        write(queue)
        queue = []
        ready = true
      })
      .catch(e => say(errText(e), true))

    const typed = t.onData(data => api.termInput(id, data).catch(() => {}))

    t.attachCustomKeyEventHandler(e => {
      if (e.type !== 'keydown' || !e.ctrlKey || e.altKey) return true
      const key = e.key.toLowerCase()
      // Ctrl+C с выделением копирует, без выделения прерывает процесс, как в Windows Terminal.
      if (key === 'c' && t.hasSelection()) {
        navigator.clipboard.writeText(t.getSelection()).catch(() => {})
        t.clearSelection()
        return false
      }
      // Вставку отдаём браузеру: он сам передаст текст терминалу.
      if (key === 'v') return false
      return true
    })
    t.focus()

    return () => {
      dead = true
      clearTimeout(timer)
      observer.disconnect()
      typed.dispose()
      off()
      t.dispose()
      term.current = null
    }
  }, [id, scrollback])

  useEffect(() => {
    if (term.current) term.current.options.theme = theme(accent)
  }, [accent])

  const active = isActive(view)
  const status = statusText(view, Date.now())
  const url = urlOf(launcher, view)
  const name = project.launchers.length > 1 ? project.name + ' · ' + launcher.title : project.name

  const clear = async () => {
    try {
      await api.clearOutput(id)
      term.current?.clear()
    } catch (e) { say(errText(e), true) }
  }

  const copy = async () => {
    const t = term.current
    if (!t) return
    try {
      t.selectAll()
      const text = t.getSelection()
      t.clearSelection()
      await navigator.clipboard.writeText(text.replace(/\s+$/, '') + '\n')
      say('Вывод терминала скопирован')
    } catch (e) { say('Не удалось скопировать: ' + errText(e), true) }
  }

  return (
    <div className={'logpane' + (tall ? ' tall' : '')}>
      <div className="bar">
        <span className={'led ' + LED[view.status]} />
        <span className="name cut">{name}</span>
        <span className="mn" style={{ fontSize: 11, color: 'var(--text-3)' }}>{status.text}</span>
        {active && url && (
          <a className="mn cut" style={{ fontSize: 12 }} onClick={() => act.openUrl(url)}>{shortUrl(url)}</a>
        )}

        <span className="row" style={{ marginLeft: 'auto', gap: 6 }}>
          <RunButtons
            view={view} busy={busy}
            onStart={() => act.start(id)} onPause={() => act.pause(id)} onResume={() => act.resume(id)}
            onRestart={() => act.restart(id)} onStop={() => act.stop(id)}
          />
          <span className="sep" />
          <button className="ghost icon" title="Скопировать весь вывод" onClick={copy}>
            <Icon name="copy" size={15} />
          </button>
          <button className="ghost icon" title="Очистить терминал" onClick={clear}>
            <Icon name="trash" size={15} />
          </button>
          <button className="ghost icon" title={tall ? 'Свернуть' : 'Развернуть'} onClick={() => setTall(v => !v)}>
            <Icon name={tall ? 'collapse' : 'expand'} size={15} />
          </button>
          <button className="ghost icon" title="Закрыть терминал" onClick={onClose}>
            <Icon name="close" size={15} />
          </button>
        </span>
      </div>

      <div className="termhost" ref={host} />
    </div>
  )
}
