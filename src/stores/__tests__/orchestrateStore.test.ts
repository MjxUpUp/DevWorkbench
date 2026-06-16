import { describe, it, expect, beforeEach, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { parseNodeIds, useOrchestrateStore, SAMPLE_YAML } from '../orchestrateStore';
import type { ChatStreamEvent } from '../../types';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

describe('orchestrateStore', () => {
  beforeEach(() => {
    useOrchestrateStore.getState().reset();
  });

  describe('parseNodeIds', () => {
    it('extracts node ids from sample YAML', () => {
      const ids = parseNodeIds(SAMPLE_YAML);
      expect(ids).toEqual(['prompt_1', 'agent_1', 'gate_1']);
    });

    it('returns empty when no nodes block', () => {
      expect(parseNodeIds('start: a\nend: a\n')).toEqual([]);
    });

    it('stops at the edges section', () => {
      const yaml = [
        'start: a',
        'end: b',
        'nodes:',
        '  a:',
        '    type: prompt',
        '    text: hi',
        '  b:',
        '    type: merge',
        'edges:',
        '  - { from: a, to: b }',
      ].join('\n');
      expect(parseNodeIds(yaml)).toEqual(['a', 'b']);
    });
  });

  describe('applyEvent', () => {
    it('marks node running on node_start', () => {
      useOrchestrateStore.getState().applyEvent({ kind: 'node_start', node: 'agent_1' });
      expect(useOrchestrateStore.getState().nodes['agent_1']).toEqual({ status: 'running' });
    });

    it('marks node done on node_end', () => {
      useOrchestrateStore.getState().applyEvent({
        kind: 'node_end',
        node: 'agent_1',
        status: 'done',
      });
      expect(useOrchestrateStore.getState().nodes['agent_1']?.status).toBe('done');
    });

    it('records error on failed node_end', () => {
      useOrchestrateStore.getState().applyEvent({
        kind: 'node_end',
        node: 'gate_1',
        status: 'failed',
        error: 'compile failed',
      });
      expect(useOrchestrateStore.getState().nodes['gate_1']).toEqual({
        status: 'failed',
        error: 'compile failed',
      });
    });

    it('sets pendingApproval on approval_required', () => {
      useOrchestrateStore.getState().applyEvent({
        kind: 'approval_required',
        node: 'human_1',
        prompt: 'deploy to prod?',
        resume_token: 'approve__human_1',
      });
      const store = useOrchestrateStore.getState();
      expect(store.nodes['human_1']?.status).toBe('waiting_approval');
      expect(store.pendingApproval?.resumeToken).toBe('approve__human_1');
    });

    it('clears runId and sets output on graph_done', () => {
      useOrchestrateStore.getState().startRun('r1');
      useOrchestrateStore.getState().applyEvent({
        kind: 'graph_done',
        output: { result: 'ok' },
      });
      const store = useOrchestrateStore.getState();
      expect(store.runId).toBeNull();
      expect(store.output).toEqual({ result: 'ok' });
    });

    it('sets error on graph_failed', () => {
      useOrchestrateStore.getState().applyEvent({
        kind: 'graph_failed',
        error: 'cycle detected',
      });
      expect(useOrchestrateStore.getState().error).toBe('cycle detected');
    });

    it('accumulates a text node_output chunk into blocks', () => {
      useOrchestrateStore.getState().applyEvent({ kind: 'node_start', node: 'agent_1' });
      useOrchestrateStore.getState().applyEvent({
        kind: 'node_output',
        node: 'agent_1',
        chunk: { kind: 'text', content: 'hello' } as ChatStreamEvent,
      });
      expect(useOrchestrateStore.getState().nodes['agent_1']?.blocks).toEqual([
        { kind: 'text', content: 'hello' },
      ]);
    });

    it('merges consecutive text chunks (same semantics as agentStore.appendBlock)', () => {
      useOrchestrateStore.getState().applyEvent({ kind: 'node_start', node: 'agent_1' });
      useOrchestrateStore.getState().applyEvent({
        kind: 'node_output', node: 'agent_1',
        chunk: { kind: 'text', content: 'hel' } as ChatStreamEvent,
      });
      useOrchestrateStore.getState().applyEvent({
        kind: 'node_output', node: 'agent_1',
        chunk: { kind: 'text', content: 'lo' } as ChatStreamEvent,
      });
      // Two text deltas fold into ONE text block — otherwise BlocksView would
      // render hundreds of per-token cards.
      expect(useOrchestrateStore.getState().nodes['agent_1']?.blocks).toEqual([
        { kind: 'text', content: 'hello' },
      ]);
    });

    it('keeps tool_use and tool_result as separate blocks (no merge across kinds)', () => {
      useOrchestrateStore.getState().applyEvent({ kind: 'node_start', node: 'agent_1' });
      useOrchestrateStore.getState().applyEvent({
        kind: 'node_output', node: 'agent_1',
        chunk: { kind: 'text', content: 'reading' } as ChatStreamEvent,
      });
      useOrchestrateStore.getState().applyEvent({
        kind: 'node_output', node: 'agent_1',
        chunk: { kind: 'tool_use', name: 'Read', input: { file_path: '/x' } } as ChatStreamEvent,
      });
      useOrchestrateStore.getState().applyEvent({
        kind: 'node_output', node: 'agent_1',
        chunk: { kind: 'tool_result', content: 'file contents', is_error: false } as ChatStreamEvent,
      });
      expect(useOrchestrateStore.getState().nodes['agent_1']?.blocks).toEqual([
        { kind: 'text', content: 'reading' },
        { kind: 'tool_use', name: 'Read', input: { file_path: '/x' } },
        { kind: 'tool_result', content: 'file contents', is_error: false },
      ]);
    });

    it('degrades a chunk without a kind discriminator to a text block', () => {
      // Test/mock executors emit {partial}; a stray non-ChatStreamEvent chunk
      // must still surface (as text) rather than vanish.
      useOrchestrateStore.getState().applyEvent({ kind: 'node_start', node: 'agent_1' });
      useOrchestrateStore.getState().applyEvent({
        kind: 'node_output', node: 'agent_1',
        chunk: { partial: 'STUB' },
      });
      expect(useOrchestrateStore.getState().nodes['agent_1']?.blocks).toEqual([
        { kind: 'text', content: 'STUB' },
      ]);
    });
  });

  describe('startRun / reset', () => {
    it('startRun clears nodes and sets runId', () => {
      useOrchestrateStore.getState().applyEvent({ kind: 'node_start', node: 'x' });
      useOrchestrateStore.getState().startRun('r2');
      const store = useOrchestrateStore.getState();
      expect(store.nodes).toEqual({});
      expect(store.runId).toBe('r2');
      expect(store.error).toBeNull();
    });

    it('reset clears everything', () => {
      useOrchestrateStore.getState().startRun('r3');
      useOrchestrateStore.getState().applyEvent({ kind: 'node_start', node: 'y' });
      useOrchestrateStore.getState().reset();
      const store = useOrchestrateStore.getState();
      expect(store.nodes).toEqual({});
      expect(store.runId).toBeNull();
      expect(store.output).toBeNull();
    });
  });

  describe('approve — Human-node approval loop', () => {
    beforeEach(() => {
      vi.clearAllMocks();
      useOrchestrateStore.getState().reset();
    });

    it('forwards an approve decision to approve_workflow_step and clears the prompt', async () => {
      vi.mocked(invoke).mockResolvedValue(undefined);
      useOrchestrateStore.setState({
        runId: 'run-1',
        pendingApproval: { node: 'human_1', prompt: '继续吗?', resumeToken: 'tok-abc' },
      });

      await useOrchestrateStore.getState().approve(true);

      expect(invoke).toHaveBeenCalledWith('approve_workflow_step', {
        runId: 'run-1',
        resumeToken: 'tok-abc',
        approved: true,
      });
      expect(useOrchestrateStore.getState().pendingApproval).toBeNull();
    });

    it('forwards a reject decision with approved=false', async () => {
      vi.mocked(invoke).mockResolvedValue(undefined);
      useOrchestrateStore.setState({
        runId: 'run-9',
        pendingApproval: { node: 'h', prompt: 'x', resumeToken: 't' },
      });

      await useOrchestrateStore.getState().approve(false);

      expect(invoke).toHaveBeenCalledWith('approve_workflow_step', {
        runId: 'run-9',
        resumeToken: 't',
        approved: false,
      });
    });

    it('is a no-op when there is no pending approval or no run', async () => {
      useOrchestrateStore.setState({ runId: 'run-1', pendingApproval: null });
      await useOrchestrateStore.getState().approve(true);
      expect(invoke).not.toHaveBeenCalled();

      useOrchestrateStore.setState({
        runId: null,
        pendingApproval: { node: 'h', prompt: 'x', resumeToken: 't' },
      });
      await useOrchestrateStore.getState().approve(true);
      expect(invoke).not.toHaveBeenCalled();
    });

    it('keeps the prompt visible when the invoke rejects (run still live)', async () => {
      vi.mocked(invoke).mockRejectedValue(new Error('channel closed'));
      useOrchestrateStore.setState({
        runId: 'run-7',
        pendingApproval: { node: 'h', prompt: 'x', resumeToken: 't' },
      });

      await useOrchestrateStore.getState().approve(true);

      // The optimistic clear only runs after a successful invoke — a rejection
      // must leave the prompt so the user can retry.
      expect(useOrchestrateStore.getState().pendingApproval).not.toBeNull();
    });
  });
});
