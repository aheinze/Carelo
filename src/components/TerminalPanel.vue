<script setup>
import { computed, markRaw, nextTick, onMounted, onUnmounted, ref, watch } from 'vue';
import { listen } from '@tauri-apps/api/event';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { SearchAddon } from '@xterm/addon-search';
import { WebLinksAddon } from '@xterm/addon-web-links';
import { Unicode11Addon } from '@xterm/addon-unicode11';
import '@xterm/xterm/css/xterm.css';
import AppIcon from './AppIcon.vue';
import {
  closeTerminalSession,
  openExternalUrl,
  resizeTerminalSession,
  startTerminalSession,
  terminalSessionCwd,
  writeTerminalSession,
} from '../composables/useFileOperations';
import { useFileManagerStore } from '../stores/fileManagerStore';
import { archiveParentPath, isArchivePath } from '../utils/archivePaths';

const props = defineProps({
  visible: {
    type: Boolean,
    required: true,
  },
});

const store = useFileManagerStore();
const TERMINAL_SCROLLBACK = 5000;
const sessions = ref([]);
const activeSessionId = ref(null);
const terminalHost = ref(null);
const searchVisible = ref(false);
const searchTerm = ref('');
const searchInput = ref(null);
const terminalMenu = ref(null);
const SEARCH_DECORATIONS = {
  decorations: {
    matchOverviewRuler: '#d29922',
    activeMatchColorOverviewRuler: '#f0a500',
    matchBackground: 'rgba(210, 153, 34, 0.32)',
    activeMatchBackground: 'rgba(240, 165, 0, 0.55)',
  },
};
let unlistenOutput = null;
let unlistenExit = null;
let resizeObserver = null;
let resizeFrame = 0;
let colorSchemeMediaQuery = null;
let colorSchemeListener = null;
let cwdPollTimer = null;
const CWD_POLL_INTERVAL_MS = 1200;

const activeSession = computed(() =>
  sessions.value.find((session) => session.id === activeSessionId.value) || sessions.value[0] || null,
);
const activeCwd = computed(() => {
  if (!store.appSettings.terminalStartsInActiveFolder) {
    return undefined;
  }

  const directory = store.effectiveDirectoryFor(store.activePaneId) || store.activePane?.currentPath || '';
  return isArchivePath(directory) ? archiveParentPath(directory) : directory || undefined;
});

function titleForShell(shell) {
  const parts = String(shell || 'shell').split('/');
  return parts.at(-1) || 'shell';
}

// Label tabs by the folder the terminal was opened in, falling back to the
// shell name when there's no usable directory.
function titleForCwd(cwd, shell) {
  const value = String(cwd || '').trim();

  if (!value) {
    return titleForShell(shell);
  }

  const clean = value.replace(/\/+$/, '');

  if (!clean) {
    return '/';
  }

  return clean.split('/').filter(Boolean).at(-1) || '/';
}

// Poll each live session's shell cwd so the tab follows `cd` inside the
// terminal (Linux reads /proc/<pid>/cwd; other platforms report nothing).
async function refreshSessionCwds() {
  const liveSessions = sessions.value.filter(
    (session) => !session.exited && typeof session.id === 'number',
  );

  for (const session of liveSessions) {
    try {
      const cwd = await terminalSessionCwd(session.id);

      if (cwd && cwd !== session.cwd) {
        session.cwd = cwd;
        session.title = titleForCwd(cwd, session.shell);
      }
    } catch {
      // Transient failures (e.g. session closing) are ignored.
    }
  }
}

function startCwdPolling() {
  if (cwdPollTimer) {
    return;
  }

  cwdPollTimer = window.setInterval(() => {
    if (props.visible) {
      refreshSessionCwds();
    }
  }, CWD_POLL_INTERVAL_MS);
}

function stopCwdPolling() {
  if (cwdPollTimer) {
    window.clearInterval(cwdPollTimer);
    cwdPollTimer = null;
  }
}

function errorMessage(error, fallback) {
  return error?.message || error?.toString?.() || fallback;
}

function readTerminalTheme() {
  const s = getComputedStyle(document.documentElement);
  const v = (name) => s.getPropertyValue(name).trim();
  return {
    background:                 v('--term-bg'),
    foreground:                 v('--term-fg'),
    cursor:                     v('--term-cursor'),
    cursorAccent:               v('--term-bg'),
    selectionBackground:        v('--term-selection'),
    selectionInactiveBackground:v('--term-selection-ia'),
    black:                      v('--term-black'),
    red:                        v('--term-red'),
    green:                      v('--term-green'),
    yellow:                     v('--term-yellow'),
    blue:                       v('--term-blue'),
    magenta:                    v('--term-magenta'),
    cyan:                       v('--term-cyan'),
    white:                      v('--term-white'),
    brightBlack:                v('--term-br-black'),
    brightRed:                  v('--term-br-red'),
    brightGreen:                v('--term-br-green'),
    brightYellow:               v('--term-br-yellow'),
    brightBlue:                 v('--term-br-blue'),
    brightMagenta:              v('--term-br-magenta'),
    brightCyan:                 v('--term-br-cyan'),
    brightWhite:                v('--term-br-white'),
  };
}

function applyTerminalTheme(session) {
  if (!session) {
    return;
  }

  session.terminal.options.theme = readTerminalTheme();
}

function applyTerminalThemes() {
  sessions.value.forEach(applyTerminalTheme);
}

function createTerminal() {
  // markRaw: xterm manages its own internal state and breaks if Vue wraps it
  // in a reactive proxy (selection/clear and other stateful methods misbehave).
  const terminal = markRaw(new Terminal({
    // SearchAddon match highlighting uses the decoration API, which xterm 6
    // gates behind allowProposedApi (throws otherwise).
    allowProposedApi: true,
    allowTransparency: true,
    convertEol: false,
    cursorBlink: true,
    cursorStyle: 'bar',
    fontFamily: '"SF Mono", ui-monospace, Menlo, Consolas, monospace',
    fontSize: 12.5,
    fontWeight: 500,
    lineHeight: 1.22,
    macOptionIsMeta: true,
    rightClickSelectsWord: false,
    scrollback: TERMINAL_SCROLLBACK,
    tabStopWidth: 2,
    theme: readTerminalTheme(),
  }));
  const fitAddon = markRaw(new FitAddon());
  const searchAddon = markRaw(new SearchAddon());

  terminal.loadAddon(fitAddon);
  terminal.loadAddon(searchAddon);
  terminal.loadAddon(new WebLinksAddon((event, uri) => {
    openExternalUrl(uri).catch(() => {});
  }));
  terminal.loadAddon(new Unicode11Addon());
  terminal.unicode.activeVersion = '11';

  return { terminal, fitAddon, searchAddon };
}

function createSessionRecord(info) {
  const { terminal, fitAddon, searchAddon } = createTerminal();
  const element = markRaw(document.createElement('div'));
  const disposables = [];
  const session = {
    id: info.sessionId,
    title: titleForCwd(info.cwd, info.shell),
    shell: info.shell,
    cwd: info.cwd,
    terminal,
    fitAddon,
    searchAddon,
    element,
    disposables,
    exited: false,
    opened: false,
  };

  element.className = 'terminal-session-host';
  terminal.attachCustomKeyEventHandler((event) => terminalKeyHandler(event, session));

  if (typeof info.sessionId === 'number') {
    disposables.push(
      terminal.onData((data) => {
        writeTerminalSession(info.sessionId, data).catch((error) => {
          terminal.write(`\r\n${errorMessage(error, 'Unable to write to terminal.')}\r\n`);
        });
      }),
    );
    disposables.push(
      terminal.onResize(({ cols, rows }) => {
        resizeTerminalSession(info.sessionId, rows, cols).catch(() => {});
      }),
    );
  }

  return session;
}

function createErrorSession(cwd, error) {
  const session = createSessionRecord({
    sessionId: `error-${Date.now()}`,
    shell: 'Terminal',
    cwd: cwd || '',
  });

  session.exited = true;
  session.terminal.write(`${errorMessage(error, 'Unable to start terminal session.')}\r\n`);
  return session;
}

async function createSession(cwd = activeCwd.value) {
  let session;

  try {
    const info = await startTerminalSession(cwd);
    session = createSessionRecord(info);
  } catch (error) {
    session = createErrorSession(cwd, error);
  }

  sessions.value.push(session);
  activeSessionId.value = session.id;
  await nextTick();
  attachActiveSession();
}

function disposeSession(session) {
  session.disposables.forEach((disposable) => disposable.dispose?.());
  session.terminal.dispose();

  if (typeof session.id === 'number') {
    closeTerminalSession(session.id).catch(() => {});
  }
}

async function closeSession(sessionId) {
  const index = sessions.value.findIndex((session) => session.id === sessionId);

  if (index < 0) {
    return;
  }

  const [session] = sessions.value.splice(index, 1);
  disposeSession(session);

  if (activeSessionId.value === sessionId) {
    activeSessionId.value = sessions.value[Math.min(index, sessions.value.length - 1)]?.id || null;
  }

  await nextTick();
  attachActiveSession();
}

function closeAllSessions() {
  const all = [...sessions.value];

  if (all.length === 0) {
    return;
  }

  sessions.value = [];
  activeSessionId.value = null;
  all.forEach(disposeSession);
  nextTick(attachActiveSession);
}

function focusTerminal() {
  activeSession.value?.terminal.focus();
}

function copyFromSession(session) {
  const text = session?.terminal?.getSelection();

  if (text) {
    navigator.clipboard?.writeText(text).catch(() => {});
  }
}

async function pasteIntoSession(session) {
  if (!session || typeof session.id !== 'number') {
    return;
  }

  try {
    const text = await navigator.clipboard?.readText();

    if (text) {
      writeTerminalSession(session.id, text).catch(() => {});
    }
  } catch {
    // Clipboard read can be denied; nothing to do.
  }
}

// Intercept terminal keystrokes for copy/paste/find; everything else is sent
// to the shell. Returning false tells xterm not to forward the key.
function terminalKeyHandler(event, session) {
  const mod = event.metaKey || event.ctrlKey;

  // Let the global handler toggle the terminal panel.
  if (mod && event.key === '`') {
    return true;
  }

  if (event.type === 'keydown' && mod) {
    const key = event.key.toLowerCase();

    if (event.shiftKey && key === 'c') {
      event.preventDefault();
      copyFromSession(session);
      return false;
    }

    if (event.shiftKey && key === 'v') {
      event.preventDefault();
      pasteIntoSession(session);
      return false;
    }

    if (!event.shiftKey && key === 'f') {
      event.preventDefault();
      openSearch();
      return false;
    }
  }

  event.stopPropagation();
  return true;
}

function openSearch() {
  searchVisible.value = true;
  nextTick(() => searchInput.value?.focus?.());
}

function closeSearch() {
  searchVisible.value = false;
  activeSession.value?.searchAddon?.clearDecorations?.();
  focusTerminal();
}

function runSearch(forward = true, incremental = false) {
  const term = searchTerm.value;
  const addon = activeSession.value?.searchAddon;

  if (!term || !addon) {
    return;
  }

  // `incremental` keeps the current match while typing instead of jumping ahead.
  const options = incremental ? { ...SEARCH_DECORATIONS, incremental: true } : SEARCH_DECORATIONS;

  try {
    if (forward) {
      addon.findNext(term, options);
    } else {
      addon.findPrevious(term, options);
    }
  } catch {
    // Never let a search hiccup break the input handler.
  }
}

function openTerminalMenu(event) {
  if (!activeSession.value) {
    return;
  }

  terminalMenu.value = { x: event.clientX, y: event.clientY };
}

function runTerminalMenu(action) {
  const session = activeSession.value;
  terminalMenu.value = null;

  if (!session) {
    return;
  }

  switch (action) {
    case 'copy':
      copyFromSession(session);
      break;
    case 'paste':
      pasteIntoSession(session);
      break;
    case 'selectAll':
      session.terminal.selectAll();
      break;
    case 'clear':
      session.terminal.clear();
      break;
    default:
      break;
  }
}

function appendOutput(sessionId, data) {
  const session = sessions.value.find((item) => item.id === sessionId);

  if (session) {
    session.terminal.write(String(data));
  }
}

function markSessionExited(sessionId) {
  const session = sessions.value.find((item) => item.id === sessionId);

  if (!session) {
    return;
  }

  session.exited = true;
  session.terminal.write('\r\n[process exited]\r\n');
}

function attachActiveSession() {
  const host = terminalHost.value;
  const session = activeSession.value;

  if (!host) {
    return;
  }

  host.replaceChildren();

  if (!session) {
    return;
  }

  host.appendChild(session.element);
  applyTerminalTheme(session);

  if (!session.opened) {
    session.terminal.open(session.element);
    session.opened = true;
  }

  scheduleFitActiveSession();
  focusTerminal();
}

function scheduleFitActiveSession() {
  if (!props.visible || !activeSession.value) {
    return;
  }

  if (resizeFrame) {
    window.cancelAnimationFrame(resizeFrame);
  }

  resizeFrame = window.requestAnimationFrame(() => {
    resizeFrame = 0;
    const session = activeSession.value;

    if (!session?.opened) {
      return;
    }

    try {
      session.fitAddon.fit();
    } catch {
      return;
    }

    if (typeof session.id === 'number') {
      resizeTerminalSession(session.id, session.terminal.rows, session.terminal.cols).catch(() => {});
    }
  });
}

function handleTerminalKeydown(event) {
  if ((event.metaKey || event.ctrlKey) && event.key === '`') {
    return;
  }

  event.stopPropagation();
}

function panelHeightLimit() {
  return Math.min(560, Math.max(220, window.innerHeight - 160));
}

function startPanelResize(event) {
  event.preventDefault();

  const startY = event.clientY;
  const startHeight = store.terminalPanelHeight;
  const previousCursor = document.body.style.cursor;

  document.body.style.cursor = 'ns-resize';

  function handlePointerMove(moveEvent) {
    const nextHeight = startHeight + startY - moveEvent.clientY;
    store.setTerminalPanelHeight(Math.min(panelHeightLimit(), Math.max(180, nextHeight)));
    scheduleFitActiveSession();
  }

  function handlePointerUp() {
    document.body.style.cursor = previousCursor;
    window.removeEventListener('pointermove', handlePointerMove);
    window.removeEventListener('pointerup', handlePointerUp);
    scheduleFitActiveSession();
  }

  window.addEventListener('pointermove', handlePointerMove);
  window.addEventListener('pointerup', handlePointerUp, { once: true });
}

watch(
  () => props.visible,
  async (visible) => {
    if (visible && sessions.value.length === 0) {
      await createSession();
    } else if (visible) {
      await nextTick();
      attachActiveSession();
      refreshSessionCwds();
    }
  },
);

watch(activeSessionId, () => {
  nextTick(attachActiveSession);
  terminalMenu.value = null;
});

watch(searchTerm, () => runSearch(true, true));

watch(
  () => store.appSettings.appearanceMode,
  () => {
    window.requestAnimationFrame(() => applyTerminalThemes());
  },
);

onMounted(async () => {
  unlistenOutput = await listen('terminal://output', (event) => {
    appendOutput(event.payload.sessionId, event.payload.data);
  });
  unlistenExit = await listen('terminal://exit', (event) => {
    markSessionExited(event.payload.sessionId);
  });

  if (typeof ResizeObserver !== 'undefined') {
    resizeObserver = new ResizeObserver(scheduleFitActiveSession);

    if (terminalHost.value) {
      resizeObserver.observe(terminalHost.value);
    }
  }

  if (typeof window !== 'undefined' && typeof window.matchMedia === 'function') {
    colorSchemeMediaQuery = window.matchMedia('(prefers-color-scheme: light)');
    colorSchemeListener = () => {
      applyTerminalThemes();
      scheduleFitActiveSession();
    };
    if (colorSchemeMediaQuery.addEventListener) {
      colorSchemeMediaQuery.addEventListener('change', colorSchemeListener);
    } else {
      colorSchemeMediaQuery.addListener?.(colorSchemeListener);
    }
  }

  if (props.visible && sessions.value.length === 0) {
    await createSession();
  }

  startCwdPolling();
});

onUnmounted(() => {
  stopCwdPolling();
  unlistenOutput?.();
  unlistenExit?.();
  resizeObserver?.disconnect();
  if (colorSchemeMediaQuery?.removeEventListener) {
    colorSchemeMediaQuery.removeEventListener('change', colorSchemeListener);
  } else {
    colorSchemeMediaQuery?.removeListener?.(colorSchemeListener);
  }

  if (resizeFrame) {
    window.cancelAnimationFrame(resizeFrame);
  }

  sessions.value.forEach(disposeSession);
});
</script>

<template>
  <section class="terminal-panel" :class="{ 'terminal-panel--hidden': !visible }" aria-label="Terminal panel">
    <div class="terminal-resize-handle" aria-hidden="true" @pointerdown="startPanelResize"></div>

    <header class="terminal-header">
      <div class="terminal-tabs" role="tablist" aria-label="Terminal sessions">
        <div
          v-for="session in sessions"
          :key="session.id"
          class="terminal-tab"
          :class="{ 'terminal-tab--active': session.id === activeSessionId }"
        >
          <button
            v-tooltip="session.cwd || 'No active directory'"
            type="button"
            class="terminal-tab-main"
            role="tab"
            :title="session.cwd || 'No active directory'"
            @click="activeSessionId = session.id"
          >
            <AppIcon name="terminal" :size="14" />
            <span>{{ session.title }}</span>
            <small v-if="session.exited">exited</small>
          </button>
          <button type="button" class="terminal-tab-close" aria-label="Close terminal" @click.stop="closeSession(session.id)">
            <AppIcon name="x" :size="11" :stroke-width="2.2" />
          </button>
        </div>
      </div>

      <div class="terminal-actions">
        <span class="terminal-cwd" :title="activeSession?.cwd">{{ activeSession?.cwd || 'No terminal' }}</span>
        <button type="button" class="terminal-action" aria-label="New terminal" @click="createSession()">
          <AppIcon name="plus" :size="15" />
        </button>
        <button type="button" class="terminal-action" aria-label="Close all terminals" @click="closeAllSessions">
          <AppIcon name="trash" :size="15" />
        </button>
        <button type="button" class="terminal-action" aria-label="Hide terminal" @click="store.toggleTerminalPanel(false)">
          <AppIcon name="chevron-down" :size="15" />
        </button>
      </div>
    </header>

    <div class="terminal-body" @keydown="handleTerminalKeydown">
      <div v-if="!activeSession" class="terminal-empty">
        <AppIcon name="terminal" :size="24" />
        <span>No terminal session</span>
        <button type="button" @click="createSession()">New Terminal</button>
      </div>
      <div
        v-show="activeSession"
        ref="terminalHost"
        class="terminal-host"
        @click="focusTerminal"
        @contextmenu.prevent="openTerminalMenu"
      ></div>

      <div v-if="searchVisible" class="terminal-search" @keydown.stop>
        <AppIcon name="search" :size="13" :stroke-width="1.9" />
        <input
          ref="searchInput"
          v-model="searchTerm"
          type="text"
          spellcheck="false"
          placeholder="Find in terminal…"
          aria-label="Find in terminal"
          @keydown.enter.prevent="runSearch(!$event.shiftKey)"
          @keydown.escape.prevent="closeSearch"
        />
        <button type="button" class="terminal-search-btn" aria-label="Previous match" @click="runSearch(false)">
          <AppIcon name="chevron-left" :size="15" :stroke-width="2" />
        </button>
        <button type="button" class="terminal-search-btn" aria-label="Next match" @click="runSearch(true)">
          <AppIcon name="chevron-right" :size="15" :stroke-width="2" />
        </button>
        <button type="button" class="terminal-search-btn" aria-label="Close search" @click="closeSearch">
          <AppIcon name="x" :size="13" :stroke-width="2.2" />
        </button>
      </div>
    </div>

    <div
      v-if="terminalMenu"
      class="terminal-menu-backdrop"
      @click="terminalMenu = null"
      @contextmenu.prevent="terminalMenu = null"
    >
      <div class="terminal-menu" :style="{ left: `${terminalMenu.x}px`, top: `${terminalMenu.y}px` }" @click.stop>
        <button type="button" @click="runTerminalMenu('copy')">Copy</button>
        <button type="button" @click="runTerminalMenu('paste')">Paste</button>
        <button type="button" @click="runTerminalMenu('selectAll')">Select All</button>
        <button type="button" @click="runTerminalMenu('clear')">Clear</button>
      </div>
    </div>
  </section>
</template>

<style scoped>
.terminal-panel {
  position: relative;
  display: grid;
  min-height: 0;
  grid-template-rows: 34px minmax(0, 1fr);
  border-top: 1px solid var(--separator);
  background: var(--term-bg);
  box-shadow: inset 0 1px 0 var(--hairline);
  overflow: hidden;
}

.terminal-resize-handle {
  position: absolute;
  z-index: 2;
  top: -3px;
  right: 0;
  left: 0;
  height: 7px;
  cursor: ns-resize;
}

.terminal-resize-handle::after {
  position: absolute;
  top: 3px;
  left: 50%;
  width: 42px;
  height: 2px;
  border-radius: 999px;
  background: color-mix(in srgb, var(--text) 18%, transparent);
  content: "";
  transform: translateX(-50%);
}

.terminal-resize-handle:hover::after {
  background: color-mix(in srgb, var(--text) 32%, transparent);
}

.terminal-panel--hidden {
  display: none;
}

.terminal-header {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  align-items: center;
  gap: 12px;
  min-width: 0;
  border-bottom: 1px solid var(--separator);
  padding: 4px 10px;
  background: var(--term-header-bg);
  box-shadow: inset 0 1px 0 var(--hairline);
}

.terminal-tabs,
.terminal-actions {
  display: flex;
  min-width: 0;
  align-items: center;
}

.terminal-tabs {
  gap: 4px;
  overflow: auto hidden;
  scrollbar-width: none;
}

.terminal-tabs::-webkit-scrollbar {
  display: none;
}

.terminal-tab {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 16px;
  width: 156px;
  height: 25px;
  flex: 0 0 auto;
  align-items: center;
  gap: 3px;
  border-radius: 7px;
  padding: 0 5px 0 0;
  background: transparent;
  color: var(--text-faint);
  text-align: left;
}

.terminal-tab:hover {
  background: var(--btn-hover);
  color: var(--text-muted);
}

.terminal-tab--active {
  background: var(--btn-active-bg);
  color: var(--text);
  box-shadow: var(--btn-active-shadow);
}

.terminal-tab-main {
  display: grid;
  grid-template-columns: 16px minmax(0, 1fr) auto;
  min-width: 0;
  height: 100%;
  align-items: center;
  gap: 5px;
  border-radius: 7px;
  padding: 0 0 0 8px;
  background: transparent;
  color: inherit;
  text-align: left;
}

.terminal-tab span,
.terminal-cwd {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.terminal-tab span {
  font-size: 12px;
  font-weight: 650;
}

.terminal-tab small {
  color: var(--text-faint);
  font-size: 9.5px;
  font-weight: 700;
  text-transform: uppercase;
}

.terminal-tab-close {
  display: grid;
  width: 16px;
  height: 16px;
  place-items: center;
  border-radius: 50%;
  background: transparent;
  color: inherit;
  opacity: 0;
}

.terminal-tab:hover .terminal-tab-close,
.terminal-tab--active .terminal-tab-close {
  opacity: 1;
}

.terminal-tab-close:hover {
  background: var(--btn-hover);
}

.terminal-actions {
  gap: 5px;
}

.terminal-cwd {
  max-width: 340px;
  color: var(--text-muted);
  font-size: 11px;
  font-weight: 560;
}

.terminal-action {
  display: grid;
  width: 25px;
  height: 25px;
  place-items: center;
  border-radius: 7px;
  background: transparent;
  color: var(--icon);
}

.terminal-action:hover {
  background: var(--btn-hover);
  color: var(--text);
}

.terminal-body {
  position: relative;
  min-width: 0;
  min-height: 0;
  background: var(--term-surface);
}

.terminal-host {
  width: 100%;
  height: 100%;
  min-height: 0;
  overflow: hidden;
  padding: 8px 10px 9px;
  background: var(--term-bg);
}

.terminal-empty {
  display: grid;
  height: 100%;
  place-content: center;
  gap: 8px;
  justify-items: center;
  color: var(--text-faint);
  font-size: 12px;
  font-weight: 600;
}

.terminal-empty button {
  height: 26px;
  border: 1px solid var(--control-border);
  border-radius: 7px;
  padding: 0 10px;
  background: var(--control-bg);
  color: var(--text);
  font-size: 12px;
  font-weight: 650;
}

.terminal-host :deep(.terminal-session-host),
.terminal-host :deep(.xterm) {
  width: 100%;
  height: 100%;
}

.terminal-host :deep(.xterm) {
  overflow: hidden;
  border-radius: 7px;
  background: var(--term-bg);
  color: var(--term-fg);
  outline: none;
  box-shadow:
    inset 0 1px 0 var(--hairline),
    inset 0 0 0 1px var(--separator);
}

.terminal-host :deep(.xterm-viewport) {
  border-radius: 7px;
  background-color: var(--term-bg) !important;
  scrollbar-color: color-mix(in srgb, var(--term-fg) 22%, transparent) transparent;
}

.terminal-host :deep(.xterm-viewport::-webkit-scrollbar) {
  width: 8px;
}

.terminal-host :deep(.xterm-viewport::-webkit-scrollbar-track) {
  background: transparent;
}

.terminal-host :deep(.xterm-viewport::-webkit-scrollbar-thumb) {
  border: 2px solid var(--term-bg);
  border-radius: 999px;
  background: color-mix(in srgb, var(--term-fg) 18%, transparent);
}

.terminal-host :deep(.xterm-viewport::-webkit-scrollbar-thumb:hover) {
  background: color-mix(in srgb, var(--term-fg) 32%, transparent);
}

.terminal-host :deep(.xterm-screen) {
  padding: 0;
  background: var(--term-bg);
}

.terminal-host :deep(.xterm-screen canvas) {
  background: var(--term-bg);
}

.terminal-host :deep(.xterm-rows) {
  color: var(--term-fg);
}

.terminal-host :deep(.xterm .composition-view) {
  border: 1px solid var(--control-border);
  border-radius: 6px;
  background: var(--term-header-bg);
  color: var(--term-fg);
  box-shadow: var(--shadow-overlay);
}

.terminal-host :deep(.xterm-accessibility-tree) {
  background: var(--term-bg);
  color: var(--term-fg);
  font-family: "SF Mono", ui-monospace, Menlo, Consolas, monospace;
}

.terminal-host :deep(.xterm-decoration-overview-ruler) {
  background: transparent;
}

.terminal-host :deep(.xterm .xterm-helpers) {
  opacity: 0;
}

/* ── Find-in-terminal bar ─────────────────────────────────── */
.terminal-search {
  position: absolute;
  top: 8px;
  right: 14px;
  z-index: 6;
  display: flex;
  align-items: center;
  gap: 6px;
  height: 30px;
  padding: 0 6px 0 10px;
  border: 1px solid var(--control-border);
  border-radius: 8px;
  background: var(--popover-bg);
  box-shadow: var(--shadow-overlay);
  color: var(--text-faint);
}

.terminal-search input {
  width: 180px;
  min-width: 0;
  border: 0;
  outline: 0;
  background: transparent;
  color: var(--text);
  font-size: 12px;
}

.terminal-search input::placeholder {
  color: var(--text-faint);
}

.terminal-search-btn {
  display: grid;
  width: 24px;
  height: 24px;
  place-items: center;
  border-radius: 6px;
  background: transparent;
  color: var(--icon);
  transition: background 90ms ease, color 90ms ease;
}

.terminal-search-btn:hover {
  background: var(--btn-hover);
  color: var(--text);
}

/* ── Right-click menu ─────────────────────────────────────── */
.terminal-menu-backdrop {
  position: fixed;
  inset: 0;
  z-index: 2500;
}

.terminal-menu {
  position: fixed;
  min-width: 150px;
  padding: 5px;
  border: 1px solid var(--control-border);
  border-radius: var(--radius-panel);
  background: var(--popover-bg);
  box-shadow: var(--shadow-overlay);
}

.terminal-menu button {
  display: block;
  width: 100%;
  padding: 7px 10px;
  border-radius: 7px;
  background: transparent;
  color: var(--text);
  font-size: 13px;
  text-align: left;
  cursor: pointer;
}

.terminal-menu button:hover {
  background: var(--btn-hover);
}
</style>
