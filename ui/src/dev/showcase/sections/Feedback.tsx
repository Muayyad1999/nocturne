import { Plus, Server } from 'lucide-react';
import { RpcError } from '@/api/errors';
import { Button } from '@/design/Button';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Skeleton } from '@/design/Skeleton';
import { Spinner } from '@/design/Spinner';
import { toast } from '@/design/Toast';
import { Example, Section } from '../parts';
import styles from '../Showcase.module.css';

const UNREACHABLE = new RpcError('unavailable', 'core-sw-01 did not answer within 10 seconds.');

export function Feedback() {
  return (
    <>
      <Section
        title="EmptyState"
        description="Never a dead end: it says what the screen is for and offers the one action that fills it."
      >
        <Example title="Page" span={2} layout="fill">
          <EmptyState
            icon={Server}
            title="No devices yet"
            description="Add a device by address, or let Nocturne discover what is on a subnet."
            action={
              <>
                <Button variant="primary" icon={Plus}>
                  Add device
                </Button>
                <Button>Discover a subnet</Button>
              </>
            }
          />
        </Example>
        <Example title="Compact, for cards and panels" layout="fill">
          <EmptyState
            size="compact"
            icon={Server}
            title="No neighbours found"
            description="This device does not report LLDP or CDP neighbours."
          />
        </Example>
      </Section>

      <Section
        title="ErrorState"
        description="What happened, in the backend's own words, with the error code and a way to try again."
      >
        <Example title="Page" span={2} layout="fill">
          <ErrorState
            title="The interfaces could not be loaded"
            error={UNREACHABLE}
            onRetry={() => toast.info('Retrying')}
            actions={<Button variant="ghost">Open terminal instead</Button>}
          />
        </Example>
        <Example title="Compact" layout="fill">
          <ErrorState
            size="compact"
            title="Neighbours could not be loaded"
            error="The device refused the SNMP request."
            onRetry={() => toast.info('Retrying')}
          />
        </Example>
      </Section>

      <Section
        title="Skeleton and Spinner"
        description="A loading page shows skeletons shaped like its content. A spinner is for a small region: a button, a row, a panel header."
      >
        <Example title="Skeleton: shaped like what is coming" span={2} layout="column">
          <div className={styles.inline} style={{ flexWrap: 'nowrap' }}>
            <Skeleton shape="circle" width={32} height={32} />
            <div className={styles.stack} style={{ gap: 8 }}>
              <Skeleton width="38%" height={14} />
              <Skeleton width="62%" />
            </div>
          </div>
          <Skeleton shape="block" height={72} />
          <Skeleton width="88%" />
          <Skeleton width="74%" />
        </Example>
        <Example title="Spinner">
          <Spinner size={12} />
          <Spinner />
          <Spinner size={24} />
          <span className={styles.inline} style={{ gap: 8 }}>
            <Spinner label={null} />
            Collecting from 12 devices
          </span>
        </Example>
      </Section>
    </>
  );
}
