/**
 * What the very first person sees: an inventory with nothing in it.
 *
 * Two doors, one sentence each. The first builds the demo campus — simulated
 * devices that behave like real ones, so within a few seconds this screen has
 * something true to show; everything from it carries the LAB mark for ever.
 * The second goes to the inventory, where "Add device" is the primary action.
 *
 * `spec-ui-pages.md` asks for a third door, *Discover my network*. There is no
 * discovery screen in this build (no route, no page folder — the engine has
 * `discovery.*` but nothing drives it), and a door that leads nowhere is worse
 * than a door that is not there. See NOTES.md.
 */

import { FlaskConical, Plus, Rocket } from 'lucide-react';
import { Link } from 'react-router';
import { Button } from '@/design/Button';
import { Icon } from '@/design/Icon';
import styles from './FirstRun.module.css';

export interface FirstRunProps {
  /** Whether this build can simulate a network at all. */
  canSimulate: boolean;
  onExploreDemo: () => void;
  starting: boolean;
}

export function FirstRun({ canSimulate, onExploreDemo, starting }: FirstRunProps) {
  return (
    <section className={styles.welcome} aria-labelledby="overview-welcome">
      <h2 id="overview-welcome" className={styles.title}>
        Nothing is in the inventory yet
      </h2>
      <p className={styles.lead}>
        This screen opens the morning with what is down, what is firing and what changed. It has
        nothing to read until Nocturne knows about a device.
      </p>

      <div className={styles.doors}>
        {canSimulate && (
          <article className={styles.door}>
            <span className={styles.icon}>
              <Icon icon={FlaskConical} size={18} />
            </span>
            <h3 className={styles.doorTitle}>Explore the demo network</h3>
            <p className={styles.doorText}>
              A simulated campus runs inside the engine — devices, links, configurations and
              findings — so you can try everything, safely, before touching your own network.
            </p>
            <Button variant="primary" icon={Rocket} loading={starting} onClick={onExploreDemo}>
              Start the demo network
            </Button>
          </article>
        )}

        <article className={styles.door}>
          <span className={styles.icon}>
            <Icon icon={Plus} size={18} />
          </span>
          <h3 className={styles.doorTitle}>Add a device</h3>
          <p className={styles.doorText}>
            Name it, give it an address and a credential, and test the connection before saving
            anything. Nothing is read from it until you ask.
          </p>
          <Link className={styles.secondary} to="/devices">
            Go to Devices
          </Link>
        </article>
      </div>
    </section>
  );
}
