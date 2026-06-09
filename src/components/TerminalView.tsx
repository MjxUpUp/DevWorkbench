import { useRef, useEffect, useState } from 'react';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { WebLinksAddon } from '@xterm/addon-web-links';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import '@xterm/xterm/css/xterm.css';

interface TerminalViewProps {
  sessionId: string | null;
}

// xterm.js renders via canvas — CSS variables don't work.
// Group 7 app themes into 2 terminal theme buckets.
const TERMINAL_THEMES: Record<string, { background: string; foreground: string; cursor: string; selectionBackground: string }> = {
  dark: {
    background: '#1e1e2e',
    foreground: '#d4d4d4',
    cursor: '#e0e0e0',
    selectionBackground: '#444466',
  },
  light: {
    background: '#fafafa',
    foreground: '#1e1e1e',
    cursor: '#333333',
    selectionBackground: '#add6ff',
  },
};

const DARK_THEMES = new Set(['obsidian', 'midnight', 'ember', 'rose', 'nord']);

function getTerminalTheme(appTheme?: string): typeof TERMINAL_THEMES.dark {
  const bucket = DARK_THEMES.has(appTheme || 'obsidian') ? 'dark' : 'light';
  return TERMINAL_THEMES[bucket];
}

export function TerminalView({ sessionId }: TerminalViewProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitAddonRef = useRef<FitAddon | null>(null);
  const [hasOutput, setHasOutput] = useState(false);

  // Create terminal once
  useEffect(() => {
    if (!containerRef.current) return;

    const term = new Terminal({
      cursorBlink: true,
      convertEol: true,
      fontSize: 14,
      fontFamily: 'Cascadia Code, Fira Code, Consolas, monospace',
      theme: getTerminalTheme(document.documentElement.dataset.theme),
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

  // React to theme changes via MutationObserver on <html data-theme>
  // Single mechanism — no prop needed.
  useEffect(() => {
    const term = termRef.current;
    if (!term) return;

    const applyTheme = () => {
      const appTheme = document.documentElement.dataset.theme || 'obsidian';
      term.options.theme = getTerminalTheme(appTheme);
    };

    // Apply initial
    applyTheme();

    const observer = new MutationObserver(applyTheme);
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ['data-theme'],
    });

    return () => observer.disconnect();
  }, []);

  // Wire up session-specific event listeners
  useEffect(() => {
    const term = termRef.current;
    if (!term || !sessionId) {
      setHasOutput(false);
      return;
    }

    setHasOutput(false);
    term.clear();
    term.options.disableStdin = false;

    // Listen for PTY output
    const unlisten = listen<{ sessionId: string; data: number[] }>(
      'pty:output',
      (event) => {
        if (event.payload.sessionId === sessionId) {
          const bytes = new Uint8Array(event.payload.data);
          term.write(bytes);
          setHasOutput(true);
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
  }, [sessionId]);

  return (
    <div className="terminal-view">
      <div ref={containerRef} className="terminal-container" />
      {!sessionId && (
        <div className="terminal-overlay">
          <span className="terminal-placeholder">等待输出...</span>
        </div>
      )}
      {sessionId && !hasOutput && (
        <div className="terminal-overlay">
          <span className="terminal-placeholder">Agent 运行中，等待输出...</span>
        </div>
      )}
    </div>
  );
}
