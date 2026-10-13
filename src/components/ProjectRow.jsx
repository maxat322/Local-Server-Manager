import React from 'react'
import Icon from './Icon.jsx'
import RunButtons from './RunButtons.jsx'
import { IDLE, LED, isActive, shortUrl, statusText, urlOf } from '../constants.js'

/** Кнопка внутри строки: клик по ней не должен заодно открывать терминал, как клик по строке. */
const only = (fn) => (e) => {
  e.stopPropagation()
  fn()
}

/**
 * Репозиторий на главной. С одним способом запуска это одна строка; с несколькими —
 * заголовок и по строке на каждый способ, потому что у каждого свой статус и свой порт.
 */
export default function ProjectRow({ project, procs, now, busy, logId, act }) {
  const many = project.launchers.length > 1

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
          <div className="acts"><div className="tools">{tools}</div></div>
        </div>
      )}
      {project.launchers.map(l => (
        <LauncherLine
          key={l.id} project={project} launcher={l} view={procs[l.id] || IDLE} now={now}
          busy={busy.has(l.id)} selected={logId === l.id} sub={many} act={act}
          tools={many ? null : tools}
        />
      ))}
    </div>
  )
}

function LauncherLine({ project, launcher, view, now, busy, selected, sub, act, tools }) {
  const active = isActive(view)
  const status = statusText(view, now)
  const url = urlOf(launcher, view)

  return (
    <div className={'svc' + (sub ? ' sub' : '') + (selected ? ' sel' : '')}
         onClick={() => act.showLogs(launcher.id)}>
      <span className={'led ' + LED[view.status]} />
      <div className="col nm">
        <span className="title cut">{sub ? launcher.title : project.name}</span>
        {/* Сервер завершился сам: вместо команды показываем его последние слова, в них причина. */}
        {view.note
          ? <span className={'cmd cut ' + status.tone} title={view.note}>{view.note}</span>
          : !sub && <span className="cmd cut" title={project.path}>{launcher.title}</span>}
      </div>

      {active && url
        ? <a className="url cut" title={'Открыть ' + url} onClick={only(() => act.openUrl(url))}>{shortUrl(url)}</a>
        : <span className="url none">—</span>}
      <span className={'meta ' + status.tone}>{status.text}</span>

      <div className="acts">
        {/* Ширина блоков постоянная: иначе адрес и статус в соседних строках стоят вразнобой. */}
        <div className="run">
          <RunButtons
            view={view} busy={busy}
            onStart={() => act.start(launcher.id)}
            onPause={() => act.pause(launcher.id)}
            onResume={() => act.resume(launcher.id)}
            onRestart={() => act.restart(launcher.id)}
            onStop={() => act.stop(launcher.id)}
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
