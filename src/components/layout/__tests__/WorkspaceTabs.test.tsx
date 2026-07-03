import { describe, it, expect, beforeEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { WorkspaceTabs } from '../WorkspaceTabs';
import { useProjectStore } from '../../../stores/projectStore';
import { useNavigationStore } from '../../../stores/navigationStore';
import type { Project } from '../../../types';

const makeProject = (name: string, path: string): Project => ({
  id: path,
  name,
  description: '',
  path,
  tags: [],
  cover_image: null,
  open_count: 0,
  last_opened_at: null,
  starred: false,
  created_at: '',
  last_opened_tools: [],
  workspace_tools: [],
});

describe('WorkspaceTabs', () => {
  beforeEach(() => {
    useProjectStore.setState({
      projects: [makeProject('Alpha', '/a'), makeProject('Beta', '/b'), makeProject('Gamma', '/c')],
    });
    useNavigationStore.setState({
      activeProject: makeProject('Alpha', '/a'),
      selectedConversationId: 'conv-x',
      addProjectOpen: false,
    });
  });

  it('renders one tab per project plus the add button', () => {
    render(<WorkspaceTabs />);
    expect(screen.getAllByTestId('ws-tab')).toHaveLength(3);
    expect(screen.getByText('Alpha')).toBeInTheDocument();
    expect(screen.getByText('Beta')).toBeInTheDocument();
    expect(screen.getByText('Gamma')).toBeInTheDocument();
    expect(screen.getByTestId('ws-tab-add')).toBeInTheDocument();
  });

  it('marks only the active workspace tab', () => {
    render(<WorkspaceTabs />);
    const tabs = screen.getAllByTestId('ws-tab');
    expect(tabs[0]).toHaveClass('ws-tab-active');
    expect(tabs[1]).not.toHaveClass('ws-tab-active');
    expect(tabs[2]).not.toHaveClass('ws-tab-active');
  });

  it('switches the active workspace and clears the conversation scope on click', () => {
    render(<WorkspaceTabs />);
    fireEvent.click(screen.getByText('Beta'));
    expect(useNavigationStore.getState().activeProject?.path).toBe('/b');
    // 工作区切换重置会话作用域（selectProject 内清空 selectedConversationId）。
    expect(useNavigationStore.getState().selectedConversationId).toBeNull();
  });

  it('opens the AddProject modal on + click', () => {
    render(<WorkspaceTabs />);
    fireEvent.click(screen.getByTestId('ws-tab-add'));
    expect(useNavigationStore.getState().addProjectOpen).toBe(true);
  });
});
