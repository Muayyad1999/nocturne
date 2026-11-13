import { RotateCcwClock, Sparkles, X } from 'lucide-react';
import { useState } from 'react';
import { useNavigate } from 'react-router';
import { IconButton } from '@/design/Button';
import { Icon } from '@/design/Icon';
import { ResizeHandle } from '@/design/Splitter';
import AssistantDockContent from '@/pages/assistant/Dock';
import { DOCK_DEFAULT_WIDTH, DOCK_MAX_WIDTH, DOCK_MIN_WIDTH, useLayout } from '../layout';
import styles from './AssistantDock.module.css';

/**
 * The assistant dock's frame: a resizable panel with a header. What goes
 * inside (the transcript and the composer) belongs to the Assistant slice,
 * in `pages/assistant/Dock.tsx`, built from DockTranscript and DockComposer.
 */
export function AssistantDock() {
  const storedWidth = useLayout((state) => state.dockWidth);
  const setDockWidth = useLayout((state) => state.setDockWidth);
  const setDockOpen = useLayout((state) => state.setDockOpen);
  // While dragging, the width is local; it is stored once, when the drag ends.
  const [dragWidth, setDragWidth] = useState<number | null>(null);
  const width = dragWidth ?? storedWidth;
  const navigate = useNavigate();

  return (
    <aside className={styles.dock} aria-label="Assistant" style={{ width }}>
      <ResizeHandle
        orientation="vertical"
        reverse
        value={width}
        min={DOCK_MIN_WIDTH}
        max={DOCK_MAX_WIDTH}
        defaultValue={DOCK_DEFAULT_WIDTH}
        onChange={setDragWidth}
        onChangeEnd={(next) => {
          setDockWidth(next);
          setDragWidth(null);
        }}
        aria-label="Resize assistant"
      />
      <div className={styles.panel}>
        <header className={styles.header}>
          <Icon icon={Sparkles} className={styles.spark} />
          <h2 className={styles.title}>Assistant</h2>
          <IconButton
            icon={RotateCcwClock}
            aria-label="Conversation history"
            tooltipPlacement="bottom"
            onClick={() => void navigate('/assistant')}
          />
          <IconButton
            icon={X}
            aria-label="Close assistant"
            tooltipPlacement="bottom-end"
            onClick={() => setDockOpen(false)}
          />
        </header>
        <AssistantDockContent />
      </div>
    </aside>
  );
}
