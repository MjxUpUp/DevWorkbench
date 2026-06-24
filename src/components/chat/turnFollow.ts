// Pure decision: should ChatView's activeLeafId advance to the newest turn?
//
// visibleTurns walks UP the parent chain from activeLeafId, so a turn appended
// AFTER the current leaf (a CHILD of the old last turn) is never reached by
// that upward walk and stays invisible — neither its user message nor its
// streaming reply renders. The leaf advances to the newest turn when:
//   - it is unset (a freshly opened/mounted view), or
//   - it is stale (its turn is no longer present in `turns` — e.g. the user
//     switched to a conversation whose turns differ), or
//   - the newest turn is a DIRECT CHILD of the current leaf — a natural
//     continuation of the branch currently in view.
// A sibling fork (edit-and-regenerate) has a parent that is NOT the leaf, so it
// is intentionally NOT auto-followed — the branch switcher handles it, keeping
// A4's manual-switch semantics intact. This is strictly additive over the
// previous null/stale conditions; fork handling is unchanged.
export function shouldFollowLatest<T extends { id: string; parentSessionId?: string | null }>(
  turns: T[],
  activeLeafId: string | null,
): boolean {
  if (turns.length === 0) return false;
  if (activeLeafId === null) return true;
  if (!turns.some((t) => t.id === activeLeafId)) return true;
  const latest = turns[turns.length - 1];
  return latest.parentSessionId === activeLeafId;
}
