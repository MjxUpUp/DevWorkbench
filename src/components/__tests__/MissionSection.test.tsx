import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';

vi.mock('../../utils/missionApi', () => ({
  missionApi: {
    init: vi.fn(),
    loadPrd: vi.fn(),
    apply: vi.fn(),
    status: vi.fn(),
  },
}));

import { missionApi } from '../../utils/missionApi';
import { MissionSection } from '../settings/MissionSection';

describe('MissionSection', () => {
  beforeEach(() => {
    vi.mocked(missionApi.init).mockReset();
    vi.mocked(missionApi.loadPrd).mockReset();
    vi.mocked(missionApi.apply).mockReset();
    vi.mocked(missionApi.status).mockReset();
  });

  it('disables all action buttons until a mission id is entered', () => {
    render(<MissionSection />);
    expect(screen.getByText('init').closest('button')).toBeDisabled();
    expect(screen.getByText('load PRD').closest('button')).toBeDisabled();
    expect(screen.getByText('apply').closest('button')).toBeDisabled();
  });

  it('init calls mission_init and shows the phase', async () => {
    vi.mocked(missionApi.init).mockResolvedValue({
      currentPhase: 'plan',
      iteration: 0,
      maxIterations: 20,
    });
    render(<MissionSection />);
    fireEvent.change(screen.getByPlaceholderText(/mission id/), {
      target: { value: 'm1' },
    });
    fireEvent.click(screen.getByText('init'));
    await waitFor(() => {
      expect(screen.getByText(/Phase 1 · 编写 PRD/)).toBeInTheDocument();
    });
    expect(missionApi.init).toHaveBeenCalledWith('m1');
  });

  it('apply is gated on a valid PRD load, then flips to executing', async () => {
    vi.mocked(missionApi.loadPrd).mockResolvedValue({
      valid: true,
      problems: [],
      prd: {},
      corrupted: false,
    });
    vi.mocked(missionApi.apply).mockResolvedValue({
      currentPhase: 'executing',
      iteration: 0,
      maxIterations: 20,
    });
    vi.mocked(missionApi.status).mockResolvedValue({
      state: { currentPhase: 'executing', iteration: 1, maxIterations: 20 },
      passed: 1,
      total: 5,
      corrupted: false,
    });
    render(<MissionSection />);
    fireEvent.change(screen.getByPlaceholderText(/mission id/), {
      target: { value: 'm2' },
    });
    // Before load, apply is disabled (no valid PRD).
    expect(screen.getByText('apply').closest('button')).toBeDisabled();
    // Load → PRD valid → apply enabled.
    fireEvent.click(screen.getByText('load PRD'));
    await waitFor(() => {
      expect(screen.getByText(/✅ 通过，可 apply/)).toBeInTheDocument();
    });
    expect(screen.getByText('apply').closest('button')).not.toBeDisabled();
    // Apply → executing + status refresh.
    fireEvent.click(screen.getByText('apply'));
    await waitFor(() => {
      expect(screen.getByText(/Phase 2 · 执行验收/)).toBeInTheDocument();
    });
    expect(missionApi.apply).toHaveBeenCalledWith('m2');
    expect(screen.getByText(/1\/5 stories/)).toBeInTheDocument();
  });

  it('shows validation problems on an invalid PRD', async () => {
    vi.mocked(missionApi.loadPrd).mockResolvedValue({
      valid: false,
      problems: ["Missing top-level 'userStories' array"],
      prd: null,
      corrupted: false,
    });
    render(<MissionSection />);
    fireEvent.change(screen.getByPlaceholderText(/mission id/), {
      target: { value: 'm3' },
    });
    fireEvent.click(screen.getByText('load PRD'));
    await waitFor(() => {
      expect(screen.getByText(/Missing top-level 'userStories' array/)).toBeInTheDocument();
    });
    expect(screen.getByText('apply').closest('button')).toBeDisabled();
  });
});
