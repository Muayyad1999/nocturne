import { Fragment } from 'react';
import { CodeBlock } from '@/design/CodeBlock';
import { parseParts, parseSpans } from './markup';
import styles from './Transcript.module.css';

function Line({ text }: { text: string }) {
  return (
    <>
      {parseSpans(text).map((span, index) => {
        const key = `${index}-${span.text}`;
        if (span.kind === 'code') {
          return (
            <code key={key} className={styles.inlineCode}>
              {span.text}
            </code>
          );
        }
        if (span.kind === 'strong') return <strong key={key}>{span.text}</strong>;
        return <Fragment key={key}>{span.text}</Fragment>;
      })}
    </>
  );
}

/**
 * What the model wrote. Fenced blocks become real code blocks — a model that
 * hands you a configuration should hand you something you can copy — and
 * everything else stays prose.
 */
export function AssistantText({ text }: { text: string }) {
  const parts = parseParts(text);
  return (
    <div className={styles.prose}>
      {parts.map((part, index) => {
        const key = `${index}-${part.kind}`;
        switch (part.kind) {
          case 'code':
            return (
              <CodeBlock
                key={key}
                className={styles.code}
                code={part.text}
                title={part.language || undefined}
                wrap
                maxHeight={320}
              />
            );
          case 'list':
            return part.ordered ? (
              <ol key={key} className={styles.list}>
                {part.items.map((item, at) => (
                  <li key={`${at}-${item}`}>
                    <Line text={item} />
                  </li>
                ))}
              </ol>
            ) : (
              <ul key={key} className={styles.list}>
                {part.items.map((item, at) => (
                  <li key={`${at}-${item}`}>
                    <Line text={item} />
                  </li>
                ))}
              </ul>
            );
          case 'heading':
            return (
              <h4 key={key} className={styles.heading}>
                <Line text={part.text} />
              </h4>
            );
          default:
            return (
              <p key={key} className={styles.paragraph}>
                <Line text={part.text} />
              </p>
            );
        }
      })}
    </div>
  );
}
