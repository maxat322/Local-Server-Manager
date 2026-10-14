import React from 'react'
import Icon from './Icon.jsx'
import RunButtons from './RunButtons.jsx'
import { IDLE, LED, fmtBytes, fmtPct, isActive, isAlive, shortUrl, statusText, urlOf } from '../constants.js'

/** Кнопка внутри строки: клик по ней не должен заодно открывать терминал, как клик по строке. */
const only = (fn) => (e) => {
  e.stopPropagation()
  fn()
}

/**
 * Репозиторий на главной. С одним способом запуска это одна строка; с несколькими —
 * заголовок и по строке на каждый способ, потому что у каждого свой статус и свой порт.
 */
export default function ProjectRow({ project, procs, foreign, usage, totalMem, now, busy, logId, act }) {
  const many = project.launchers.length > 1
  // Расход считается на репозиторий целиком: у нескольких способов запуска он общий.
  // Сервер, запущенный мимо менеджера, тоже ест ресурсы: его расход виден так же.
  const alive = project.launchers.some(l => isAlive(procs[l.id] || IDLE) || foreign[l.id])
  const meter = <Usage on={alive} usage={usage} totalMem={totalMem} />

  const tools = (
    <>
      <button className="ghost icon" title="Открыть папку" onClick={only(() => act.openFolder(project.path))}>
        <Icon name="folder" size={15} />
      </button>
      <button className="ghost icon" title="Изменить" onClick={only(() => act.edit(project))}>
        <Icon name="edit" size={15} />
      </button>
      <button className="ghost icon danger" title="Убрать из списка" onClick={only(() => act.remove(project))}>
        <Icon name="trash" size={15} />
      </button>
    </>
  )

  return (
    <div className="proj">
      {many && (
        <div className="svc head">
          <div className="col nm">
            <span className="title cut">{project.name}</span>
            <span className="cmd cut">{project.path}</span>
          </div>
          {/* Пустые ячейки держат цифры репозитория в той же колонке, что у одиночных строк. */}
          <span className="url none" />
          <span className="meta" />
          {meter}
          <div className="acts"><div className="run-space" /><div className="tools">{tools}</div></div>
        </div>
      )}
      {project.launchers.map(l => (
        <LauncherLine
          key={l.id} project={project} launcher={l} view={procs[l.id] || IDLE} now={now}
          elsewhere={foreign[l.id]}
          busy={busy.has(l.id)} selected={logId === l.id} sub={many} act={act}
          tools={many ? null : tools} meter={many ? <Usage on={false} /> : meter}
        />
      ))}
    </div>
  )
}

/**
 * Сколько репозиторий занимает прямо сейчас. Место под цифры есть в каждой строке, даже пустой:
 * иначе адреса и статусы соседних строк стояли бы вразнобой. Пока ядро не прислало первый замер,
 * стоят прочерки: ноль означал бы, что сервер ничего не ест.
 */
function Usage({ on, usage: u, totalMem }) {
  if (!on) return <span className="usage" />
  const dash = '—'
  const share = u && totalMem ? fmtPct((u.mem / totalMem) * 100) : dash
  return (
    <span className="usage">
      <span className="cpu" title="Процессор: доля всех ядер, как в диспетчере задач">
        <i>CPU</i>{u ? fmtPct(u.cpu) : dash}
      </span>
      <span className="ram" title="Оперативная память всех процессов: доля от установленной и объём">
        <i>RAM</i>{share} {u ? fmtBytes(u.mem) : ''}
      </span>
      <span className="gpu" title="Видеокарта: загрузка самого занятого движка и выделенная видеопамять">
        <i>GPU</i>{u ? fmtPct(u.gpu) : dash} {u ? fmtBytes(u.vram) : ''}
      </span>
    </span>
  )
}

function LauncherLine({ project, launcher, view, now, busy, selected, sub, act, tools, meter, elsewhere }) {
  // Запуск мимо менеджера интересен, только пока здесь сервер не работает: иначе это его же процессы.
  const ext = !isAlive(view) && elsewhere
  const active = isActive(view) || !!ext
  const status = ext ? { text: 'вне менеджера', tone: 'ext' } : statusText(view, now)
  const url = ext ? launcher.url || (ext.ports[0] ? 'http://localhost:' + ext.ports[0] : null) : urlOf(launcher, view)
  const note = ext ? ext.name + ' · PID ' + ext.root : view.note
  const noteTitle = ext
    ? 'Запущен не из менеджера. ' + (ext.runs > 1 ? 'Запусков: ' + ext.runs + ', процессов: ' : 'Процессов: ') +
      ext.pids.length + (ext.ports.length > 1 ? ', порты: ' + ext.ports.join(', ') : '') + '\n' + ext.command
    : note

  return (
    <div className={'svc' + (sub ? ' sub' : '') + (selected ? ' sel' : '')}
         onClick={() => act.showLogs(launcher.id)}>
      <span className={'led ' + (ext ? 'ext' : LED[view.status])} />
      <div className="col nm">
        <span className="title cut">{sub ? launcher.title : project.name}</span>
        {/* Сервер завершился сам: вместо команды показываем его последние слова, в них причина. */}
        {note
          ? <span className={'cmd cut ' + status.tone} title={noteTitle}>{note}</span>
          : !sub && <span className="cmd cut" title={project.path}>{launcher.title}</span>}
      </div>

      {active && url
        ? <a className="url cut" title={'Открыть ' + url} onClick={only(() => act.openUrl(url))}>{shortUrl(url)}</a>
        : <span className="url none">—</span>}
      <span className={'meta ' + status.tone}>{status.text}</span>
      {meter}

      <div className="acts">
        {/* Ширина блоков постоянная: иначе адрес и статус в соседних строках стоят вразнобой. */}
        <div className="run">
          <RunButtons
            view={view} busy={busy} external={!!ext}
            onStart={() => act.start(launcher.id)}
            onPause={() => act.pause(launcher.id)}
            onResume={() => act.resume(launcher.id)}
            onRestart={() => (ext ? act.restartForeign : act.restart)(launcher.id)}
            onStop={() => (ext ? act.stopForeign : act.stop)(launcher.id)}
          />
        </div>
        <button className="ghost icon" title="Терминал" onClick={only(() => act.showLogs(launcher.id))}>
          <Icon name="terminal" size={15} />
        </button>
        <div className="tools">{tools}</div>
      </div>
    </div>
  )
}
