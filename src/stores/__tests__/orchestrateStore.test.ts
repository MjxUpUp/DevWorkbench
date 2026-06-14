import { describe, it, expect, beforeEach } from 'vitest';
import { parseNodeIds, useOrchestrateStore, SAMPLE_YAML } from '../orchestrateStore';

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
});
