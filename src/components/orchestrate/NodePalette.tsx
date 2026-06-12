import type { DAGNodeType } from '../../stores/orchestrateStore';

interface PaletteItem {
  type: DAGNodeType;
  icon: string;
  label: string;
  color: string;
}

const PALETTE_ITEMS: PaletteItem[] = [
  { type: 'prompt', icon: '💬', label: 'Prompt', color: 'var(--node-prompt)' },
  { type: 'agent', icon: '🤖', label: 'Agent', color: 'var(--node-agent)' },
  { type: 'gate', icon: '🛡️', label: 'Gate', color: 'var(--node-gate)' },
  { type: 'parallel', icon: '⫸', label: 'Parallel', color: 'var(--node-parallel)' },
  { type: 'merge', icon: '⫷', label: 'Merge', color: 'var(--node-merge)' },
  { type: 'human', icon: '👤', label: 'Human', color: 'var(--node-human)' },
  { type: 'transform', icon: '⚙️', label: 'Transform', color: 'var(--node-transform)' },
];

function handleDragStart(e: React.DragEvent, item: PaletteItem) {
  e.dataTransfer.setData('application/dag-node-type', item.type);
  e.dataTransfer.effectAllowed = 'copy';
}

export function NodePalette() {
  return (
    <aside className="node-palette">
      <div className="node-palette__title">Nodes</div>
      <div className="node-palette__list">
        {PALETTE_ITEMS.map((item) => (
          <div
            key={item.type}
            className="node-palette__item"
            draggable
            onDragStart={(e) => handleDragStart(e, item)}
            style={{ '--item-color': item.color } as React.CSSProperties}
          >
            <span className="node-palette__item-icon">{item.icon}</span>
            <span className="node-palette__item-label">{item.label}</span>
          </div>
        ))}
      </div>
    </aside>
  );
}
