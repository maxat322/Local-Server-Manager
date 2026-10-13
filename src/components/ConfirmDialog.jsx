import React, { useEffect } from 'react'
import Icon from './Icon.jsx'

/**
 * Диалог подтверждения. Показывает не «вы уверены?», а последствия: что произойдёт и с чем.
 * Нативный confirm() не годится: во встроенном окне он молча возвращает false.
 */
export default function ConfirmDialog({ title, consequences, confirmLabel, danger, busy, onConfirm, onClose }) {
  useEffect(() => {
    const esc = (e) => e.key === 'Escape' && onClose()
    window.addEventListener('keydown', esc)
    return () => window.removeEventListener('keydown', esc)
  }, [onClose])

  return (
    <div className="overlay" onClick={onClose}>
      <div className="modal" onClick={e => e.stopPropagation()}>
        <div style={{ display: 'flex', gap: 14 }}>
          <div style={{
            width: 38, height: 38, flexShrink: 0, borderRadius: 10, background: '#12100c',
            border: '1px solid #3a3226', display: 'flex', alignItems: 'center', justifyContent: 'center',
            color: 'var(--warn)',
          }}>
            <Icon name="warn" size={20} />
          </div>
          <div className="col" style={{ gap: 8 }}>
            <h2>{title}</h2>
            <div className="sub">{consequences}</div>
          </div>
        </div>

        <div className="modal-foot">
          <button onClick={onClose} disabled={busy}>Отмена</button>
          <button className={danger ? 'primary danger' : 'primary'} onClick={onConfirm} disabled={busy}>
            {busy ? 'Выполняется…' : confirmLabel}
          </button>
        </div>
      </div>
    </div>
  )
}
