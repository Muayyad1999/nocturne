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
import type { LayerState, LinkLoad } from './load';

export interface LegendProps {
  graph: Graph | undefined;
  /** The loads being drawn, or `null` when the layer is off or has nothing. */
  loads?: ReadonlyMap<string, LinkLoad> | null;
  loadLayer?: LayerState;
}

/**
 * What each of the four load appearances means. They are four different
 * facts and the drawing shows them four different ways, so the key has to
 * say which is which — "no rate" and "idle" above all, because a reader who
 * confuses those two is being told a link is quiet when nobody has looked.
 */
const LOAD_LEGEND: readonly { id: string; title: string; text: string }[] = [
  { id: 'scaled', title: 'Loaded', text: 'the bar is how much of the port speed is in use' },
  { id: 'idle', title: 'Idle', text: 'measured, and nothing is moving' },
  { id: 'unscaled', title: 'No speed', text: 'a rate, but the port never said how fast it is' },
  { id: 'unknown', title: 'No rate', text: 'nothing has been measured — not the same as idle' },
  { id: 'stale', title: 'Stale', text: 'older than this device is measured, so not current' },
];

export function Legend({ graph, loads, loadLayer = 'ready' }: LegendProps) {
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

  // Only the load states this drawing actually contains, for the same reason
  // as the confidences: a key that explains something that is not on screen
  // is noise, and a row that is missing is itself worth noticing.
  const present = new Set<string>();
  for (const load of loads?.values() ?? []) {
    for (const direction of [load.aToB, load.bToA]) {
      present.add(direction.kind === 'scaled' && direction.bps === 0 ? 'idle' : direction.kind);
      if (direction.age.state === 'stale') present.add('stale');
    }
  }
  const loadRows = LOAD_LEGEND.filter((row) => present.has(row.id));

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

      {loadRows.length > 0 && (
        <>
          <span className={styles.divider} />
          <ul className={styles.rows} aria-label="Link load">
            {loadRows.map((row) => (
              <li key={row.id}>
                <LoadSample state={row.id} />
                <span>
                  <b>{row.title}</b> — {row.text}
                </span>
              </li>
            ))}
          </ul>
        </>
      )}

      {loadLayer === 'absent' && loads === null && (
        <>
          <span className={styles.divider} />
          <span className={styles.none}>
            Link load needs monitoring, which this build is not answering for.
          </span>
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

/**
 * A little drawing of one load state: the channel, and the bar in it. The
 * same shapes the canvas uses, so the key and the drawing cannot drift apart
 * by being written twice in two different ways.
 */
function LoadSample({ state }: { state: string }) {
  const fill = state === 'scaled' ? 18 : state === 'stale' ? 18 : 0;
  return (
    <svg className={styles.sample} viewBox="0 0 30 10" aria-hidden="true">
      {state !== 'unknown' && (
        <line
          className={styles.loadTrack}
          data-kind={state}
          x1={1}
          y1={5}
          x2={29}
          y2={5}
        />
      )}
      {fill > 0 && (
        <line
          className={styles.loadFill}
          data-state={state}
          x1={1}
          y1={5}
          x2={1 + fill}
          y2={5}
        />
      )}
    </svg>
  );
}
