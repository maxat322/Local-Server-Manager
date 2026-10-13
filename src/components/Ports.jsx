import React, { useCallback, useEffect, useMemo, useState } from 'react'
import Icon from './Icon.jsx'
import { api, errText } from '../ipc.js'
import { fmtWhen } from '../constants.js'

/** Как часто перечитывать таблицу, пока вкладка открыта. */
const REFRESH_MS = 5000

/**
 * Порты, которые уже были заняты на этой машине: открытые сейчас и те, что открывались раньше.
 * Вкладка отвечает на вопрос «какой порт дать новому проекту», поэтому помнит и выключенное:
 * порт сервера, который сегодня не запущен, свободным только кажется.
 */
export default function Ports({ say }) {
  const [rows, setRows] = useState(null)
  const [query, setQuery] = useState('')
  const [onlyOpen, setOnlyOpen] = useState(false)

  const load = useCallback(() => api.ports().then(setRows).catch(e => say(errText(e), true)), [say])
  useEffect(() => {
    load()
    const timer = setInterval(load, REFRESH_MS)
    return () => clearInterval(timer)
  }, [load])

  const q = query.trim().toLowerCase()
  const shown = useMemo(() => {
    if (!rows) return []
    return rows.filter(r => {
      if (onlyOpen && !r.open) return false
      if (!q) return true
      return String(r.port).includes(q) ||
        r.process.toLowerCase().includes(q) ||
        (r.owner || '').toLowerCase().includes(q)
    })
  }, [rows, q, onlyOpen])

  if (!rows) return <div className="dim">Читаем таблицу портов…</div>

  const forget = async (port) => {
    try { setRows(await api.forgetPort(port)) } catch (e) { say(errText(e), true) }
  }

  const open = rows.filter(r => r.open).length

  return (
    <>
      <div className="section-title">
        <span>ПОРТЫ · ЗАПОМНЕНО {rows.length} · ОТКРЫТО СЕЙЧАС {open}</span>
        <span className="row" style={{ marginLeft: 'auto', gap: 6 }}>
          <button className={'sm' + (onlyOpen ? '' : ' on')} onClick={() => setOnlyOpen(false)}>Все</button>
          <button className={'sm' + (onlyOpen ? ' on' : '')} onClick={() => setOnlyOpen(true)}>Открыты сейчас</button>
          <button className="ghost icon" title="Обновить" onClick={load}><Icon name="refresh" size={15} /></button>
        </span>
      </div>

      <div className="search">
        <Icon name="search" size={16} />
        <input
          value={query} onChange={e => setQuery(e.target.value)} autoFocus spellCheck={false}
          placeholder="Номер порта, программа или проект: 5173, node, SZ Learn"
        />
        {query && (
          <button className="ghost icon" title="Сбросить" onClick={() => setQuery('')}>
            <Icon name="close" size={14} />
          </button>
        )}
      </div>

      <Verdict query={q} rows={rows} />

      <div className="card">
        {shown.map(r => (
          <div className="prow" key={r.port}>
            <span className={'led calm' + (r.open ? ' ok' : '')} />
            <span className="num">{r.port}</span>
            <div className="col who">
              <span className="cut">{r.owner || r.process || 'программа не определена'}</span>
              {r.owner && r.process && <span className="sub cut">{r.process}</span>}
            </div>
            <span className={'state' + (r.open ? ' open' : '')}>
              {r.open ? 'открыт сейчас' : 'закрыт, был открыт ' + fmtWhen(r.lastSeen)}
            </span>
            <span className="since" title="Когда порт замечен впервые">с {fmtWhen(r.firstSeen)}</span>
            <button className="ghost icon danger" disabled={r.open}
                    title={r.open ? 'Порт открыт: забыть его нельзя' : 'Забыть порт'}
                    onClick={() => forget(r.port)}>
              <Icon name="trash" size={15} />
            </button>
          </div>
        ))}
        {!shown.length && (
          <div className="card-pad dim">
            {rows.length ? 'По этому запросу ничего не запомнено.' : 'Пока не замечено ни одного открытого порта.'}
          </div>
        )}
      </div>

      <div className="notebox" style={{ marginTop: 14 }}>
        <Icon name="info" size={16} style={{ flexShrink: 0, marginTop: 1 }} />
        <span>
          Запоминаются порты, которые слушала любая программа на этой машине, пока приложение было
          запущено, в том числе серверы, поднятые мимо него. Временные порты Windows (от 49152)
          пропускаются: система раздаёт их случайно, и проектам их не назначают.
        </span>
      </div>
    </>
  )
}

/** Прямой ответ на запрос-число: можно ли брать этот порт. */
function Verdict({ query, rows }) {
  if (!/^\d{2,5}$/.test(query)) return null
  const port = Number(query)
  if (port < 1 || port > 65535) return null
  const hit = rows.find(r => r.port === port)
  const who = hit && (hit.owner || hit.process || 'программа не определена')

  if (!hit) {
    return (
      <div className="notebox verdict free">
        <Icon name="check" size={16} style={{ flexShrink: 0, marginTop: 1 }} />
        <span>Порт {port} не встречался: его не слушают сейчас и не слушали раньше. Можно брать.</span>
      </div>
    )
  }
  return (
    <div className="warnbox verdict">
      <Icon name="warn" size={16} style={{ flexShrink: 0, marginTop: 1 }} />
      <span>
        {hit.open
          ? 'Порт ' + port + ' занят прямо сейчас: ' + who + '.'
          : 'Порт ' + port + ' сейчас свободен, но его занимал ' + who + ' (' + fmtWhen(hit.lastSeen) +
            '). Если это ваш проект, он займёт порт снова.'}
      </span>
    </div>
  )
}
