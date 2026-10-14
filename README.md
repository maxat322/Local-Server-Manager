# Local Server Manager

**English** · [Русский](README.ru.md)

Start and control local development servers from the system tray. Add a repository folder,
the app finds what can start its server, and then you start, restart and stop servers from the
main screen or from the tray icon menu next to the clock.

```bash
npm install          # once
npm run tauri dev    # the app with hot reload of the interface
npm run tauri build  # installer in src-tauri/target/release/bundle/nsis
```

You need Node 20+, Rust (the `msvc` toolchain) and Visual Studio C++ Build Tools. WebView2 is
already present on Windows 11.

## How to use it

1. **Add** → choose a repository folder. The app shows the launch methods it found and marks the
   best one in advance.
2. If needed, select several (for example `web` and `api` separately), point to a launch file
   manually, or enter your own command.
3. The repository appears on the main screen with a status and three buttons. The first one changes
   with the state: start (green) when stopped, pause (blue) when running, resume (green) when paused.
   Then restart (amber) and stop (red). Clicking the row opens the server's terminal.

The number of running servers is visible without opening the window: the tray icon turns into that
number, and the app's taskbar button gets a green count badge. Servers on pause are not counted.
Numbers above nine are shown as "9+".

Closing the window moves the app to the tray; the servers keep running. A left click on the icon
opens the window, a right click opens the menu: each server with a "running" checkmark,
"Start all", "Stop all", "Exit".

## Started outside the manager

A server from the list can run without the manager: it was started from your own terminal, by
double-clicking a `.bat`, or it was left over from an earlier session. Such a run holds the port,
and a start from the manager then exits right away with "port in use".

The manager notices these runs. The server's row gets a violet ring indicator, the status
"вне менеджера" (outside the manager), the address of the port it holds, and the program with its
process ID (the tooltip shows the command line and all ports). The list header gets an
"ВНЕ МЕНЕДЖЕРА" counter.

- **Stop** ends that run entirely: the top process and all its children.
- **Restart** ends it, waits for the ports to be released, and starts the server in the manager,
  with the built-in terminal and buttons.
- **Start** is disabled: a second copy would hit the same port. "Start all" and autostart skip such
  servers.

A process counts as an outside run if it listens on a TCP port and the repository or launch folder
appears in its program path, command line or working directory. The port is required: editors and
language servers also run from the project folder, but they do not listen on ports. Editors,
terminals and browsers are never considered. A run is the chain of processes going up from the
server while their command line contains the same folder: so `cmd /c start-local.cmd` above `node`
is stopped together with it, while the terminal everything was started from stays open.

Detection runs every four seconds and only while the window is open. The resource usage of such a
server is shown on its line just like for one started from the manager.

## Resource usage

A repository that has something running shows how much it uses right now, on the same line as its
address and status, for example `CPU 3%  RAM 5% 1.6ГБ  GPU 0% 0МБ`:

| Figure | What it is |
|---|---|
| CPU | share of all the machine's cores, as in Task Manager |
| RAM | share of installed memory and the amount used by all the server's processes (working set) |
| GPU | graphics card load (the busiest engine: 3D, compute, video encoding) and dedicated video memory |

Percentages are rounded to whole numbers. In a narrow window the CPU, RAM and GPU labels move into
the tooltip; the order of the figures stays the same.

The whole process tree of a server is counted, and for a repository with several launch methods the
figures are the sum of them all. A server started outside the manager counts too: its processes are
measured one by one, so the line shows for it as well. They refresh every two seconds and only while the window is open:
in the tray the app takes no measurements.

On pause the CPU and GPU show zero and RAM drops to almost zero, but video memory stays in use:
pausing does not release it. The GPU counters come from Windows itself, so they work with a card
from any vendor; if the system has no such counters, zeros are shown.

## Built-in terminal

Each server — an npm script, `.cmd`, `.bat`, `.ps1` or a custom command — runs in a real Windows
pseudoconsole (ConPTY), just without a separate window. The panel at the bottom of the main screen
shows it the same way a console window would:

- the process sees a terminal, so tools draw colors and progress indicators;
- you can type into the terminal: answer a script's question, press a key on `pause`;
- Ctrl+C interrupts the server, as in a normal console. After that a batch file asks
  "Terminate batch job (Y/N)?" — this is `cmd`'s own question and needs an answer;
- Ctrl+C with text selected copies it, Ctrl+V pastes;
- output from previous runs stays in the scrollback, and each run is marked with a "▸ Start: …" line.

You can close the terminal: the server keeps running, and the screen is restored when you reopen it.

### Scripts that open their own window

If a batch file starts the server with `start "Title" cmd /k …`, Windows opens a new console window
for it, and this cannot be intercepted from outside: the file itself asks for it. The server stays
under the app's control (its port is visible, stopping works), but its output goes to that window.

The scanner detects such files. In the list of launch methods the file gets a warning, and right
above it the app offers the command that the file hides inside `start`: run directly, it goes to the
built-in terminal. `start /b` and `start` with a website address do not open a console window and
do not trigger the warning.

## Pause

A server can be frozen instead of stopped: all its processes stop receiving CPU time, and their
memory is pushed out to the page file. On resume the server continues from the same point without
rebuilding anything: it is the same process, not a new start.

Keep in mind:

- the port stays occupied while paused, and requests to the server are not refused — they wait for
  the resume;
- the file watcher is frozen together with the server and may miss edits made during the pause: then
  save the file again or restart the server;
- a long pause can cause connections to a database or external API to drop on the other side due to
  their timeouts;
- physical memory is released, but the space in the page file stays in use.

Pause is not available while the server is still starting. A paused server can be stopped and
restarted with the regular buttons.

## Ports

The second tab is a memory of ports: which port numbers have been used on this machine and by whom.
It helps you pick a port for a new project that will not clash with a running server, or with one
that usually runs but is currently off.

- Every port that any program listened on while the app was running is remembered: both servers
  started by the manager (with the project name) and anything started outside it (with the program name).
- A closed port stays in the list: you can see when it was last open.
- Search by number, program or project. For a numeric query the tab answers directly: "never used,
  free to take", "in use now", or "free, but last used by such-and-such project".
- Windows dynamic ports (from 49152 up) are skipped, because the system hands them out at random.

The memory is stored in `ports.json` next to the settings. You can remove an entry with "Forget".

## What counts as a launch method

| Source | What is found |
|---|---|
| `package.json` in the root | all scripts; `dev`, `start`, `serve`, `dev:*`, `start:*` rank highest |
| `package.json` in subfolders | the same for `web/`, `server/`, `frontend/` and packages in `apps/*`, `packages/*`, `services/*` |
| files in the root | `*.cmd`, `*.bat`, `*.ps1`; names with `start`, `run`, `dev`, `local` rank higher, with `stop`, `install`, `build` lower |
| manual | any file (`.cmd`, `.bat`, `.ps1`, `.exe`, `.js`, `.py`) or an arbitrary command |

The package manager is detected from the `packageManager` field, then from lock files
(`pnpm-lock.yaml` → `yarn.lock` → `bun.lockb` → `package-lock.json`). A package inside a monorepo
inherits the root's package manager. Scanning only reads files and never runs anything.

## How starting and stopping work

A dev server is almost never a single process: `start-local.cmd` starts `node`, which starts
`concurrently`, which starts several more `node` processes. If you kill the root `cmd.exe`, the
children keep running and hold their ports.

So each server runs inside a Windows **Job Object**. The process is created suspended, attached to
the job, and only then resumed, so no child is ever born outside it. "Stop" terminates the whole job:
the entire tree dies, including windows opened through `start`.

A few consequences worth knowing:

- **Servers live as long as the app does.** The job is created with `KILL_ON_JOB_CLOSE`: when the app
  exits, even if it crashes, the system stops all its servers. No orphaned `node.exe` processes are
  left behind. Closing the window does not touch the servers, and "Exit" asks for confirmation if
  anything is running.
- **The "Stop" button is forceful.** It kills the processes immediately without giving them a chance
  to clean up. If a server should shut down gracefully, press Ctrl+C in its terminal.
- **Containers survive a stop.** `docker compose up -d` hands the work to the Docker daemon, which is
  not part of the job. Stop such services with your own script.
- **A browser opened by a script may close together with the server.** If a script runs
  `start http://localhost:…` and the browser was not running before, its process ends up in the same
  job. If the browser was already open, this does not happen.
- **`pause` waits for a key.** A script that ends with `pause` stays in the "running" status until a
  key is pressed in its terminal — just as it would keep a console window open.

### Statuses

| Status | When |
|---|---|
| starting | the process is created, nothing is listening on a port yet |
| running | one of the server's processes is listening on a TCP port, or 10 seconds have passed (for processes without a port) |
| paused | processes were frozen with the pause button |
| stopped | stopped with the button, or interrupted with Ctrl+C in the terminal |
| exited | ended on its own with code 0: not an error, but the server is not running |
| crashed | ended on its own with a non-zero code; a Windows notification is shown |

When a process ends on its own, the server's name is replaced by a line of its output that looks like
the reason ("port in use", `EADDRINUSE`, `not found`); if there is none, the last line is shown.
If this happens within 20 seconds of starting, the server's terminal opens by itself.

A typical "exited" case: the server is already running somewhere else and holds the port. The second
copy sees the port is busy, reports it and exits with code 0. The manager only controls processes it
started itself, so the other copy has to be stopped where it is running.

A server counts as running as long as at least one process in its job is alive, even if the root
script has already exited: this is how scripts behave when they start the server with `start` and
then finish.

The address for the link button is taken from the output (the first `http://localhost:…`); if there is
none, from the listening port. You can also set it manually in the repository's settings.

## Project structure

```
src/                     interface: React + Vite, JSX
  App.jsx                layout, dialogs, actions
  ipc.js                 calls to the core and event subscriptions
  mock.js                core stub for UI work in a browser, not included in the build
  components/            screens and dialogs; TerminalPanel.jsx — the terminal on xterm.js
src-tauri/
  src/                   shell: window, tray, commands for the interface
    lib.rs  commands.rs  tray.rs
  core/                  the core, independent of Tauri
    src/config.rs        settings and the repository list
    src/scan.rs          finding launch methods
    src/job.rs           Job Object
    src/freeze.rs        pause: freezing the job's processes
    src/badge.rs         the running-server count as an image
    src/pty.rs           ConPTY pseudoconsole
    src/runner.rs        starting, statuses, terminal output
    src/usage.rs         CPU, memory and GPU usage
    src/foreign.rs       servers started outside the manager
    src/ports.rs         listening ports and program names
    src/portbook.rs      port memory
    src/text.rs          output parsing: server address, exit reason
```

The core lives in a separate crate on purpose: it is tested with real processes without building the
shell.

```bash
cargo test --manifest-path src-tauri/Cargo.toml -p lsm-core
```

The tests start `cmd`, `powershell` and `node` and check that stopping kills the whole tree, that the
port is released, that a process sees a real terminal, that `pause` waits for a key, and that typed
text reaches the script. `node` must be on `PATH`.

## Settings

They are stored in `%APPDATA%\com.localservermanager.app\config.json`: the repository list and the app
options. The file is written through a temporary file. If it cannot be read, it is not overwritten but
moved aside as `config.json.bad`.

Terminal output is kept in memory (5000 scrollback lines per server by default) and is not written to disk.

The `LSM_DATA_DIR` variable replaces the settings folder: handy for starting the app with an empty list
without touching your real one.

## Interface without the core

`npm run dev` opens the interface in a regular browser at `http://localhost:1430`. There is no core
there, so the stub `src/mock.js` with made-up servers is used: all screen states are visible, but
nothing is started.

## Not yet

- A custom stop command for a server (for example, for Docker containers).
- Several terminals on screen at once: one server's terminal is open at a time.
- Running `.sh` scripts and working outside Windows.
- Custom ordering of repositories: they are listed in the order they were added.
