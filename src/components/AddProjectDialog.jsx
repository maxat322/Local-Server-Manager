import React, { useEffect, useState } from 'react'
import LauncherPicker, { toLauncher } from './LauncherPicker.jsx'
import { candidateKey } from '../constants.js'

/**
 * Добавление репозитория: папка уже выбрана и просканирована, осталось отметить, чем запускать.
 * Отмечен заранее только лучший кандидат: у проекта с десятком dev-скриптов отметить все
 * значило бы поднять одно и то же несколько раз.
 */
export default function AddProjectDialog({ scan, busy, say, onAdd, onClose }) {
  const [name, setName] = useState(scan.name)
  const [extra, setExtra] = useState([])
  const [picked, setPicked] = useState(() => {
    const best = scan.candidates[0]
    return new Set(best && best.recommended ? [candidateKey(best)] : [])
  })

  useEffect(() => {
    const esc = (e) => e.key === 'Escape' && onClose()
    window.addEventListener('keydown', esc)
    return () => window.removeEventListener('keydown', esc)
  }, [onClose])

  const all = [...scan.candidates, ...extra]

  const toggle = (key) => setPicked(prev => {
    const next = new Set(prev)
    if (!next.delete(key)) next.add(key)
    return next
  })

  const addManual = (c) => {
    const key = candidateKey(c)
    if (!all.some(x => candidateKey(x) === key)) setExtra(list => [...list, c])
    setPicked(prev => new Set(prev).add(key))
  }

  const chosen = all.filter(c => picked.has(candidateKey(c)))

  return (
    <div className="overlay" onClick={onClose}>
      <div className="modal wide" onClick={e => e.stopPropagation()}>
        <h2>Новый репозиторий</h2>
        <div className="sub mn cut" style={{ fontSize: 12, marginBottom: 16 }} title={scan.path}>{scan.path}</div>

        <div className="field">
          <label>Название</label>
          <input value={name} onChange={e => setName(e.target.value)} autoFocus />
        </div>

        <label>Чем запускать</label>
        <LauncherPicker
          root={scan.path} candidates={all} picked={picked} onToggle={toggle} onManual={addManual} say={say}
        />

        <div className="modal-foot">
          <button onClick={onClose} disabled={busy}>Отмена</button>
          <button className="primary" disabled={busy || !chosen.length}
                  onClick={() => onAdd({ id: '', name, path: scan.path, launchers: chosen.map(toLauncher) })}>
            {busy ? 'Добавляем…' : 'Добавить'}
          </button>
        </div>
      </div>
    </div>
  )
}
