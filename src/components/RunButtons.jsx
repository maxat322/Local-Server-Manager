import React from 'react'
import Icon from './Icon.jsx'
import { isAlive } from '../constants.js'

/**
 * Управление сервером: три кнопки на постоянных местах.
 *
 * Первая меняется по состоянию: «запустить» у остановленного, «пауза» у работающего,
 * «продолжить» у приостановленного. Отдельной кнопки запуска рядом с паузой не нужно:
 * запустить заново можно перезапуском.
 *
 * Сервер, запущенный мимо менеджера (`external`), запустить здесь нельзя: копия упёрлась бы в его
 * порт. Его можно остановить или перезапустить: тот запуск гасится, а сервер поднимается здесь.
 *
 * Пауза недоступна, пока сервер только запускается. Это и по смыслу верно (замораживать
 * недособранное незачем), и страхует от двойного клика по «запустить», второй щелчок
 * которого иначе попадал бы уже в паузу.
 */
export default function RunButtons({ view, busy, external, onStart, onPause, onResume, onRestart, onStop }) {
  const press = (fn) => (e) => {
    // В строке списка клик по кнопке не должен заодно открывать терминал.
    e.stopPropagation()
    fn()
  }
  const alive = isAlive(view) || external

  let first
  if (external) {
    first = (
      <button className="ibtn start" disabled title="Уже запущен не из менеджера. Перезапуск поднимет его здесь">
        <Icon name="play" size={14} />
      </button>
    )
  } else if (view.status === 'paused') {
    first = (
      <button className="ibtn start" title="Продолжить" disabled={busy} onClick={press(onResume)}>
        <Icon name="play" size={14} />
      </button>
    )
  } else if (alive) {
    first = (
      <button className="ibtn pause" disabled={busy || view.status !== 'running'} onClick={press(onPause)}
              title={view.status === 'running' ? 'Пауза: заморозить, не останавливая' : 'Пауза станет доступна, когда сервер поднимется'}>
        <Icon name="pause" size={14} />
      </button>
    )
  } else {
    first = (
      <button className="ibtn start" title="Запустить" disabled={busy} onClick={press(onStart)}>
        <Icon name="play" size={14} />
      </button>
    )
  }

  return (
    <>
      {first}
      <button className="ibtn restart" disabled={busy || !alive} onClick={press(onRestart)}
              title={external ? 'Перезапустить здесь: остановить тот запуск и поднять сервер в менеджере' : 'Перезапустить'}>
        <Icon name="refresh" size={14} />
      </button>
      <button className="ibtn stop" disabled={busy || !alive} onClick={press(onStop)}
              title={external ? 'Остановить запуск, сделанный не из менеджера' : 'Остановить'}>
        <Icon name="stop" size={14} />
      </button>
    </>
  )
}
