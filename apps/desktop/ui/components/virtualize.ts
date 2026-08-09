export interface VisibleRangeParams {
  scrollTop: number;
  containerHeight: number;
  rowHeight: number;
  itemCount: number;
  overscan: number;
}

export interface VisibleRange {
  start: number;
  end: number;
}

/**
 * Fixed-row-height windowing math for LogViewer: given how far the
 * container has scrolled, returns the slice of `itemCount` rows that
 * should actually be rendered (visible rows plus `overscan` on each
 * side), so a log with tens of thousands of lines only ever mounts a
 * small handful of DOM nodes.
 */
export function computeVisibleRange(params: VisibleRangeParams): VisibleRange {
  const { scrollTop, containerHeight, rowHeight, itemCount, overscan } = params;
  const firstVisible = Math.floor(scrollTop / rowHeight);
  const visibleCount = Math.ceil(containerHeight / rowHeight);
  const start = Math.max(0, firstVisible - overscan);
  const end = Math.min(itemCount, firstVisible + visibleCount + overscan);
  return { start, end };
}
