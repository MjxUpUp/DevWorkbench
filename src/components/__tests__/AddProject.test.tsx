import { describe, it, expect, vi } from 'vitest';
import { render, fireEvent } from '@testing-library/react';

// AddProject pulls Tauri invoke + the native dialog plugin at module load.
// Mock both so jsdom never hits the native bridge. This file only exercises
// the modal's a11y shell (role + ESC dismiss) — the add/scan flows chain
// real IPC and stay covered by E2E.
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }));

import { AddProject } from '../AddProject';

describe('AddProject modal a11y', () => {
  it('renders as an aria-modal dialog and closes on ESC', () => {
    const onClose = vi.fn();
    render(
      // onAdd's full Project-omitting signature isn't relevant to the a11y
      // shell under test; cast the stub rather than reconstruct the type.
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      <AddProject onAdd={vi.fn() as any} onClose={onClose} existingProjects={[]} />,
    );

    // role=dialog + aria-modal per the WAI-ARIA dialog pattern.
    const dialog = document.querySelector('[role="dialog"]');
    expect(dialog).not.toBeNull();
    expect(dialog?.getAttribute('aria-modal')).toBe('true');

    // ESC dismisses — the overlay click is mouse-only, so ESC is the only
    // keyboard path to close. This was the real gap before the refactor.
    fireEvent.keyDown(window, { key: 'Escape' });
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
