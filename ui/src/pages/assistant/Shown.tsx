import { Fragment } from 'react';
import { runsOf, type Run } from './request';
import styles from './Transcript.module.css';

/**
 * Text a model, a device or a person wrote, drawn as text: every character
 * that would change how it reads without being seen is written out where it
 * stands, in a mark of its own (`request.ts`).
 */
export function Shown({ text, runs }: { text: string; runs?: Run[] }) {
  return (
    <>
      {(runs ?? runsOf(text)).map((run, index) =>
        'unseen' in run ? (
          <span key={index} className={styles.unseen} data-unseen={run.unseen}>
            {run.unseen}
          </span>
        ) : (
          <Fragment key={index}>{run.text}</Fragment>
        ),
      )}
    </>
  );
}
