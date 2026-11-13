import { ChevronRight, Moon, Search, Sun } from 'lucide-react';
import { Fragment } from 'react';
import { Link } from 'react-router';
import { IconButton } from '@/design/Button';
import { Icon } from '@/design/Icon';
import { Kbd } from '@/design/Kbd';
import { THEME_NAME, useTheme } from '@/design/theme';
import { usePalette } from '../CommandPalette/palette';
import styles from './Header.module.css';
import { Notifications } from './Notifications';
import { usePageHeaderStore } from './pageHeader';

/** The header: where you are on the left; search, notifications and theme on the right. */
export function Header() {
  const header = usePageHeaderStore((state) => state.header);
  const theme = useTheme((state) => state.theme);
  const toggleTheme = useTheme((state) => state.toggleTheme);
  const openPalette = usePalette((state) => state.open);
  const otherTheme = THEME_NAME[theme === 'dark' ? 'light' : 'dark'];

  return (
    <header className={styles.header}>
      <div className={styles.location}>
        {header?.breadcrumbs && header.breadcrumbs.length > 0 && (
          <nav aria-label="Breadcrumb">
            <ol className={styles.breadcrumbs}>
              {header.breadcrumbs.map((crumb) => (
                <Fragment key={`${crumb.to ?? ''}:${crumb.label}`}>
                  <li className={styles.crumb}>
                    {crumb.to ? <Link to={crumb.to}>{crumb.label}</Link> : crumb.label}
                  </li>
                  <li aria-hidden="true" className={styles.separator}>
                    <Icon icon={ChevronRight} size={14} />
                  </li>
                </Fragment>
              ))}
            </ol>
          </nav>
        )}
        <h1 className={styles.title}>{header?.title}</h1>
      </div>

      {header?.actions && <div className={styles.actions}>{header.actions}</div>}

      <button type="button" className={styles.search} onClick={openPalette} aria-keyshortcuts="Control+K Meta+K">
        <Icon icon={Search} size={14} />
        <span className={styles.searchLabel}>Search or run a command</span>
        <Kbd shortcut="mod+k" />
      </button>
      <Notifications />
      <IconButton
        icon={theme === 'dark' ? Moon : Sun}
        aria-label={`Switch to ${otherTheme} theme`}
        tooltipPlacement="bottom-end"
        onClick={toggleTheme}
      />
    </header>
  );
}
