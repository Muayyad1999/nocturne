import { useEffect, useRef, type ReactNode } from 'react';
import styles from './AssistantDock.module.css';

/** How close to the end counts as "following the conversation". */
const FOLLOW_THRESHOLD = 32;

/**
 * The dock's scrollable transcript area. While the reader is at the end, it
 * stays at the end as content arrives; once they scroll up to read, it leaves
 * them where they are.
 */
export function DockTranscript({ children }: { children: ReactNode }) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const following = useRef(true);

  useEffect(() => {
    const scroll = scrollRef.current;
    const content = contentRef.current;
    if (!scroll || !content) return;
    const observer = new ResizeObserver(() => {
      if (following.current) scroll.scrollTop = scroll.scrollHeight;
    });
    observer.observe(content);
    return () => observer.disconnect();
  }, []);

  return (
    <div
      ref={scrollRef}
      className={styles.transcript}
      role="log"
      aria-label="Conversation"
      tabIndex={0}
      onScroll={(event) => {
        const { scrollTop, scrollHeight, clientHeight } = event.currentTarget;
        following.current = scrollHeight - scrollTop - clientHeight < FOLLOW_THRESHOLD;
      }}
    >
      <div ref={contentRef} className={styles.transcriptContent}>
        {children}
      </div>
    </div>
  );
}
