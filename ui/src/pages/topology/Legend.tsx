/**
 * The key to the drawing: a strip under the canvas, always on screen.
 *
 * It is not decoration. A line here is a claim about reality, and the three
 * kinds of claim — both ends reported it, one end did, nobody did and it was
 * worked out — look different on purpose. A reader who cannot tell them apart
 * is being misled. It sits under the drawing rather than floating over a
 * corner of it (where it covered three devices in the first screenshots) and
 * uses the same words as the backend's own SVG legend.
 */

import type { Graph } from '@/api/types/topology';
import { Icon } from '@/design/Icon';
import { Tooltip } from '@/design/Tooltip';
import { CONFIDENCE_LEGEND, KIND_ORDER, SOURCE_LEGEND, nodeIcon, nodeKindLabel } from './graph';
import styles from './Legend.module.css';

export interface LegendProps {
  graph: Graph | undefined;
}

export function Legend({ graph }: LegendProps) {
  // Only what is on this drawing: a key that explains things that are not on
  // screen is noise, and a missing row is itself worth noticing.
  const confidences = CONFIDENCE_LEGEND.filter((row) =>
    graph?.links.some((link) => link.confidence === row.confidence),
  );
  // Only the two sources a confidence does not already explain: a neighbour
  // protocol is what `confirmed` and `one-sided` are about, and a subnet or a
  // MAC table is what `inferred` is about, so saying either again is noise.
  const sources = SOURCE_LEGEND.filter(
    (row) =>
      (row.source === 'drawn' && graph?.links.some((link) => link.kind === 'manual')) ||
      (row.source === 'lab' && graph?.links.some((link) => link.kind === 'lab')),
  );
  const kinds = KIND_ORDER.filter((kind) => graph?.nodes.some((node) => node.kind === kind));

  return (
    <section className={styles.legend} aria-label="Legend">
      <h2>How each link is known</h2>

      <ul className={styles.rows}>
        {confidences.map((row) => (
          <li key={row.confidence}>
            <Sample confidence={row.confidence} />
            <span>
              <b>{row.title}</b> — {row.text}
            </span>
          </li>
        ))}
        {confidences.length === 0 && <li className={styles.none}>No links are drawn.</li>}
      </ul>

      {sources.length > 0 && (
        <>
          <span className={styles.divider} />
          <ul className={styles.rows}>
            {sources.map((row) => (
              <li key={row.source}>
                <Sample source={row.source} />
                <span>
                  <b>{row.title}</b> — {row.text}
                </span>
              </li>
            ))}
          </ul>
        </>
      )}

      {kinds.length > 0 && (
        <>
          <span className={styles.divider} />
          <ul className={styles.kinds}>
            {kinds.map((kind) => (
              <li key={kind}>
                <Tooltip content={nodeKindLabel(kind)}>
                  <span tabIndex={0} aria-label={nodeKindLabel(kind)}>
                    <Icon icon={nodeIcon(kind)} size={13} />
                  </span>
                </Tooltip>
              </li>
            ))}
          </ul>
          <Tooltip content="A dashed card with a + is a neighbour that is not in the inventory. The bar down a card's left edge is the device's state: green up, red down, grey nobody has looked.">
            <span className={styles.more} tabIndex={0}>
              and the cards
            </span>
          </Tooltip>
        </>
      )}
    </section>
  );
}

function Sample({
  confidence,
  source,
}: {
  confidence?: (typeof CONFIDENCE_LEGEND)[number]['confidence'];
  source?: (typeof SOURCE_LEGEND)[number]['source'];
}) {
  return (
    <svg className={styles.sample} viewBox="0 0 30 10" aria-hidden="true">
      <line
        className={styles.sampleLine}
        {...(confidence ? { 'data-confidence': confidence } : {})}
        {...(source ? { 'data-source': source } : {})}
        x1={1}
        y1={5}
        x2={29}
        y2={5}
      />
    </svg>
  );
}
