import { createEffect, createSignal, For, onCleanup, onMount } from "solid-js";
import { AnsiUp } from "ansi_up";
import { computeVisibleRange } from "./virtualize";
import { onJobLog } from "../lib/events";

const ROW_HEIGHT = 18;
const OVERSCAN = 20;
// A hard cap on retained lines, since PROMPT.md specifies the virtualized
// *window* but not a retention bound — without one, a pathologically
// chatty build could grow this signal without limit.
const MAX_LINES = 50_000;

interface LogLine {
  id: number;
  html: string;
}

function LogViewer() {
  const [lines, setLines] = createSignal<LogLine[]>([]);
  const [scrollTop, setScrollTop] = createSignal(0);
  const [containerHeight, setContainerHeight] = createSignal(400);
  const [autoScroll, setAutoScroll] = createSignal(true);
  let containerRef: HTMLDivElement | undefined;
  let nextId = 0;
  const ansiUp = new AnsiUp();

  onMount(() => {
    const unlistenPromise = onJobLog((message) => {
      if (message.type !== "LogChunk") return;
      const html = ansiUp.ansi_to_html(message.data);
      setLines((prev) => {
        const next = [...prev, { id: nextId++, html }];
        return next.length > MAX_LINES
          ? next.slice(next.length - MAX_LINES)
          : next;
      });
    });
    onCleanup(() => {
      unlistenPromise.then((unlisten) => unlisten());
    });

    if (containerRef) {
      setContainerHeight(containerRef.clientHeight);
      const observer = new ResizeObserver(() => {
        if (containerRef) setContainerHeight(containerRef.clientHeight);
      });
      observer.observe(containerRef);
      onCleanup(() => observer.disconnect());
    }
  });

  createEffect(() => {
    lines();
    if (autoScroll() && containerRef) {
      containerRef.scrollTop = containerRef.scrollHeight;
    }
  });

  const range = () =>
    computeVisibleRange({
      scrollTop: scrollTop(),
      containerHeight: containerHeight(),
      rowHeight: ROW_HEIGHT,
      itemCount: lines().length,
      overscan: OVERSCAN,
    });

  const visibleLines = () => lines().slice(range().start, range().end);

  return (
    <div class="flex h-full flex-col">
      <div class="flex items-center justify-between border-b border-neutral-800 px-3 py-1 text-xs text-neutral-400">
        <span>{lines().length} lines</span>
        <label class="flex items-center gap-1">
          <input
            type="checkbox"
            checked={autoScroll()}
            onChange={(e) => setAutoScroll(e.currentTarget.checked)}
          />
          auto-scroll
        </label>
      </div>
      <div
        ref={containerRef}
        class="flex-1 overflow-y-auto bg-black font-mono text-xs"
        onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
      >
        <div
          style={{
            height: `${lines().length * ROW_HEIGHT}px`,
            position: "relative",
          }}
        >
          <div
            style={{
              position: "absolute",
              top: `${range().start * ROW_HEIGHT}px`,
              left: "0",
              right: "0",
            }}
          >
            <For each={visibleLines()}>
              {(line) => (
                // eslint-disable-next-line solid/no-innerhtml -- ansi_up HTML-escapes plain text and only emits styling spans for ANSI codes
                <div
                  style={{ height: `${ROW_HEIGHT}px` }}
                  innerHTML={line.html}
                />
              )}
            </For>
          </div>
        </div>
      </div>
    </div>
  );
}

export default LogViewer;
