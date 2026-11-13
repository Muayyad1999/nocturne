/**
 * The top row: five readings, each a link to the screen that acts on it.
 *
 * A tile is one of two things and looks like one of two things. Either it is a
 * reading — a number, its unit, and the lines that say what the number is made
 * of — or it is a part the backend could not fill, in which case there is no
 * number at all, only the reason. The specification says such a tile is
 * "hidden, not zeroed"; it is shown here instead, because a hidden tile is
 * indistinguishable from a tile that never existed, and the reason ("there is
 * no alerting in this build") is the most useful thing the tile can say. What
 * the specification is really insisting on — that a missing source never reads
 * as `0` — is kept: the figure is never drawn for such a tile.
 *
 * A count that the backend read out of a capped list carries "at least", so a
 * floor can never be mistaken for a total.
 */

import { ChevronRight } from 'lucide-react';
import { Link } from 'react-router';
import { Icon } from '@/design/Icon';
import { Skeleton } from '@/design/Skeleton';
import { TimeAgo } from '@/design/TimeAgo';
import { Tooltip } from '@/design/Tooltip';
import type { TileView } from './overview';
import styles from './Tiles.module.css';
import { UnavailableNote } from './Unavailable';

export interface TilesProps {
  tiles: readonly TileView[];
}

export function Tiles({ tiles }: TilesProps) {
  return (
    <div className={styles.row}>
      {tiles.map((tile) => (
        <Tile key={tile.id} tile={tile} />
      ))}
    </div>
  );
}

function Tile({ tile }: { tile: TileView }) {
  const { reading } = tile;
  return (
    <Link
      to={tile.to}
      className={styles.tile}
      data-kind={reading.kind}
      data-tile={tile.id}
      aria-label={`${tile.label}: ${tile.linkLabel}`}
    >
      <span className={styles.label}>
        {tile.label}
        <Icon icon={ChevronRight} size={13} className={styles.chevron} />
      </span>

      {reading.kind === 'unavailable' ? (
        <UnavailableNote reason={reading.reason} message={reading.message} />
      ) : (
        <>
          <span className={styles.figure} title={reading.provenance}>
            {reading.floor && (
              <Tooltip content="The backend read as far as it was willing to, so this is a floor, not a total.">
                <span className={styles.floor}>at least</span>
              </Tooltip>
            )}
            <span className={styles.value}>{reading.value}</span>
            {reading.unit && <span className={styles.unit}>{reading.unit}</span>}
          </span>
          <span className={styles.notes}>
            {reading.notes.map((note) => (
              <span key={note.text} className={styles.note} data-tone={note.tone}>
                {note.text}
                {note.at !== undefined && <TimeAgo at={note.at} />}
              </span>
            ))}
          </span>
        </>
      )}
    </Link>
  );
}

/** The row while the first read is in flight: five tiles' worth of outline. */
export function TilesSkeleton() {
  return (
    <div className={styles.row} aria-busy="true" aria-label="Reading the state of the network">
      {[0, 1, 2, 3, 4].map((index) => (
        <div key={index} className={styles.tile} data-kind="loading">
          <Skeleton width={88} height={12} />
          <Skeleton width={64} height={28} />
          <Skeleton width={120} height={12} />
        </div>
      ))}
    </div>
  );
}
