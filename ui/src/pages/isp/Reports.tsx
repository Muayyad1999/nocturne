/**
 * The four reports an ISP cannot get out of a spreadsheet, because they need
 * the network: revenue, used against sold, oversubscription and margin.
 *
 * Each is its own panel with its own window, because each is a reading of a
 * different thing: revenue is a window of the ledger, usage and load are
 * measurements at a moment or averaged over a window, and margin is a monthly
 * figure against a recorded wholesale cost. They are deliberately not wired
 * to one shared date picker that would imply they are slices of one report.
 */

import { Capacity } from './Capacity';
import { Margin } from './Margin';
import { Revenue } from './Revenue';
import { Usage } from './Usage';
import styles from './isp.module.css';

export function Reports() {
  return (
    <div className={styles.scrollPane}>
      <Revenue />
      <Usage />
      <Capacity />
      <Margin />
    </div>
  );
}
