import { Bell, BellOff } from 'lucide-react';
import { Button, IconButton } from '@/design/Button';
import { EmptyState } from '@/design/EmptyState';
import { Icon } from '@/design/Icon';
import { Popover } from '@/design/Popover';
import { TimeAgo } from '@/design/TimeAgo';
import { TOAST_APPEARANCE, notifications, useToasts } from '@/design/Toast';
import { formatCount } from '@/lib/format';
import styles from './Header.module.css';

/** The bell: everything that was announced this session, newest first. */
export function Notifications() {
  const history = useToasts((state) => state.history);
  const unread = useToasts((state) => state.unread);

  return (
    <span className={styles.bell}>
      <Popover
        aria-label="Notifications"
        placement="bottom-end"
        className={styles.panel}
        onOpenChange={(open) => {
          if (open) notifications.markAllRead();
        }}
        trigger={
          <IconButton
            icon={Bell}
            aria-label={unread > 0 ? `Notifications, ${formatCount(unread)} unread` : 'Notifications'}
            tooltip="Notifications"
            tooltipPlacement="bottom-end"
          />
        }
      >
        <div className={styles.panelHeader}>
          <h2 className={styles.panelTitle}>Notifications</h2>
          {history.length > 0 && (
            <Button variant="ghost" size="sm" onClick={notifications.clear}>
              Clear
            </Button>
          )}
        </div>
        {history.length === 0 ? (
          <EmptyState
            size="compact"
            icon={BellOff}
            title="Nothing yet"
            description="Results of your actions and alerts raised during this session are kept here."
          />
        ) : (
          <ul className={styles.list}>
            {history.map((item) => {
              const { icon, tone } = TOAST_APPEARANCE[item.tone];
              return (
                <li key={item.id} className={styles.notification} data-tone={tone}>
                  <Icon icon={icon} className={styles.notificationIcon} />
                  <div className={styles.notificationBody}>
                    <p className={styles.notificationTitle}>{item.title}</p>
                    {item.description && <p className={styles.notificationText}>{item.description}</p>}
                  </div>
                  <TimeAgo at={item.at} className={styles.notificationTime} />
                </li>
              );
            })}
          </ul>
        )}
      </Popover>
      {unread > 0 && <span className={styles.unread} />}
    </span>
  );
}
