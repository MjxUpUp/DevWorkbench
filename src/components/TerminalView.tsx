import { useRef, useEffect } from 'react';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { WebLinksAddon } from '@xterm/addon-web-links';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import type { Session } from '../types';
import { useAgentStore } from '../stores/agentStore';
import '@xterm/xterm/css/xterm.css';

interface TerminalViewProps {
  sessionId: string | null;
  completedSession?: Session | null;
}

// xterm.js renders via canvas — CSS variables don't work.
// Detect system preference directly.
const TERMINAL_THEMES: Record<string, { background: string; foreground: string; cursor: string; selectionBackground: string }> = {
  dark: {
    background: '#12121A',
    foreground: '#E4E4EA',
    cursor: '#E4E4EA',
    selectionBackground: 'rgba(255,255,255,0.12)',
  },
  light: {
    background: '#F7F7F8',
    foreground: '#1A1A1E',
    cursor: '#1A1A1E',
    selectionBackground: 'rgba(0,0,0,0.1)',
  },
};

function getTerminalTheme(): typeof TERMINAL_THEMES.dark {
  const isDark = window.matchMedia('(prefers-color-scheme: dark)').matches;
  return TERMINAL_THEMES[isDark ? 'dark' : 'light'];
}

export function TerminalView({ sessionId, completedSession }: TerminalViewProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitAddonRef = useRef<FitAddon | null>(null);
  const hasOutputRef = useRef(false);

  // Create terminal once
  useEffect(() => {
    if (!containerRef.current) return;

    const term = new Terminal({
      cursorBlink: true,
      convertEol: true,
      fontSize: 14,
      fontFamily: 'Cascadia Code, Fira Code, Consolas, monospace',
      theme: getTerminalTheme(),
      scrollback: 5000,
      disableStdin: true,
    });

    const fitAddon = new FitAddon();
    term.loadAddon(fitAddon);
    term.loadAddon(new WebLinksAddon());
    term.open(containerRef.current);

    try { fitAddon.fit(); } catch { /* container not visible yet */ }

    termRef.current = term;
    fitAddonRef.current = fitAddon;

    // ResizeObserver to keep terminal fitted
    let resizeObserver: ResizeObserver | null = null;
    if (containerRef.current) {
      resizeObserver = new ResizeObserver(() => {
        try { fitAddon.fit(); } catch { /* ignore */ }
      });
      resizeObserver.observe(containerRef.current);
    }

    return () => {
      resizeObserver?.disconnect();
      term.dispose();
      termRef.current = null;
      fitAddonRef.current = null;
    };
  }, []);

  // React to system color scheme changes
  useEffect(() => {
    const term = termRef.current;
    if (!term) return;

    const applyTheme = () => {
      term.options.theme = getTerminalTheme();
    };

    applyTheme();

    const mql = window.matchMedia('(prefers-color-scheme: dark)');
    mql.addEventListener('change', applyTheme);
    return () => mql.removeEventListener('change', applyTheme);
  }, []);

  // Wire up session-specific event listeners
  useEffect(() => {
    const term = termRef.current;
    if (!term) {
      return;
    }

    // If we have a completed session, write its summary directly
    if (completedSession) {
      term.clear();
      term.options.disableStdin = true;

      // Header
      term.writeln(`\x1b[1m── Session: ${completedSession.prompt.slice(0, 80)} ──\x1b[0m`);
      term.writeln(`\x1b[90mAgent: ${completedSession.agentType}  Status: ${completedSession.status}  ` +
        `Exit: ${completedSession.exitCode ?? 'N/A'}\x1b[0m`);
      term.writeln('');

      // Output summary
      if (completedSession.outputSummary) {
        const lines = completedSession.outputSummary.split('\n');
        for (const line of lines) {
          term.writeln(line);
        }
      } else {
        term.writeln('\x1b[90m(无输出记录)\x1b[0m');
      }

      // Context snapshot
      if (completedSession.contextSnapshot?.filesChanged?.length) {
        term.writeln('');
        term.writeln('\x1b[1m── Files Changed ──\x1b[0m');
        for (const f of completedSession.contextSnapshot.filesChanged) {
          term.writeln(`  \x1b[33m${f}\x1b[0m`);
        }
      }

      term.writeln('');
      term.writeln('\x1b[90m— 会话结束 —\x1b[0m');
      if (completedSession.status === 'completed' || completedSession.status === 'failed') {
        term.writeln('');
        term.writeln('\x1b[36m↩ 在下方输入框中输入指令继续此对话\x1b[0m');
      }
      return;
    }

    if (!sessionId) {
      return;
    }

    hasOutputRef.current = false;
    term.clear();
    term.options.disableStdin = false;

    // Replay cached PTY output from store (survives tab switches)
    const cachedChunks = useAgentStore.getState().ptyOutput.get(sessionId);
    if (cachedChunks && cachedChunks.length > 0) {
      hasOutputRef.current = true;
      for (const chunk of cachedChunks) {
        term.write(chunk);
      }
    } else {
      // Show waiting message only if no cached output
      term.writeln('\x1b[90m⏳ Agent 运行中，等待输出...\x1b[0m');
    }

    // Listen for PTY output (new chunks after replay)
    const unlisten = listen<{ sessionId: string; data: number[] }>(
      'pty:output',
      (event) => {
        if (event.payload.sessionId === sessionId) {
          // Clear the waiting message on first real output
          if (!hasOutputRef.current) {
            term.clear();
            hasOutputRef.current = true;
          }
          const bytes = new Uint8Array(event.payload.data);
          term.write(bytes);
        }
      }
    );

    // Listen for agent completion
    const unlistenExit = listen<{ sessionId: string }>(
      'agent:completed',
      (event) => {
        if (event.payload.sessionId === sessionId) {
          term.write('\r\n\x1b[90m— 会话结束 —\x1b[0m\r\n');
          term.options.disableStdin = true;
        }
      }
    );

    // Forward user input to PTY
    const dataDisposable = term.onData((data) => {
      invoke('pty_write_cmd', { sessionId, data }).catch(() => {});
    });

    // Forward resize to PTY
    const resizeDisposable = term.onResize(({ cols, rows }) => {
      invoke('pty_resize_cmd', { sessionId, cols, rows }).catch(() => {});
    });

    return () => {
      unlisten.then(fn => fn());
      unlistenExit.then(fn => fn());
      dataDisposable.dispose();
      resizeDisposable.dispose();
    };
  }, [sessionId, completedSession?.id, completedSession?.outputSummary]);

  return (
    <div className="terminal-view">
      <div ref={containerRef} className="terminal-container" />
    </div>
  );
}
