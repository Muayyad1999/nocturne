import { Icon } from '@/design/Icon';
import { StatusDot } from '@/design/StatusDot';
import styles from './StatusBar.module.css';
import { selectStatusItems, useStatusItems, type StatusItem } from './statusItems';

function Item({ item }: { item: StatusItem }) {
  const content = (
    <>
      {/* The label states the condition; the dot repeats it as shape and colour. */}
      {item.tone && <StatusDot tone={item.tone} size={8} />}
      {item.icon && <Icon icon={item.icon} size={12} />}
      <span>{item.label}</span>
    </>
  );
  return item.onSelect ? (
    <button type="button" className={styles.item} title={item.detail} onClick={item.onSelect}>
      {content}
    </button>
  ) : (
    <span className={styles.item} title={item.detail}>
      {content}
    </span>
  );
}

/** Small, quiet, always true: live state on the left, facts on the right. */
export function StatusBar() {
  const items = useStatusItems((state) => state.items);
  return (
    <footer className={styles.bar} aria-label="Status bar">
      <div className={styles.side}>
        {selectStatusItems(items, 'left').map((item) => (
          <Item key={item.id} item={item} />
        ))}
      </div>
      <div className={styles.side}>
        {selectStatusItems(items, 'right').map((item) => (
          <Item key={item.id} item={item} />
        ))}
      </div>
    </footer>
  );
}
